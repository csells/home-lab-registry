use std::{os::unix::fs::PermissionsExt, process::Command};

use lab_registry::{Catalog, client::resolve_ssh_record};
use serde_json::{Value, json};

fn spec(name: &str) -> Value {
    json!({
        "name":name, "host":"test", "owner":"fixtures", "runbook":"test",
        "capabilities":["test"],"dependencies":[],"endpoints":{},"lifecycle":"active"
    })
}

#[test]
fn restore_preserves_history_resets_observations_and_never_overwrites() {
    let root = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&root.path().join("source.db")).unwrap();
    catalog.put("service-test", spec("Initial"), 0).unwrap();
    catalog.put("service-test", spec("Renamed"), 1).unwrap();
    catalog
        .observe(
            "service-test",
            json!({"status":"healthy","evidence":"fixture"}),
            Some(2),
        )
        .unwrap();
    let export = root.path().join("archive.json");
    std::fs::write(
        &export,
        serde_json::to_vec(&catalog.export().unwrap()).unwrap(),
    )
    .unwrap();
    let restored = root.path().join("restored.db");
    let mut cli = Command::new(env!("CARGO_BIN_EXE_lab-registry-restore"));
    cli.arg(&export).arg(&restored);
    let result = cli.output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let recovered = Catalog::open(&restored).unwrap();
    let view = recovered.get("service-test").unwrap();
    assert_eq!(view["revision"], 2);
    assert_eq!(view["health"]["status"], "unknown");
    assert_eq!(
        recovered.history("service-test").unwrap(),
        catalog.history("service-test").unwrap()
    );
    assert_eq!(
        std::fs::metadata(&restored).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let result = cli.output().unwrap();
    assert!(!result.status.success());
    assert_eq!(recovered.get("service-test").unwrap()["revision"], 2);
}

#[test]
fn an_incomplete_archive_does_not_publish_a_database() {
    let root = tempfile::tempdir().unwrap();
    let export = root.path().join("invalid.json");
    let destination = root.path().join("restore.db");
    std::fs::write(
        &export,
        r#"{"services":[{"id":"service-test","revision":2}],"history":{}}"#,
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_lab-registry-restore"))
        .arg(export)
        .arg(&destination)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(!destination.exists());
}

#[test]
fn resolution_requires_a_current_registration_and_keeps_the_pin_stable() {
    let mut record = json!({"spec":{"lifecycle":"active", "endpoints":{"ssh":"ssh://192.0.2.11:2222"},
        "resources":{"agent_presence":{"status":"current"}}}});
    let result = resolve_ssh_record(&record, "machine-test", "backup", None).unwrap();
    assert_eq!(result.destination, "backup@192.0.2.11");
    assert_eq!(result.port, 2222);
    assert_eq!(result.host_key_alias, "machine-test");
    record["spec"]["endpoints"]["ssh"] = json!("ssh://192.0.2.12");
    assert_eq!(
        resolve_ssh_record(&record, "machine-test", "backup", Some(2200))
            .unwrap()
            .port,
        2200
    );
    record["spec"]["resources"]["agent_presence"]["status"] = json!("stale");
    assert!(resolve_ssh_record(&record, "machine-test", "backup", None).is_err());
}
