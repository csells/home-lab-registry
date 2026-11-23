pub mod deployment;

mod restore;
pub use restore::restore_export;
pub mod client;

mod service;
pub use service::serve;
mod enrollment;
mod http;
mod probe;
pub use http::RegistryHttp;
mod presence;
mod validation;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params, params_from_iter};
use serde_json::{Value, json};

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS services (id TEXT PRIMARY KEY, revision INTEGER NOT NULL, document TEXT NOT NULL, updated REAL NOT NULL);
CREATE TABLE IF NOT EXISTS history (seq INTEGER PRIMARY KEY, id TEXT NOT NULL, revision INTEGER NOT NULL, document TEXT NOT NULL, actor TEXT NOT NULL, updated REAL NOT NULL);
CREATE TABLE IF NOT EXISTS presence (id TEXT PRIMARY KEY, address TEXT NOT NULL, interface TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
CREATE TABLE IF NOT EXISTS runtime_observation (id TEXT PRIMARY KEY, state TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
CREATE TABLE IF NOT EXISTS health (id TEXT PRIMARY KEY, status TEXT NOT NULL, evidence TEXT NOT NULL, checked REAL NOT NULL, expires REAL NOT NULL);
";

#[derive(Debug)]
pub struct Problem {
    status: u16,
    message: String,
}

impl Problem {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
    pub fn status(&self) -> u16 {
        self.status
    }
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Problem {}

impl From<rusqlite::Error> for Problem {
    fn from(error: rusqlite::Error) -> Self {
        tracing::error!(kind=?error.sqlite_error_code(), "Registry database operation failed");
        Self::new(500, "Internal error")
    }
}

impl From<serde_json::Error> for Problem {
    fn from(_error: serde_json::Error) -> Self {
        tracing::error!("Registry database JSON is invalid");
        Self::new(500, "Internal error")
    }
}

#[derive(Clone)]
pub struct Catalog(Arc<PathBuf>, Arc<BTreeSet<String>>);

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time since epoch")
        .as_secs_f64()
}

impl Catalog {
    pub fn open(path: &Path) -> Result<Self, Problem> {
        let catalog = Self(Arc::new(path.to_owned()), Arc::new(BTreeSet::new()));
        catalog.connect()?.execute_batch(SCHEMA)?;
        Ok(catalog)
    }

    pub(crate) fn with_local_checks(mut self, names: BTreeSet<String>) -> Self {
        self.1 = Arc::new(names);
        self
    }

    fn connect(&self) -> Result<Connection, Problem> {
        let connect = Connection::open(self.0.as_path())?;
        connect.busy_timeout(Duration::from_secs(10))?;
        Ok(connect)
    }

    pub fn get(&self, id: &str) -> Result<Value, Problem> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let record = load_record(&transaction, id, now())?;
        transaction.commit()?;
        Ok(record)
    }

    pub fn listing(&self, query: &BTreeMap<String, Vec<String>>) -> Result<Vec<Value>, Problem> {
        let first = |key: &str| {
            query
                .get(key)
                .and_then(|values| values.first())
                .map(String::as_str)
        };
        let include_retired = query
            .get("include_retired")
            .is_some_and(|flag| flag == &["true"]);
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let ids = {
            let mut rows = transaction.prepare("SELECT id FROM services ORDER BY id")?;
            rows.query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let stamp = now();
        let mut result = Vec::new();
        for id in ids {
            let record = load_record(&transaction, &id, stamp)?;
            let spec = &record["spec"];
            if !include_retired && spec["lifecycle"] == "retired" {
                continue;
            }
            if first("capability").is_some_and(|capability| {
                !spec["capabilities"]
                    .as_array()
                    .is_some_and(|all| all.iter().any(|value| value == capability))
            }) || first("host").is_some_and(|host| spec["host"] != host)
                || first("status").is_some_and(|status| record["health"]["status"] != status)
            {
                continue;
            }
            result.push(record);
        }
        transaction.commit()?;
        Ok(result)
    }

    pub fn put(&self, id: &str, mut document: Value, revision: i64) -> Result<Value, Problem> {
        if let Some(resources) = document.get_mut("resources").and_then(Value::as_object_mut) {
            resources.remove("agent_presence");
        }
        validation::spec(&document, &self.1)?;
        let mut connection = self.connect()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row: Option<(i64, String)> = tx
            .query_row(
                "SELECT revision, document FROM services WHERE id=?",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let current = row.as_ref().map_or(0, |entry| entry.0);
        if current != revision {
            return Err(Problem::new(
                409,
                "Revision conflict; fetch latest record before updating",
            ));
        }
        let previous = row
            .map(|(_, value)| serde_json::from_str::<Value>(&value))
            .transpose()?
            .unwrap_or(Value::Null);
        let stamp = now();
        let ordinal = current
            .checked_add(1)
            .ok_or_else(|| Problem::new(409, "Revision range exhausted"))?;
        let serialized = serde_json::to_string(&document)?;
        tx.execute(
            "INSERT OR REPLACE INTO services VALUES (?,?,?,?)",
            params![id, ordinal, serialized, stamp],
        )?;
        tx.execute(
            "INSERT INTO history(id,revision,document,actor,updated) VALUES(?,?,?,?,?)",
            params![id, ordinal, serialized, "writer", stamp],
        )?;
        tx.execute("DELETE FROM health WHERE id=?", [id])?;
        if document["lifecycle"] != previous["lifecycle"]
            || document["resources"]["runtime_binding"] != previous["resources"]["runtime_binding"]
        {
            tx.execute("DELETE FROM runtime_observation WHERE id=?", [id])?;
        }
        let updated = load_record(&tx, id, stamp)?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn observe(&self, id: &str, body: Value, revision: Option<i64>) -> Result<Value, Problem> {
        let mut connection = self.connect()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: i64 = tx
            .query_row("SELECT revision FROM services WHERE id=?", [id], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or_else(|| Problem::new(404, "Service not found"))?;
        let (status, evidence, ttl) = validation::observation(&body)?;
        if revision.is_some_and(|revision| revision != current) {
            return Err(Problem::new(409, "Observation revision conflict"));
        }
        let stamp = now();
        tx.execute(
            "INSERT OR REPLACE INTO health VALUES(?,?,?,?,?)",
            params![id, status, evidence, stamp, stamp + ttl as f64],
        )?;
        let record = load_record(&tx, id, stamp)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn health(&self, ids: &[String]) -> Result<BTreeMap<String, Value>, Problem> {
        let connection = self.connect()?;
        let placeholders = vec!["?"; ids.len()].join(",");
        let mut query = connection.prepare(&format!(
            "SELECT id,status,evidence,checked,expires FROM health WHERE id IN ({placeholders})"
        ))?;
        let records = query
            .query_map(params_from_iter(ids), |row| {
                Ok((row.get(0)?, observation_row(row, 1)?))
            })?
            .collect::<Result<_, _>>()?;
        Ok(records)
    }

    pub fn report_runtime(
        &self,
        id: &str,
        report: &Value,
        expected: &Value,
    ) -> Result<(Value, bool), Problem> {
        let (state, ttl) = validation::runtime_report(report)?;
        let mut connection = self.connect()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let document: String = tx
            .query_row("SELECT document FROM services WHERE id=?", [id], |row| {
                row.get(0)
            })
            .optional()?
            .ok_or_else(|| Problem::new(404, "Service not found"))?;
        let spec: Value = serde_json::from_str(&document)?;
        if spec["lifecycle"] != "active" || spec["resources"]["runtime_binding"] != *expected {
            return Err(Problem::new(
                409,
                "Enrollment no longer matches the active runtime binding",
            ));
        }
        let stamp = now();
        let ttl = ttl as f64;
        let previous = load_runtime(&tx, id)?;
        if let Some(previous) = previous
            && previous["state"] == state
            && previous["expires"]
                .as_f64()
                .is_some_and(|expiry| stamp + ttl / 2.0 <= expiry && expiry <= stamp + ttl)
        {
            return Ok((previous, false));
        }
        tx.execute(
            "INSERT OR REPLACE INTO runtime_observation VALUES(?,?,?,?)",
            params![id, state, stamp, stamp + ttl],
        )?;
        tx.commit()?;
        Ok((
            json!({"state":state,"checked":stamp,"expires":stamp+ttl}),
            true,
        ))
    }

    pub fn history(&self, id: &str) -> Result<Vec<Value>, Problem> {
        let mut connection = self.connect()?;
        let tx = connection.transaction()?;
        let found: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM services WHERE id=?)",
            [id],
            |row| row.get(0),
        )?;
        if !found {
            return Err(Problem::new(404, "Service not found"));
        }
        let history = {
            let mut rows = tx.prepare(
                "SELECT revision,actor,updated,document FROM history WHERE id=? ORDER BY revision",
            )?;
            rows.query_map([id], |row| history_row(row, 0))?
                .collect::<Result<_, _>>()?
        };
        tx.commit()?;
        Ok(history)
    }

    pub fn export(&self) -> Result<Value, Problem> {
        let mut connection = self.connect()?;
        let tx = connection.transaction()?;
        let services = {
            let mut query =
                tx.prepare("SELECT id,revision,updated,document FROM services ORDER BY id")?;
            let values = query
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut services = Vec::with_capacity(values.len());
            for (id, revision, updated, serialized) in values {
                let spec: Value = serde_json::from_str(&serialized)?;
                services
                    .push(json!({"id":id,"revision":revision,"updated_at":updated,"spec":spec}));
            }
            services
        };
        let mut history: BTreeMap<String, Vec<Value>> = services
            .iter()
            .map(|record| {
                (
                    record["id"]
                        .as_str()
                        .expect("known registry ID")
                        .to_string(),
                    Vec::new(),
                )
            })
            .collect();
        {
            let mut query =
                tx.prepare("SELECT id,revision,actor,updated,document FROM history ORDER BY seq")?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                if let Some(history) = history.get_mut(&id) {
                    history.push(history_row(row, 1)?);
                }
            }
        }
        tx.commit()?;
        Ok(json!({"exported_at":now(),"services":services,"history":history}))
    }
}

fn history_row(row: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<Value> {
    Ok(json!({
        "revision":row.get::<_, i64>(start)?,"actor":row.get::<_, String>(start+1)?,
        "updated":row.get::<_, f64>(start+2)?,"document":row.get::<_, String>(start+3)?,
    }))
}

fn observation_row(row: &rusqlite::Row<'_>, start: usize) -> rusqlite::Result<Value> {
    Ok(json!({
        "status":row.get::<_, String>(start)?,"evidence":row.get::<_, String>(start+1)?,
        "checked":row.get::<_, f64>(start+2)?,"expires":row.get::<_, f64>(start+3)?,
    }))
}

fn load_runtime(db: &Connection, id: &str) -> Result<Option<Value>, Problem> {
    Ok(db.query_row("SELECT state,checked,expires FROM runtime_observation WHERE id=?",[id],|row| {
        Ok(json!({"state":row.get::<_, String>(0)?,"checked":row.get::<_, f64>(1)?,"expires":row.get::<_, f64>(2)?}))
    }).optional()?)
}

fn load_document(db: &Connection, id: &str) -> Result<Value, Problem> {
    let document: String = db
        .query_row("SELECT document FROM services WHERE id=?", [id], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or_else(|| Problem::new(404, "Service not found"))?;
    Ok(serde_json::from_str(&document)?)
}

fn load_record(db: &Connection, id: &str, stamp: f64) -> Result<Value, Problem> {
    let (revision, updated, document): (i64, f64, String) = db
        .query_row(
            "SELECT revision,updated,document FROM services WHERE id=?",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| Problem::new(404, "Service not found"))?;
    let mut health = db
        .query_row(
            "SELECT id,status,evidence,checked,expires FROM health WHERE id=?",
            [id],
            |row| observation_row(row, 1),
        )
        .optional()?
        .unwrap_or_else(|| json!({"status":"unknown"}));
    if health["expires"]
        .as_f64()
        .is_some_and(|until| until < stamp)
    {
        health["status"] = json!("stale");
    }
    let mut spec: Value = serde_json::from_str(&document)?;
    presence::apply(db, id, stamp, &mut spec)?;
    let mut record =
        json!({"id":id,"revision":revision,"updated_at":updated,"spec":spec,"health":health});
    if let Some(runtime) = load_runtime(db, id)? {
        record["runtime_observation"] = runtime;
    }
    Ok(record)
}
