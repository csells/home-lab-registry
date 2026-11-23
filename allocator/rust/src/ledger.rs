use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::Rejected;

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
}

fn hold(reason: &str) -> Rejected {
    Rejected::new("ledger_reconciliation_required", reason, 409)
}

impl<G: FnMut() -> Result<String, Rejected>> Ledger<G> {
    pub fn open(database: &Path, witness: &Path, generation: G) -> Result<Self, Rejected> {
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
        let db = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|_| hold("Allocator ledger is missing or unavailable. Reconcile offline."))?;
        db.busy_timeout(Duration::from_secs(5))?;
        let mut ledger = Self {
            db,
            generation,
            witness: witness.to_owned(),
            lock,
        };
        ledger.verify()?;
        ledger
            .db
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
        Ok(ledger)
    }

    pub fn connection(&self) -> &Connection {
        &self.db
    }

    pub fn verify(&mut self) -> Result<String, Rejected> {
        validate(&self.db, &self.witness, &mut self.generation)
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
        if latest != started {
            return Err(hold("Controller VM generation changed. Reconcile offline."));
        }
        let revision = uuid::Uuid::new_v4().simple().to_string();
        tx.execute(
            "UPDATE ledger_revision SET revision=? WHERE id=1",
            [&revision],
        )?;
        // A crash after the witness is durable and before SQLite commits leaves a
        // mismatch. Both implementations hold the ledger for reconciliation.
        write_witness(
            &self.witness,
            Witness {
                revision,
                controller_generation: latest,
            },
        )?;
        tx.commit()?;
        Ok(result)
    }
}

fn validate(
    db: &Connection,
    witness: &Path,
    generation: &mut impl FnMut() -> Result<String, Rejected>,
) -> Result<String, Rejected> {
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
    if stored.as_deref() != Some(seal.revision.as_str())
        || generation()? != seal.controller_generation
    {
        return Err(hold(
            "Ledger revision or controller VM generation differs. Reconcile offline.",
        ));
    }
    Ok(seal.controller_generation)
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
