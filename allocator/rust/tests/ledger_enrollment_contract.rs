use lab_allocator::Ledger;
use rusqlite::Connection;

#[test]
fn enrollment_creates_the_complete_schema_and_refuses_a_second_genesis() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledger.sqlite");
    let authority = dir.path().join("witness.json");
    let generation = || Ok("qualified-controller".to_owned());
    let ledger = Ledger::enroll(&db, &authority, generation, &[("primary", "fixed")]).unwrap();
    let tables: i64 = ledger.connection().query_row(
        "SELECT count(*) FROM sqlite_master WHERE name IN ('lease','recovery_ticket','renewal_delegate','restore_hold',
        'export_alert','unacknowledged_expiry','incremental_checkpoint','task_credential','capacity_reservation','slot_binding',
        'ledger_revision','one_lease_per_slot')",[], |row| row.get(0)).unwrap();
    assert_eq!(tables, 12);
    let stored: String = ledger
        .connection()
        .query_row(
            "SELECT identity FROM slot_binding WHERE id='primary'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, "fixed");
    drop(ledger);
    assert!(Ledger::enroll(&db, &authority, generation, &[("primary", "fixed")]).is_err());
    assert!(Ledger::open(&db, &authority, generation).is_ok());
    assert!(Ledger::open(&db, &authority, || Ok("wrong".to_owned())).is_err());
}

#[test]
fn enrollment_never_seals_an_older_ledger_with_active_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ledger.sqlite");
    let authority = dir.path().join("witness.json");
    let existing = Connection::open(&db).unwrap();
    existing.execute_batch("CREATE TABLE lease(id TEXT PRIMARY KEY,phase TEXT); INSERT INTO lease VALUES('owned','ready');").unwrap();
    drop(existing);
    let denied = Ledger::enroll(
        &db,
        &authority,
        || Ok("qualified".to_owned()),
        &[("primary", "fixed")],
    );
    assert!(denied.is_err());
    assert!(!authority.exists());
}
