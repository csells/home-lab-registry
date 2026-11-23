use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use ring::signature::{ED25519, UnparsedPublicKey};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Rejected, ledger_schema};

pub struct Ledger<G> {
    db: Connection,
    generation: G,
    witness: PathBuf,
    lock: File,
}

impl<G> Drop for Ledger<G> {
    fn drop(&mut self) {
        if let Err(error) = self.lock.unlock() {
            eprintln!("Allocator could not explicitly unlock its ledger lock: {error}");
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Witness {
    revision: String,
    controller_generation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handoff_public_key: Option<String>,
}

#[derive(Deserialize)]
struct HandoffAuthorization {
    revision: String,
    previous_controller_generation: String,
    replacement_controller_generation: String,
    signature: String,
}

fn handoff_denied() -> Rejected {
    hold("Controller handoff does not match the enrolled authority, ledger and replacement VM.")
}

fn handoff_key(text: &str) -> Result<[u8; 32], Rejected> {
    STANDARD
        .decode(text)
        .ok()
        .and_then(|key| key.try_into().ok())
        .ok_or_else(handoff_denied)
}

impl HandoffAuthorization {
    fn verify(&self, seal: &Witness, generation: &str) -> Result<(), Rejected> {
        if self.revision != seal.revision
            || self.previous_controller_generation != seal.controller_generation
            || self.replacement_controller_generation != generation
            || self.previous_controller_generation == generation
            || !crate::tokens::hex(&self.revision, 32)
            || [&self.previous_controller_generation, &self.replacement_controller_generation]
                .iter().any(|value| uuid::Uuid::parse_str(value).is_err() || value.contains('\n'))
        {
            return Err(handoff_denied());
        }
        let key = handoff_key(seal.handoff_public_key.as_deref().ok_or_else(handoff_denied)?)?;
        let signature = STANDARD.decode(&self.signature).map_err(|_| handoff_denied())?;
        let statement = format!(
            "home-lab-allocator-ledger-handoff-v1\n{}\n{}\n{}\n",
            self.revision, self.previous_controller_generation, self.replacement_controller_generation,
        );
        UnparsedPublicKey::new(&ED25519, key)
            .verify(statement.as_bytes(), &signature)
            .map_err(|_| handoff_denied())
    }
}

fn hold(reason: &str) -> Rejected {
    Rejected::new("ledger_reconciliation_required", reason, 409)
}

impl<G: FnMut() -> Result<String, Rejected>> Ledger<G> {
    pub fn open(database: &Path, witness: &Path, generation: G) -> Result<Self, Rejected> {
        let mut ledger = Self::lock_existing(database, witness, generation)?;
        ledger.verify()?;
        ledger.db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        Ok(ledger)
    }

    fn lock_existing(database: &Path, witness: &Path, generation: G) -> Result<Self, Rejected> {
        let lock = lock_ledger(witness)?;
        let db = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|_| hold("Allocator ledger is missing or unavailable. Reconcile offline."))?;
        db.busy_timeout(Duration::from_secs(5))?;
        Ok(Self {
            db,
            generation,
            witness: witness.to_owned(),
            lock,
        })
    }

    pub fn enroll(
        database: &Path,
        witness: &Path,
        generation: G,
        slots: &[(&str, &str)],
    ) -> Result<Self, Rejected> {
        Self::enroll_for_handoff(database, witness, generation, slots, None)
    }

    pub fn enroll_for_handoff(
        database: &Path,
        witness: &Path,
        mut generation: G,
        slots: &[(&str, &str)],
        public_key: Option<&str>,
    ) -> Result<Self, Rejected> {
        if database == witness || database == Path::new(":memory:") || slots.is_empty() {
            return Err(hold(
                "Enrollment requires a durable database and an independent witness.",
            ));
        }
        if let Some(key) = public_key {
            handoff_key(key)?;
        }
        let lock = lock_ledger(witness)?;
        if witness.symlink_metadata().is_ok() {
            return Err(hold(
                "A ledger or witness is already enrolled. Reconcile offline.",
            ));
        }
        let mut db = Connection::open(database)?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if ledger_schema::exists(&tx, "ledger_revision")? {
            return Err(hold(
                "A ledger or witness is already enrolled. Reconcile offline.",
            ));
        }
        for (table, filter) in [
            ("lease", "phase!='released'"),
            ("capacity_reservation", "released_at IS NULL"),
        ] {
            if ledger_schema::exists(&tx, table)?
                && tx
                    .query_row(
                        &format!("SELECT 1 FROM {table} WHERE {filter} LIMIT 1"),
                        [],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some()
            {
                return Err(hold("Reconcile active ownership before enrollment."));
            }
        }
        ledger_schema::construct(&tx)?;
        for (id, identity) in slots {
            tx.execute("INSERT INTO slot_binding VALUES (?,?)", [id, identity])?;
        }
        let revision = uuid::Uuid::new_v4().simple().to_string();
        let controller_generation = generation()?;
        tx.execute_batch("CREATE TABLE ledger_revision (id INTEGER PRIMARY KEY CHECK(id=1),revision TEXT NOT NULL);")?;
        tx.execute("INSERT INTO ledger_revision VALUES (1,?)", [&revision])?;
        write_witness(
            witness,
            Witness {
                revision,
                controller_generation,
                handoff_public_key: public_key.map(str::to_owned),
            },
        )?;
        tx.commit()?;
        Ok(Self {
            db,
            generation,
            witness: witness.to_owned(),
            lock,
        })
    }

    pub fn migrate(&mut self) -> Result<(), Rejected> {
        if ledger_schema::needs_migration(&self.db)? {
            self.mutate(ledger_schema::construct)?;
        }
        Ok(())
    }

    pub fn connection(&self) -> &Connection {
        &self.db
    }

    pub fn verify(&mut self) -> Result<String, Rejected> {
        Ok(validate(&self.db, &self.witness, &mut self.generation)?.controller_generation)
    }

    pub fn enroll_handoff_authority(
        &mut self,
        public_key: &str,
        slots: &[(&str, &str)],
    ) -> Result<(), Rejected> {
        let key = handoff_key(public_key)?;
        let tx = self.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut seal = validate(&tx, &self.witness, &mut self.generation)?;
        verify_slots(&tx, slots)?;
        if let Some(existing) = seal.handoff_public_key.as_deref() {
            return if handoff_key(existing)? == key {
                Ok(())
            } else {
                Err(hold("The ledger already has an enrolled handoff authority."))
            };
        }
        if (self.generation)()? != seal.controller_generation {
            return Err(handoff_denied());
        }
        seal.handoff_public_key = Some(STANDARD.encode(key));
        commit_revision(tx, &self.witness, seal)
    }

    pub fn handoff(
        database: &Path,
        witness: &Path,
        generation: G,
        slots: &[(&str, &str)],
        authorization: &Value,
    ) -> Result<Self, Rejected> {
        let authorization: HandoffAuthorization = serde_json::from_value(authorization.clone())
            .map_err(|_| handoff_denied())?;
        let mut ledger = Self::lock_existing(database, witness, generation)?;
        ledger.db.execute_batch("PRAGMA synchronous=FULL;")?;
        let tx = ledger.db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut seal = read_seal(&tx, witness)?;
        verify_slots(&tx, slots)?;
        let replacement = (ledger.generation)()?;
        authorization.verify(&seal, &replacement)?;
        seal.controller_generation = replacement;
        commit_revision(tx, witness, seal)?;
        Ok(ledger)
    }

    pub fn mutate<T>(
        &mut self,
        write: impl FnOnce(&Transaction<'_>) -> Result<T, Rejected>,
    ) -> Result<T, Rejected> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let started = validate(&tx, &self.witness, &mut self.generation)?;
        let result = write(&tx)?;
        let latest = validate(&tx, &self.witness, &mut self.generation)?;
        if latest.controller_generation != started.controller_generation {
            return Err(hold("Controller VM generation changed. Reconcile offline."));
        }
        commit_revision(tx, &self.witness, latest)?;
        Ok(result)
    }
}

fn lock_ledger(witness: &Path) -> Result<File, Rejected> {
    let mut lock_name = witness.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(Path::new(&lock_name))
        .map_err(|_| hold("Independent ledger lock is unavailable."))?;
    lock.try_lock()
        .map_err(|_| hold("Another allocator owns the independent ledger lock."))?;
    Ok(lock)
}

fn validate(
    db: &Connection,
    witness: &Path,
    generation: &mut impl FnMut() -> Result<String, Rejected>,
) -> Result<Witness, Rejected> {
    let seal = read_seal(db, witness)?;
    if generation()? != seal.controller_generation {
        return Err(hold("Controller VM generation differs. Reconcile offline."));
    }
    Ok(seal)
}

fn read_seal(db: &Connection, witness: &Path) -> Result<Witness, Rejected> {
    let seal: Witness = fs::read(witness)
        .ok()
        .and_then(|value| serde_json::from_slice(&value).ok())
        .ok_or_else(|| hold("Missing or invalid independent ledger witness. Reconcile offline."))?;
    let stored: Option<String> = db
        .query_row(
            "SELECT revision FROM ledger_revision WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| hold("Ledger revision could not be verified. Reconcile offline."))?;
    if stored.as_deref() != Some(seal.revision.as_str()) {
        return Err(hold("Ledger revision differs. Reconcile offline."));
    }
    Ok(seal)
}

fn verify_slots(db: &Connection, slots: &[(&str, &str)]) -> Result<(), Rejected> {
    let mut select = db.prepare("SELECT id,identity FROM slot_binding ORDER BY id")?;
    let enrolled = select.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?.collect::<Result<Vec<_>, _>>()?;
    let mut expected = slots.to_vec();
    expected.sort_unstable();
    if enrolled.iter().map(|(id, identity)| (id.as_str(), identity.as_str())).eq(expected) {
        Ok(())
    } else {
        Err(handoff_denied())
    }
}

fn commit_revision(tx: Transaction<'_>, witness: &Path, mut seal: Witness) -> Result<(), Rejected> {
    seal.revision = uuid::Uuid::new_v4().simple().to_string();
    tx.execute("UPDATE ledger_revision SET revision=? WHERE id=1", [&seal.revision])?;
    // An interruption between the witness write and SQLite commit holds the
    // ledger for reconciliation.
    write_witness(witness, seal)?;
    tx.commit()?;
    Ok(())
}

fn write_witness(location: &Path, witness: Witness) -> Result<(), Rejected> {
    let fail = || hold("The independent ledger witness could not be saved. Reconcile offline.");
    let dir = location
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let source = serde_json::to_vec(&witness).map_err(|_| fail())?;
    let mut temporary = tempfile::NamedTempFile::new_in(dir).map_err(|_| fail())?;
    temporary.write_all(&source).map_err(|_| fail())?;
    temporary.as_file().sync_all().map_err(|_| fail())?;
    temporary.persist(location).map_err(|_| fail())?;
    File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| fail())?;
    Ok(())
}
