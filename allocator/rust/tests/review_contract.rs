use lab_allocator::lease_review;
use rusqlite::{Connection, params};
use serde_json::json;

fn catalog() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("
        CREATE TABLE lease(id TEXT, task TEXT, owner TEXT, phase TEXT, deadline REAL, released_at REAL,
                           hold_mode TEXT, hold_reason TEXT, reviewer TEXT, reviewer_hold TEXT,
                           reviewer_reason TEXT, export_error TEXT, checkpoint TEXT);
        CREATE TABLE incremental_checkpoint(id TEXT, lease_id TEXT, receipt TEXT);
        CREATE TABLE restore_hold(replacement TEXT, source TEXT, checkpoint_id TEXT);
    ").unwrap();
    db
}

#[test]
fn review_history_pages_separately_from_current_leases_and_excludes_private_fields() {
    let mut db = catalog();
    db.execute("INSERT INTO lease (id,phase,task,deadline,owner) VALUES ('open','ready','Task',90,'fixture')", []).unwrap();
    for sequence in 1..=30 {
        db.execute("INSERT INTO lease (id,phase,task,deadline,owner) VALUES (?,'released','History',50,'fixture')",
            [format!("released-{sequence:02}")]).unwrap();
    }
    let current = lease_review(&mut db, None, "current", 100.0).unwrap();
    assert_eq!(current["counts"], json!({"current":1,"released":30}));
    assert_eq!(current["leases"][0]["phase"], "overdue");
    assert_eq!(current["leases"].as_array().unwrap().len(), 1);
    let history = lease_review(&mut db, None, "released", 100.0).unwrap();
    let cursor = history["next_before"].as_i64().unwrap();
    let tail = lease_review(&mut db, Some(&cursor.to_string()), "released", 100.0).unwrap();
    assert_eq!(history["leases"].as_array().unwrap().len(), 25);
    assert_eq!(tail["leases"].as_array().unwrap().len(), 5);
    assert!(tail["next_before"].is_null());
    assert_eq!(
        lease_review(&mut db, Some("1"), "counts", 100.0)
            .unwrap_err()
            .code(),
        "invalid_cursor"
    );
    assert_eq!(
        lease_review(&mut db, None, "invented", 100.0)
            .unwrap_err()
            .code(),
        "invalid_scope"
    );
}

#[test]
fn recoverable_checkpoint_review_respects_holds_and_purge_claims_after_release() {
    let mut db = catalog();
    let retained = json!({"storage_managed":true,"verified_at":40,"retain_through":85,"pin":{"reason":"keep"}});
    db.execute("INSERT INTO lease (id,phase,task,deadline,owner,checkpoint) VALUES ('released','released','History',50,'fixture',?)",
        [retained.to_string()]).unwrap();
    let recoverable = json!({"storage_managed":true,"verified_at":60,"retain_through":90});
    let claimed = json!({"storage_managed":true,"verified_at":70,"retain_through":180,"purge_claim":"private"});
    for (key, receipt) in [("held", recoverable), ("purging", claimed)] {
        db.execute(
            "INSERT INTO incremental_checkpoint VALUES (?,?,?)",
            params![key, "released", receipt.to_string()],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO restore_hold VALUES ('next','released','held')",
        [],
    )
    .unwrap();
    let result = lease_review(&mut db, None, "released", 100.0).unwrap();
    let row = &result["leases"][0];
    assert_eq!(row["checkpoint_count"], 2);
    assert_eq!(row["last_verified_at"], 60);
    assert_eq!(row["retained_through"], 90);
    assert_eq!(row["pinned"], false);
    assert!(row.get("checkpoint").is_none());
    assert!(!result.to_string().contains("private"));
}
