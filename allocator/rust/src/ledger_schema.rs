use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::Rejected;

const LEASE_COLUMNS: &[(&str, &str)] = &[
    ("slot_id", "TEXT NOT NULL DEFAULT 'primary'"),
    ("recipe", "TEXT NOT NULL DEFAULT 'debian13-amd64'"),
    ("dynamic_guest", "TEXT"),
    ("dynamic_host_key", "TEXT"),
    ("generation", "TEXT"),
    ("clean_rollback", "TEXT"),
    ("restore_completed_at", "REAL"),
    ("resume_point", "TEXT"),
    ("requirements", "TEXT NOT NULL DEFAULT '{}'"),
    ("prior_tasks", "TEXT"),
    ("owner", "TEXT NOT NULL DEFAULT 'operator'"),
    ("ssh_key", "TEXT"),
    ("reclaim_on_expiry", "INTEGER NOT NULL DEFAULT 0"),
    ("reviewer", "TEXT"),
    ("reviewer_hold", "TEXT"),
    ("reviewer_reason", "TEXT"),
    ("reviewer_override", "TEXT"),
    ("hold_mode", "TEXT"),
    ("hold_reason", "TEXT"),
    ("export_challenge", "TEXT"),
    ("storage_challenge", "TEXT"),
    ("export_start_tasks", "TEXT"),
    ("export_requested_at", "REAL"),
    ("export_last_attempt_at", "REAL"),
    ("export_failed_at", "REAL"),
    ("export_error", "TEXT"),
    ("export_disposition", "TEXT"),
    ("checkpoint", "TEXT"),
    ("released_at", "REAL"),
];

const TABLES: &str = "
  CREATE TABLE IF NOT EXISTS lease (
    id TEXT PRIMARY KEY, request_key TEXT UNIQUE NOT NULL, task TEXT NOT NULL,
    initial_seconds INTEGER NOT NULL, phase TEXT NOT NULL, deadline REAL NOT NULL,
    upstream TEXT, address TEXT
  );

  CREATE TABLE IF NOT EXISTS recovery_ticket (
    token_hash TEXT PRIMARY KEY, lease_id TEXT NOT NULL,
    expires_at REAL NOT NULL, redeemed INTEGER NOT NULL DEFAULT 0, checkpoint_id TEXT
  );
  CREATE TABLE IF NOT EXISTS renewal_delegate (
    lease_id TEXT PRIMARY KEY, token_hash TEXT NOT NULL,
    valid_until REAL NOT NULL, extension INTEGER NOT NULL
  );
  CREATE TABLE IF NOT EXISTS restore_hold (
    replacement TEXT PRIMARY KEY, source TEXT NOT NULL, checkpoint_id TEXT
  );
  CREATE TABLE IF NOT EXISTS export_alert (
    id TEXT PRIMARY KEY, challenge TEXT NOT NULL UNIQUE, lease_id TEXT NOT NULL,
    owner TEXT NOT NULL, failed_at REAL NOT NULL, delivered_at REAL
  );
  CREATE TABLE IF NOT EXISTS unacknowledged_expiry (
    challenge TEXT PRIMARY KEY, claim TEXT NOT NULL, purged_at REAL
  );
  CREATE TABLE IF NOT EXISTS incremental_checkpoint (
    id TEXT PRIMARY KEY, lease_id TEXT NOT NULL, request_key TEXT NOT NULL,
    created_at REAL NOT NULL, size INTEGER NOT NULL, sha256 TEXT NOT NULL,
    token_hash TEXT, upload_expires REAL, redeemed INTEGER NOT NULL DEFAULT 0,
    receipt TEXT, error TEXT, UNIQUE(lease_id, request_key)
  );
  CREATE TABLE IF NOT EXISTS task_credential (
    owner TEXT PRIMARY KEY, token_hash TEXT UNIQUE NOT NULL,
    expires_at REAL NOT NULL, revoked_at REAL
  );
  CREATE TABLE IF NOT EXISTS capacity_reservation (
    id TEXT PRIMARY KEY, owner TEXT NOT NULL, request_key TEXT NOT NULL,
    memory_mib INTEGER NOT NULL, storage_mib INTEGER NOT NULL, purpose TEXT NOT NULL,
    created_at REAL NOT NULL, released_at REAL, slot_id TEXT NOT NULL DEFAULT 'primary',
    UNIQUE(owner, request_key)
  );
  CREATE TABLE IF NOT EXISTS slot_binding (
    id TEXT PRIMARY KEY, identity TEXT NOT NULL
  );
";

fn columns(db: &Connection, table: &str) -> Result<BTreeSet<String>, Rejected> {
    let mut statement = db.prepare("SELECT name FROM pragma_table_info(?)")?;
    Ok(statement
        .query_map([table], |row| row.get(0))?
        .collect::<Result<_, _>>()?)
}

pub(crate) fn exists(db: &Connection, name: &str) -> Result<bool, Rejected> {
    Ok(db
        .query_row("SELECT 1 FROM sqlite_master WHERE name=?", [name], |_| {
            Ok(())
        })
        .optional()?
        .is_some())
}

fn add_columns(
    tx: &Transaction<'_>,
    table: &str,
    desired: &[(&str, &str)],
) -> Result<(), Rejected> {
    let observed = columns(tx, table)?;
    for (column, definition) in desired.iter().filter(|(name, _)| !observed.contains(*name)) {
        tx.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition};"
        ))?;
    }
    Ok(())
}

pub(crate) fn construct(tx: &Transaction<'_>) -> Result<(), Rejected> {
    tx.execute_batch(TABLES)?;
    add_columns(tx, "lease", LEASE_COLUMNS)?;
    add_columns(tx, "recovery_ticket", &[("checkpoint_id", "TEXT")])?;
    add_columns(
        tx,
        "capacity_reservation",
        &[("slot_id", "TEXT NOT NULL DEFAULT 'primary'")],
    )?;
    tx.execute_batch("
        DROP INDEX IF EXISTS one_reserved_slot;
        CREATE UNIQUE INDEX IF NOT EXISTS one_lease_per_slot ON lease (slot_id) WHERE phase!='released';
        UPDATE lease SET storage_challenge=export_challenge WHERE storage_challenge IS NULL AND export_challenge IS NOT NULL;
        INSERT OR IGNORE INTO restore_hold
            SELECT id, json_extract(resume_point,'$.lease'), json_extract(resume_point,'$.checkpoint_id')
            FROM lease WHERE phase!='released' AND resume_point IS NOT NULL AND restore_completed_at IS NULL;
    ")?;
    Ok(())
}

pub(crate) fn needs_migration(db: &Connection) -> Result<bool, Rejected> {
    for name in [
        "lease",
        "recovery_ticket",
        "renewal_delegate",
        "restore_hold",
        "export_alert",
        "unacknowledged_expiry",
        "incremental_checkpoint",
        "task_credential",
        "capacity_reservation",
        "slot_binding",
        "one_lease_per_slot",
    ] {
        if !exists(db, name)? {
            return Ok(true);
        }
    }
    let lease = columns(db, "lease")?;
    Ok(LEASE_COLUMNS.iter().any(|(name, _)| !lease.contains(*name))
        || !columns(db, "recovery_ticket")?.contains("checkpoint_id")
        || !columns(db, "capacity_reservation")?.contains("slot_id"))
}
