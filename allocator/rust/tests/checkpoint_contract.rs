use lab_allocator::{CheckpointStore, Ledger};
use rusqlite::Connection;
use serde_json::json;

fn fixture() -> (
    tempfile::TempDir,
    Ledger<impl FnMut() -> Result<String, lab_allocator::Rejected>>,
) {
    let tmp = tempfile::tempdir().unwrap();
    let database = tmp.path().join("ledger.db");
    let witness = tmp.path().join("witness.json");
    let db = Connection::open(&database).unwrap();
    db.execute_batch("
       CREATE TABLE ledger_revision (id INTEGER PRIMARY KEY CHECK(id=1),revision TEXT NOT NULL);
       INSERT INTO ledger_revision VALUES (1,'fixture');
       CREATE TABLE lease (id TEXT PRIMARY KEY, owner TEXT NOT NULL, phase TEXT NOT NULL, reclaim_on_expiry INTEGER NOT NULL);
       INSERT INTO lease VALUES ('lease-id','owner','ready',1);
       CREATE TABLE incremental_checkpoint (
           id TEXT PRIMARY KEY, lease_id TEXT NOT NULL, request_key TEXT NOT NULL,
           created_at REAL NOT NULL, size INTEGER NOT NULL, sha256 TEXT NOT NULL,
           token_hash TEXT, upload_expires REAL, redeemed INTEGER NOT NULL DEFAULT 0,
           receipt TEXT, error TEXT, UNIQUE(lease_id,request_key)
       );
    ").unwrap();
    drop(db);
    std::fs::write(
        &witness,
        r#"{"revision":"fixture","controller_generation":"test"}"#,
    )
    .unwrap();
    let ledger = Ledger::open(&database, &witness, || Ok("test".into())).unwrap();
    (tmp, ledger)
}

#[test]
fn checkpoint_tickets_bind_content_owner_and_exact_storage_acknowledgment() {
    let (_tmp, mut ledger) = fixture();
    let mut store = CheckpointStore::new(&mut ledger);
    let hash = "e".repeat(64);
    let snapshot = "d".repeat(64);
    let request = json!({"request_key":"task-file","sha256":hash,"size":268_435_456});
    let over_limit = json!({"request_key":"oversized","sha256":hash,"size":536_870_913});
    assert_eq!(
        store
            .grant(
                "lease-id",
                "owner",
                &over_limit,
                Some("https://recovery.example.invalid/"),
                99.0,
            )
            .unwrap_err()
            .code(),
        "invalid_checkpoint"
    );
    let denied = store
        .grant(
            "lease-id",
            "another-owner",
            &request,
            Some("https://recovery.example.invalid/"),
            100.0,
        )
        .unwrap_err();
    assert_eq!(denied.code(), "unknown_lease");
    let grant = store
        .grant(
            "lease-id",
            "owner",
            &request,
            Some("https://recovery.example.invalid/"),
            100.0,
        )
        .unwrap();
    let ticket = grant["ticket"].as_str().unwrap();
    assert_eq!(ticket.len(), 43);
    assert_eq!(
        store
            .grant(
                "lease-id",
                "owner",
                &request,
                Some("https://recovery.example.invalid/"),
                101.0
            )
            .unwrap_err()
            .code(),
        "checkpoint_in_progress"
    );
    let response = store.redeem(ticket, 102.0).unwrap();
    assert_eq!(response["sha256"], hash);
    assert_eq!(response["size"], 268_435_456);
    assert_eq!(
        store.redeem(ticket, 103.0).unwrap_err().code(),
        "ticket_unavailable"
    );
    assert_eq!(
        store
            .complete(
                &json!({"ticket":ticket,"snapshot":snapshot,"sha256":"bad"}),
                Some(250.0),
                104.0
            )
            .unwrap_err()
            .code(),
        "invalid_checkpoint_result"
    );
    let saved = store
        .complete(
            &json!({"ticket":ticket,"snapshot":snapshot,"sha256":hash}),
            Some(250.0),
            105.0,
        )
        .unwrap();
    assert_eq!(saved["snapshot"], snapshot);
    assert_eq!(saved["sha256"], hash);
    assert_eq!(saved["storage_managed"], true);
    assert_eq!(saved["retain_through"], 250.0);
    assert_eq!(
        store
            .grant(
                "lease-id",
                "owner",
                &request,
                Some("https://recovery.example.invalid/"),
                107.0
            )
            .unwrap()["checkpoint"],
        saved
    );
    let rows = store.list("lease-id").unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert!(!rows.to_string().contains(ticket));
}
