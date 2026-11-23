use base64::{Engine, engine::general_purpose::STANDARD};
use lab_allocator::Ledger;
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use serde_json::{Value, json};

const OLD: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
const NEW: &str = "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff";
const SLOTS: &[(&str, &str)] = &[("primary", "pinned-provider")];

fn authorization(key: &Ed25519KeyPair, revision: &str, next: &str) -> Value {
    let statement = format!("home-lab-allocator-ledger-handoff-v1\n{revision}\n{OLD}\n{next}\n");
    json!({
        "revision": revision,
        "previous_controller_generation": OLD,
        "replacement_controller_generation": next,
        "signature": STANDARD.encode(key.sign(statement.as_bytes()).as_ref()),
    })
}

fn refused<G>(result: Result<Ledger<G>, lab_allocator::Rejected>) {
    assert_eq!(
        result.err().unwrap().code(),
        "ledger_reconciliation_required"
    );
}

#[test]
fn an_existing_controller_can_enroll_handoff_authority_once_without_resetting_its_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("state.db");
    let witness = dir.path().join("witness.json");
    let signing_key = Ed25519KeyPair::from_pkcs8(
        Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let key = STANDARD.encode(signing_key.public_key().as_ref());
    let old = || Ok(OLD.to_owned());
    let new = || Ok(NEW.to_owned());
    let historical = ("historical", "former-provider");
    let mut original = Ledger::enroll(&database, &witness, old, &[SLOTS[0], historical]).unwrap();
    original.mutate(|tx| {
        tx.execute("INSERT INTO lease(id,request_key,task,initial_seconds,phase,deadline,slot_id) VALUES('work','request','unfinished',60,'ready',9000,'primary')", [])?;
        tx.execute("INSERT INTO lease(id,request_key,task,initial_seconds,phase,deadline,slot_id) VALUES('past','other','historical work',60,'ready',9000,'historical')", [])?;
        Ok(())
    }).unwrap();
    let first: Value = serde_json::from_reader(std::fs::File::open(&witness).unwrap()).unwrap();
    assert!(first.get("handoff_public_key").is_none());
    assert_eq!(
        original
            .enroll_handoff_authority("invalid", SLOTS)
            .unwrap_err()
            .code(),
        "ledger_reconciliation_required"
    );
    assert!(original.enroll_handoff_authority(&key, SLOTS).is_err());
    original
        .mutate(|tx| {
            tx.execute("UPDATE lease SET phase='released' WHERE id='past'", [])?;
            Ok(())
        })
        .unwrap();
    original.enroll_handoff_authority(&key, SLOTS).unwrap();
    let enrolled = std::fs::read(&witness).unwrap();
    original.enroll_handoff_authority(&key, SLOTS).unwrap();
    assert_eq!(std::fs::read(&witness).unwrap(), enrolled);
    assert!(
        original
            .enroll_handoff_authority(&STANDARD.encode([5; 32]), SLOTS)
            .is_err()
    );
    drop(original);
    let seal: Value = serde_json::from_reader(std::fs::File::open(&witness).unwrap()).unwrap();
    assert_ne!(first["revision"], seal["revision"]);
    assert_eq!(seal["handoff_public_key"], key);
    let approved = authorization(&signing_key, seal["revision"].as_str().unwrap(), NEW);
    let replacement = Ledger::handoff(&database, &witness, new, SLOTS, &approved).unwrap();
    let task: String = replacement
        .connection()
        .query_row(
            "SELECT task FROM lease WHERE id='work' AND phase='ready'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task, "unfinished");
    assert_eq!(
        replacement
            .connection()
            .query_row(
                "SELECT identity FROM slot_binding WHERE id='historical'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        historical.1
    );
}

#[test]
fn fenced_replacement_preserves_the_current_ledger_and_rejects_stale_authority() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("state.db");
    let witness = dir.path().join("independent-witness.json");
    let stale = dir.path().join("older-backup.db");
    let signing_key = Ed25519KeyPair::from_pkcs8(
        Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .unwrap()
            .as_ref(),
    )
    .unwrap();
    let key = STANDARD.encode(signing_key.public_key().as_ref());
    let old = || Ok(OLD.to_owned());
    let new = || Ok(NEW.to_owned());
    let mut controller =
        Ledger::enroll_for_handoff(&database, &witness, old, SLOTS, Some(&key)).unwrap();
    controller
        .connection()
        .backup("main", &stale, rusqlite::backup::NO_PROGRESS)
        .unwrap();
    controller.mutate(|tx| {
        tx.execute("INSERT INTO lease(id,request_key,task,initial_seconds,phase,deadline,slot_id) VALUES('work','request','unfinished',60,'ready',9000,'primary')", [])?;
        Ok(())
    }).unwrap();
    let saved_witness = std::fs::read(&witness).unwrap();
    let saved: Value = serde_json::from_slice(&saved_witness).unwrap();
    let revision = saved["revision"].as_str().unwrap();
    let approved = authorization(&signing_key, revision, NEW);

    refused(Ledger::handoff(&database, &witness, new, SLOTS, &approved));
    drop(controller);
    refused(Ledger::handoff(&stale, &witness, new, SLOTS, &approved));
    let mut forged = approved.clone();
    forged["signature"] = json!(STANDARD.encode([0; 64]));
    refused(Ledger::handoff(&database, &witness, new, SLOTS, &forged));
    refused(Ledger::handoff(
        &database,
        &witness,
        new,
        &[("primary", "other-provider")],
        &approved,
    ));
    refused(Ledger::handoff(&database, &witness, old, SLOTS, &approved));
    assert_eq!(std::fs::read(&witness).unwrap(), saved_witness);

    let replacement = Ledger::handoff(&database, &witness, new, SLOTS, &approved).unwrap();
    let task: String = replacement
        .connection()
        .query_row(
            "SELECT task FROM lease WHERE id='work' AND phase='ready' AND slot_id='primary'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task, "unfinished");
    drop(replacement);
    assert!(Ledger::open(&database, &witness, old).is_err());
    assert!(Ledger::open(&database, &witness, new).is_ok());
    refused(Ledger::handoff(&database, &witness, new, SLOTS, &approved));
}
