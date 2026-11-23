use std::{
    path::{Path, PathBuf},
    process::Command,
};

use lab_allocator::Ledger;
use rusqlite::Connection;

fn legacy(code: &str, paths: &[&Path]) {
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let result = Command::new("python3")
        .current_dir(cwd)
        .arg("-c")
        .arg(code)
        .args(paths)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "Legacy ledger: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn legacy_and_native_allocate_revisions_share_the_independent_witness() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("allocator.db");
    let witness = temp.path().join("authority.json");
    let backup = temp.path().join("older.db");
    legacy(
        r#"
import sys, sqlite3
from ledger import Ledger
conn=sqlite3.connect(sys.argv[1],factory=Ledger)
conn.attach(sys.argv[2],lambda:'provider-generation',enroll=True)
with conn:
    conn.execute('CREATE TABLE fixture (hold INTEGER)')
    conn.execute('INSERT INTO fixture VALUES (0)')
conn.close()
"#,
        &[&database, &witness],
    );
    let generation = || Ok("provider-generation".into());
    let mut ledger = Ledger::open(&database, &witness, generation).unwrap();
    assert!(
        Ledger::open(&database, &witness, generation).is_err(),
        "single owner"
    );
    ledger
        .connection()
        .backup("main", &backup, rusqlite::backup::NO_PROGRESS)
        .unwrap();
    ledger
        .mutate(|tx| {
            tx.execute("UPDATE fixture SET hold=1", []).unwrap();
            Ok(())
        })
        .unwrap();
    drop(ledger);
    legacy(
        r#"
import sys, sqlite3
from ledger import Ledger
conn=sqlite3.connect(sys.argv[1],factory=Ledger)
conn.attach(sys.argv[2],lambda:'provider-generation')
assert conn.execute('SELECT hold FROM fixture').fetchone()==(1,)
conn.close()
"#,
        &[&database, &witness],
    );
    assert!(
        matches!(Ledger::open(&backup,&witness,generation), Err(error) if error.code()=="ledger_reconciliation_required")
    );
    assert!(
        matches!(Ledger::open(&database,&witness,|| Ok("restored-vm".into())), Err(error) if error.code()=="ledger_reconciliation_required")
    );
    assert_eq!(
        Connection::open(database)
            .unwrap()
            .query_row("SELECT hold FROM fixture", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn a_failed_mutation_does_not_move_the_witness_and_unknown_operation_holds_are_durable() {
    use lab_allocator::{LeaseJournal, Rejected};
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("allocator.db");
    let witness = temp.path().join("authority.json");
    legacy(
        r#"
import sys, sqlite3
from ledger import Ledger
conn=sqlite3.connect(sys.argv[1],factory=Ledger)
conn.attach(sys.argv[2],lambda:'provider-generation',enroll=True)
with conn:
    conn.execute("CREATE TABLE lease(id TEXT PRIMARY KEY, owner TEXT NOT NULL, phase TEXT NOT NULL, upstream TEXT, prior_tasks TEXT, address TEXT, generation TEXT)")
    conn.execute("INSERT INTO lease VALUES ('owned','task-owner','ready',NULL,NULL,'192.0.2.20','guest-generation')")
conn.close()
"#,
        &[&database, &witness],
    );
    let generation = || Ok("provider-generation".into());
    let mut ledger = Ledger::open(&database, &witness, generation).unwrap();
    let mut journal = LeaseJournal::new(&mut ledger);
    let before = std::fs::read(&witness).unwrap();
    let denied = journal
        .record_intent(
            "owned",
            "other",
            "ready",
            "stopping",
            "guest-generation",
            &["existing".into()],
        )
        .unwrap_err();
    assert_eq!(denied.code(), "unknown_lease");
    assert_eq!(std::fs::read(&witness).unwrap(), before);
    let intent = journal
        .record_intent(
            "owned",
            "task-owner",
            "ready",
            "stopping",
            "guest-generation",
            &["existing".into()],
        )
        .unwrap();
    assert_eq!(intent.phase, "stopping");
    assert!(intent.upstream.is_none());
    assert!(
        journal
            .record_intent(
                "owned",
                "task-owner",
                "ready",
                "stopping",
                "guest-generation",
                &[]
            )
            .is_err()
    );
    drop(ledger);
    let mut reopened = Ledger::open(&database, &witness, generation).unwrap();
    let mut journal = LeaseJournal::new(&mut reopened);
    assert!(
        journal
            .recover_task(
                "owned",
                "task-owner",
                "stopping",
                "guest-generation",
                &["existing".into()]
            )
            .unwrap()
            .is_none()
    );
    assert!(
        journal
            .recover_task(
                "owned",
                "task-owner",
                "stopping",
                "guest-generation",
                &["new".into(), "other".into()]
            )
            .unwrap()
            .is_none()
    );
    assert!(
        journal
            .recover_task(
                "owned",
                "task-owner",
                "stopping",
                "restored-guest",
                &["new".into()]
            )
            .is_err()
    );
    let recovered = journal
        .recover_task(
            "owned",
            "task-owner",
            "stopping",
            "guest-generation",
            &["new".into(), "existing".into()],
        )
        .unwrap()
        .unwrap();
    assert_eq!(recovered, "new");

    let result = reopened.mutate(|tx| {
        tx.execute("UPDATE lease SET phase='released' WHERE id='owned'", [])
            .unwrap();
        Err::<(), _>(Rejected::new(
            "injected",
            "Deliberately failed transaction",
            409,
        ))
    });
    assert!(result.is_err());
    assert_eq!(
        LeaseJournal::new(&mut reopened)
            .get("owned", "task-owner")
            .unwrap()
            .phase,
        "stopping"
    );
}
