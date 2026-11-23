use rusqlite::{Connection, OptionalExtension, Row, params, types::ValueRef};
use serde_json::{Map, Value, json};

use crate::{LeasePhase, Rejected, review::recoverable, tokens::hex};

pub(crate) fn sqlite_json(row: &Row<'_>) -> rusqlite::Result<Value> {
    let mut result = Map::new();
    let statement = row.as_ref();
    for column in 0..statement.column_count() {
        let name = statement.column_name(column)?;
        let value = match row.get_ref(column)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(value) => json!(value),
            ValueRef::Real(value) => json!(value),
            ValueRef::Text(value) => {
                let value = std::str::from_utf8(value)
                    .map_err(|error| rusqlite::Error::Utf8Error(column, error))?;
                json!(value)
            }
            ValueRef::Blob(_) => {
                return Err(rusqlite::Error::InvalidColumnType(
                    column,
                    name.into(),
                    rusqlite::types::Type::Blob,
                ));
            }
        };
        result.insert(name.into(), value);
    }
    Ok(Value::Object(result))
}

pub(crate) fn stored(value: &Value) -> Result<Value, Rejected> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let raw = value.as_str().ok_or_else(invalid_data)?;
    serde_json::from_str(raw).map_err(|_| invalid_data())
}

pub(crate) fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str, Rejected> {
    row[key].as_str().ok_or_else(invalid_data)
}

pub(crate) fn invalid_data() -> Rejected {
    Rejected::new(
        "ledger_reconciliation_required",
        "The lease ledger contains an invalid record.",
        409,
    )
}

pub(crate) fn no_lease() -> Rejected {
    Rejected::new("unknown_lease", "No lease has this identifier.", 404)
}

pub(crate) fn lease(db: &Connection, id: &str, owner: Option<&str>) -> Result<Value, Rejected> {
    if !hex(id, 32) {
        return Err(no_lease());
    }
    db.query_row(
        "SELECT * FROM lease WHERE id=? AND (? IS NULL OR owner=?)",
        params![id, owner, owner],
        sqlite_json,
    )
    .optional()?
    .ok_or_else(no_lease)
}

pub(crate) fn phase(row: &Value) -> Result<LeasePhase, Rejected> {
    serde_json::from_value(row["phase"].clone()).map_err(|_| invalid_data())
}

pub(crate) fn selected_receipt(
    db: &Connection,
    row: &Value,
    checkpoint: Option<&str>,
) -> Result<Value, Rejected> {
    match checkpoint {
        None => stored(&row["checkpoint"]),
        Some(checkpoint) if hex(checkpoint, 32) => {
            let Some(raw) = db
                .query_row(
                    "SELECT receipt FROM incremental_checkpoint WHERE lease_id=? AND id=?",
                    params![text(row, "id")?, checkpoint],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()?
            else {
                return Err(no_checkpoint());
            };
            stored(&json!(raw))
        }
        _ => Err(Rejected::new(
            "invalid_checkpoint",
            "A checkpoint ID must be 32 lowercase hex characters.",
            400,
        )),
    }
}

pub(crate) fn no_checkpoint() -> Rejected {
    Rejected::new(
        "checkpoint_unavailable",
        "No retained checkpoint is available for this lease.",
        404,
    )
}

pub(crate) fn available_receipt(
    db: &Connection,
    row: &Value,
    checkpoint: Option<&str>,
    now: f64,
    ticket_held: bool,
) -> Result<Value, Rejected> {
    let receipt = selected_receipt(db, row, checkpoint)?;
    let id = text(row, "id")?;
    let phase = text(row, "phase")?;
    let available = recoverable(db, id, checkpoint, phase, now, &receipt)?;
    let held = ticket_held
        && receipt["storage_managed"] == true
        && (checkpoint.is_some() || phase == "released")
        && receipt["purge_claim"].is_null()
        && receipt["purged_at"].is_null();
    if available || held {
        Ok(receipt)
    } else {
        Err(no_checkpoint())
    }
}
