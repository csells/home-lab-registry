use std::collections::HashMap;

use rusqlite::{Connection, Row, params};
use serde_json::{Map, Value, json};

use crate::Rejected;

#[derive(Clone)]
pub struct GuestScope {
    pub node: String,
    pub guest_id: String,
}

pub fn lease_inventory(
    db: &Connection,
    scopes: &HashMap<String, GuestScope>,
) -> Result<Value, Rejected> {
    let mut statement = db.prepare("SELECT id,slot_id FROM lease WHERE phase!='released'")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut guests = Vec::new();
    for row in rows {
        let (lease, slot) = row?;
        if let Some(scope) = scopes.get(&slot) {
            guests.push(json!({"node":scope.node,"id":scope.guest_id,"name":format!("lease-{lease}"),"type":"qemu","lease":lease}));
        }
    }
    Ok(json!({"guests":guests}))
}

pub fn lease_review(
    db: &mut Connection,
    before: Option<&str>,
    scope: &str,
    now: f64,
) -> Result<Value, Rejected> {
    let restriction = match scope {
        "all" | "counts" | "issues" => "1",
        "current" => "phase!='released'",
        "released" => "phase='released'",
        _ => return Err(Rejected::new("invalid_scope", "Unknown lease scope.", 400)),
    };
    let before = before
        .map(|value| {
            value
                .parse::<i64>()
                .ok()
                .filter(|number| {
                    *number > 0
                        && value.len() <= 18
                        && value.bytes().all(|byte| byte.is_ascii_digit())
                })
                .ok_or_else(|| {
                    Rejected::new("invalid_cursor", "Invalid lease history cursor.", 400)
                })
        })
        .transpose()?;
    let tx = db.transaction()?;
    let (active,released) = tx.query_row(
        "SELECT COUNT(*) FILTER (WHERE phase!='released'), COUNT(*) FILTER (WHERE phase='released') FROM lease",
        [], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?)),
    )?;
    let mut result = json!({"counts":{"current":active,"released":released},"as_of":now});
    if matches!(scope, "counts" | "issues") {
        if before.is_some() {
            return Err(Rejected::new(
                "invalid_cursor",
                "Summary cannot be paginated.",
                400,
            ));
        }
        if scope == "issues" {
            let (issues, deadline) = issues(&tx, now)?;
            result["issues"] = json!(issues);
            result["next_deadline"] = json!(deadline);
        }
        tx.commit()?;
        return Ok(result);
    }
    let (records, next) = page(&tx, before, restriction, now)?;
    result["leases"] = json!(records);
    result["next_before"] = json!(next);
    tx.commit()?;
    Ok(result)
}

fn issues(db: &Connection, now: f64) -> Result<(Vec<Value>, Option<f64>), Rejected> {
    let mut query=db.prepare(
        "SELECT task,deadline,phase,export_error,hold_mode,reviewer_hold FROM lease
         WHERE phase IN ('ready','awaiting-export') OR (phase!='released' AND export_error IS NOT NULL) ORDER BY rowid"
    )?;
    let mut rows = query.query([])?;
    let mut result = Vec::new();
    let mut next = None;
    while let Some(row) = rows.next()? {
        let task: Option<String> = row.get(0)?;
        let phase: String = row.get(2)?;
        let error: Option<String> = row.get(3)?;
        let held = [
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
        ]
        .into_iter()
        .flatten()
        .any(|hold| !hold.is_empty());
        if error.is_some_and(|error| !error.is_empty()) {
            result.push(json!({"task":task,"code":"export-failed"}));
        } else if phase == "ready"
            && !held
            && let Some(deadline) = row.get::<_, Option<f64>>(1)?
        {
            if deadline <= now {
                result.push(json!({"task":task,"code":"overdue"}));
            } else {
                next = Some(next.map_or(deadline, |earliest: f64| earliest.min(deadline)));
            }
        }
    }
    Ok((result, next))
}

const PAGE_SIZE: usize = 25;

fn page(
    db: &Connection,
    before: Option<i64>,
    restriction: &str,
    now: f64,
) -> Result<(Vec<Value>, Option<i64>), Rejected> {
    let mut query = db.prepare(&format!(
        "SELECT rowid,id,task,owner,phase,deadline,released_at,hold_mode,hold_reason,reviewer,
                reviewer_hold,reviewer_reason,export_error,checkpoint
         FROM lease WHERE {restriction} AND (? IS NULL OR rowid<?)
         ORDER BY rowid DESC LIMIT {}",
        PAGE_SIZE + 1
    ))?;
    let mut rows = query.query(params![before, before])?;
    let mut records = Vec::new();
    let mut cursor = None;
    while let Some(row) = rows.next()? {
        if records.len() == PAGE_SIZE {
            return Ok((records, cursor));
        }
        cursor = Some(row.get(0)?);
        records.push(review_row(db, row, now)?);
    }
    Ok((records, None))
}

fn review_row(db: &Connection, row: &Row<'_>, now: f64) -> Result<Value, Rejected> {
    let mut record = Map::new();
    for (name, index) in [
        ("id", 1),
        ("task", 2),
        ("owner", 3),
        ("phase", 4),
        ("hold_mode", 7),
        ("hold_reason", 8),
        ("reviewer", 9),
        ("reviewer_hold", 10),
        ("reviewer_reason", 11),
        ("export_error", 12),
    ] {
        record.insert(name.to_owned(), json!(row.get::<_, Option<String>>(index)?));
    }
    let deadline = row.get::<_, Option<f64>>(5)?;
    let mut phase = record["phase"].as_str().unwrap_or_default().to_owned();
    if phase == "ready" && deadline.is_some_and(|deadline| deadline <= now) {
        phase = "overdue".into();
        record.insert("phase".into(), json!(phase));
    }
    record.insert("deadline".into(), json!(deadline));
    record.insert("released_at".into(), json!(row.get::<_, Option<f64>>(6)?));
    let id = record["id"].as_str().ok_or_else(invalid_receipt)?;
    let mut stored = db.prepare(
        "SELECT id,receipt FROM incremental_checkpoint WHERE lease_id=? AND receipt IS NOT NULL",
    )?;
    let receipts = stored.query_map([id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut choices = Vec::new();
    for receipt in receipts {
        let (key, body) = receipt?;
        let receipt = serde_json::from_str::<Value>(&body).map_err(|_| invalid_receipt())?;
        if recoverable(db, id, Some(&key), &phase, now, &receipt)? {
            choices.push(receipt);
        }
    }
    if let Some(body) = row.get::<_, Option<String>>(13)? {
        let receipt = serde_json::from_str::<Value>(&body).map_err(|_| invalid_receipt())?;
        if recoverable(db, id, None, &phase, now, &receipt)? {
            choices.push(receipt);
        }
    }
    let mut last = None;
    for receipt in &choices {
        let stamp = receipt["verified_at"]
            .as_f64()
            .ok_or_else(invalid_receipt)?;
        if last.is_none_or(|(previous, _)| stamp > previous) {
            last = Some((stamp, receipt));
        }
    }
    let last = last.map_or(&Value::Null, |(_, receipt)| receipt);
    record.insert("checkpoint_count".into(), json!(choices.len()));
    record.insert("last_verified_at".into(), last["verified_at"].clone());
    record.insert("retained_through".into(), last["retain_through"].clone());
    record.insert("pinned".into(), json!(last["pin"].is_object()));
    Ok(Value::Object(record))
}

fn invalid_receipt() -> Rejected {
    Rejected::new(
        "ledger_unavailable",
        "The allocator could not read retained checkpoint metadata.",
        503,
    )
}

pub(crate) fn recoverable(
    db: &Connection,
    lease: &str,
    checkpoint: Option<&str>,
    phase: &str,
    now: f64,
    receipt: &Value,
) -> Result<bool, Rejected> {
    if receipt["storage_managed"] != true
        || (checkpoint.is_none() && phase != "released")
        || !receipt["purge_claim"].is_null()
        || !receipt["purged_at"].is_null()
    {
        return Ok(false);
    }
    let available = receipt["pin"].is_object()
        || receipt["retain_through"]
            .as_f64()
            .is_none_or(|until| until > now);
    if available {
        return Ok(true);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM restore_hold WHERE source=? AND checkpoint_id IS ?)",
        params![lease, checkpoint],
        |row| row.get(0),
    )?)
}
