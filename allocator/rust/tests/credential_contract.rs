use lab_allocator::{Ledger, TaskCredentials};
use rusqlite::Connection;
use serde_json::json;

#[test]
fn task_reissue_revokes_the_prior_token_and_explicit_revocation_clears_renewal_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let database = tmp.path().join("ledger.db");
    let witness = tmp.path().join("witness.json");
    let db = Connection::open(&database).unwrap();
    db.execute_batch("
       CREATE TABLE ledger_revision (id INTEGER PRIMARY KEY CHECK(id=1),revision TEXT NOT NULL);
       INSERT INTO ledger_revision VALUES (1,'fixture');
       CREATE TABLE task_credential (owner TEXT PRIMARY KEY,token_hash TEXT UNIQUE NOT NULL,expires_at REAL NOT NULL,revoked_at REAL);
       CREATE TABLE lease (id TEXT PRIMARY KEY,owner TEXT NOT NULL);
       CREATE TABLE renewal_delegate (lease_id TEXT PRIMARY KEY,token_hash TEXT NOT NULL,valid_until REAL NOT NULL,extension INTEGER NOT NULL);
       INSERT INTO lease VALUES ('first','issued:runner:task');
       INSERT INTO renewal_delegate VALUES ('first','test',180.0,20);
    ").unwrap();
    drop(db);
    std::fs::write(
        &witness,
        r#"{"revision":"fixture","controller_generation":"test"}"#,
    )
    .unwrap();
    let mut ledger = Ledger::open(&database, &witness, || Ok("test".into())).unwrap();
    let mut credentials = TaskCredentials::new(&mut ledger, 100).unwrap();
    let request = json!({"task_ref":"task","seconds":50});
    let first = credentials.issue("runner", &request, 100.0).unwrap();
    let old = first["token"].as_str().unwrap();
    assert_eq!(
        credentials.owner(old, 105.0).unwrap().as_deref(),
        Some("issued:runner:task")
    );
    assert!(credentials.owner(old, 150.0).unwrap().is_none());
    let second = credentials.issue("runner", &request, 120.0).unwrap();
    let current = second["token"].as_str().unwrap();
    assert_ne!(old, current);
    assert!(credentials.owner(old, 121.0).unwrap().is_none());
    assert!(credentials.owner(current, 121.0).unwrap().is_some());
    assert_eq!(
        credentials
            .issue("runner", &json!({"task_ref":"task","revoke":true}), 130.0)
            .unwrap()["revoked"],
        true
    );
    assert!(credentials.owner(current, 131.0).unwrap().is_none());
    let count = ledger
        .connection()
        .query_row("SELECT COUNT(*) FROM renewal_delegate", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(count, 0);
}
