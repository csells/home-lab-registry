use rusqlite::Connection;
use serde_json::{Value, json};

use crate::{Catalog, Problem, load_document, now};
use rusqlite::{OptionalExtension, TransactionBehavior, params};

pub(crate) fn connect_address(value: &str, address: &str) -> Option<String> {
    let (scheme, rest) = value.split_once("://")?;
    let stop = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, suffix) = rest.split_at(stop);
    let parsed = url::Url::parse(value).ok()?;
    parsed.host_str()?;
    let host_port = authority.rsplit('@').next()?;
    let explicit = if host_port.starts_with('[') {
        host_port.split_once(']')?.1.strip_prefix(':')
    } else {
        host_port.rsplit_once(':').map(|(_, port)| port)
    };
    let port = explicit
        .filter(|port| !port.is_empty())
        .map_or_else(String::new, |port| format!(":{port}"));
    Some(format!("{scheme}://{address}{port}{suffix}"))
}

fn rewrite(target: &mut Value, address: &str) {
    if let Some(rewritten) = target
        .as_str()
        .and_then(|value| connect_address(value, address))
    {
        *target = json!(rewritten);
    }
}

fn overlay(spec: &mut Value, presence: Value, policy: &Value) {
    let address = presence["address"].as_str().unwrap_or_default().to_owned();
    let resources = &mut spec["resources"];
    resources["agent_presence"] = presence;
    if let Some(routes) = resources["management_access"].as_array_mut() {
        let ids = policy["access_ids"].as_array();
        for route in routes {
            if ids.is_some_and(|ids| ids.contains(&route["id"]))
                && let Some(url) = route.get_mut("url")
            {
                rewrite(url, &address);
            }
        }
    }
    if policy["metrics"] == true
        && let Some(url) = resources["metrics_stream"].get_mut("url")
    {
        rewrite(url, &address);
    }
    if let Some(keys) = policy["endpoint_keys"].as_array() {
        for key in keys.iter().filter_map(Value::as_str) {
            if let Some(url) = spec["endpoints"].get_mut(key) {
                rewrite(url, &address);
            }
        }
    }
    if policy["probe"] == true {
        if let Some(host) = spec["probe"].get_mut("tcp_host") {
            *host = json!(address);
        }
        if let Some(url) = spec["probe"].get_mut("url") {
            rewrite(url, &address);
        }
    }
}

fn load(db: &Connection, id: &str, stamp: f64) -> Result<Option<Value>, Problem> {
    let row = db.query_row(
        "SELECT p.address,p.interface,p.checked,p.expires FROM presence p JOIN services s ON p.id=s.id WHERE p.id=?",
        [id],
        |row| Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, f64>(2)?,row.get::<_, f64>(3)?)),
    ).optional()?;
    Ok(row.map(|(address, interface, checked, expires)| {
        json!({
            "address":address,"interface":interface,"checked":checked,"expires":expires,
            "status":if expires > stamp {"current"} else {"stale"},
        })
    }))
}

pub(crate) fn apply(db: &Connection, id: &str, stamp: f64, doc: &mut Value) -> Result<(), Problem> {
    if let Some(presence) = load(db, id, stamp)? {
        let policy = doc["resources"]["registration"].clone();
        overlay(doc, presence, &policy);
    }
    let bindings = doc["resources"]["address_bindings"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for binding in bindings {
        let Some(machine) = binding["machine_id"].as_str() else {
            continue;
        };
        if let Some(presence) = load(db, machine, stamp)? {
            overlay(doc, presence, &binding);
        }
    }
    Ok(())
}

impl Catalog {
    pub fn announce_verified(
        &self,
        id: &str,
        address: &str,
        interface: &str,
    ) -> Result<(Value, bool), Problem> {
        let mut connection = self.connect()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let spec = load_document(&tx, id)?;
        if spec["lifecycle"] != "active"
            || !spec["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|cap| cap == "machine"))
        {
            return Err(Problem::new(403, "Active enrolled machine required"));
        }
        let previous = tx
            .query_row(
                "SELECT address,interface FROM presence WHERE id=?",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let changed = previous
            .as_ref()
            .is_none_or(|(previous_address, previous_interface)| {
                previous_address != address || previous_interface != interface
            });
        let stamp = now();
        let expires = stamp + 300.0;
        tx.execute(
            "INSERT OR REPLACE INTO presence VALUES(?,?,?,?,?)",
            params![id, address, interface, stamp, expires],
        )?;
        tx.commit()?;
        Ok((json!({"accepted":true,"expires":expires}), changed))
    }

    pub(crate) fn last_enrolled_address(&self, id: &str) -> Result<Option<String>, Problem> {
        let db = self.connect()?;
        Ok(db
            .query_row("SELECT address FROM presence WHERE id=?", [id], |row| {
                row.get(0)
            })
            .optional()?)
    }
}
