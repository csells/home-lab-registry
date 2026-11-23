use std::{collections::BTreeMap, error::Error, fs::File, io, path::Path};

use rusqlite::params;
use serde::Deserialize;
use serde_json::Value;

use crate::{Catalog, client::valid_id};

#[derive(Deserialize)]
struct Export {
    services: Vec<Record>,
    history: BTreeMap<String, Vec<Revision>>,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    revision: i64,
    updated_at: f64,
    spec: Value,
}

#[derive(Deserialize)]
struct Revision {
    revision: i64,
    document: String,
    actor: String,
    updated: f64,
}

pub fn restore_export(source: &Path, destination: &Path) -> Result<usize, Box<dyn Error>> {
    if destination.symlink_metadata().is_ok() {
        return Err("Destination already exists; restore to a new database".into());
    }
    let export: Export = serde_json::from_reader(File::open(source)?)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let pending = tempfile::NamedTempFile::new_in(parent)?;
    let catalog = Catalog::open(pending.path())?;
    {
        let mut connection = catalog.connect()?;
        let transaction = connection.transaction()?;
        for record in &export.services {
            if !valid_id(&record.id)
                || record.revision <= 0
                || !record.updated_at.is_finite()
                || !record.spec.is_object()
            {
                return Err("Invalid registry record in backup export".into());
            }
            let history = export
                .history
                .get(&record.id)
                .ok_or("Registry export is missing record history")?;
            transaction.execute(
                "INSERT INTO services VALUES (?1,?2,?3,?4)",
                params![
                    record.id,
                    record.revision,
                    record.spec.to_string(),
                    record.updated_at
                ],
            )?;
            for entry in history {
                let _: Value = serde_json::from_str(&entry.document)?;
                transaction.execute("INSERT INTO history(id,revision,document,actor,updated) VALUES(?1,?2,?3,?4,?5)",
                    params![record.id,entry.revision,entry.document,entry.actor,entry.updated])?;
            }
        }
        transaction.commit()?;
        let checked: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if checked != "ok" {
            return Err("The restored catalog failed its database integrity check".into());
        }
        // Merge the staging WAL and close the database before publishing a single
        // standalone file. Observations will be collected by the restarted registry.
        let mode: String =
            connection.query_row("PRAGMA journal_mode=DELETE", [], |row| row.get(0))?;
        if mode.to_ascii_lowercase() != "delete" {
            return Err("Could not checkpoint the restored catalog".into());
        }
    }
    pending.as_file().sync_all()?;
    pending.persist_noclobber(destination).map_err(|failure| {
        if failure.error.kind() == io::ErrorKind::AlreadyExists {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Destination already exists; restore to a new database",
            )
        } else {
            failure.error
        }
    })?;
    File::open(parent)?.sync_all()?;
    Ok(export.services.len())
}
