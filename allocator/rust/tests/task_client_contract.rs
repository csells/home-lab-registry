use lab_allocator::task_client::{TaskSession, endpoint_from_record, verified_origin};
use serde_json::json;

#[test]
fn task_discovery_accepts_only_active_registry_lease_origins() {
    let mut entry = json!({"spec":{"lifecycle":"active","capabilities":["lease-pilot"],
        "endpoints":{"lease_api":"https://allocator.example.invalid:8093/"}}});
    assert_eq!(
        endpoint_from_record(&entry).unwrap(),
        "https://allocator.example.invalid:8093"
    );
    entry["spec"]["lifecycle"] = json!("retired");
    assert!(endpoint_from_record(&entry).is_err());
    for denied in [
        "http://example.invalid",
        "https://user@example.invalid",
        "https://example.invalid/path",
        "https://example.invalid/?query",
        "https://example.invalid/#hash",
    ] {
        assert!(verified_origin(denied).is_err(), "{denied}");
    }
}

#[test]
fn a_session_uses_only_its_enrolled_endpoint_and_local_files() {
    let fixture = tempfile::tempdir().unwrap();
    let path = fixture.path().join("session.json");
    std::fs::write(
        &path,
        r#"{"version":1,"endpoint":"https://allocator.example.invalid","expires_at":200}"#,
    )
    .unwrap();
    let session = TaskSession::load(&path, 100.0).unwrap();
    assert_eq!(session.endpoint, "https://allocator.example.invalid");
    assert_eq!(session.ca, fixture.path().join("ca.pem"));
    assert_eq!(session.token, fixture.path().join("task.token"));
    assert!(TaskSession::load(&path, 205.0).is_err());
    std::fs::write(&path, r#"{"version":1,"endpoint":"https://allocator.example.invalid","expires_at":200,"token_file":"/tmp/unowned"}"#).unwrap();
    assert!(TaskSession::load(&path, 100.0).is_err());
}

#[test]
fn task_cli_exposes_existing_routes_and_rejects_enrollment_bypass_before_connecting() {
    use std::process::Command;
    let cli = env!("CARGO_BIN_EXE_lab-lease");
    let help = Command::new(cli).arg("--help").output().unwrap();
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for command in [
        "acquire",
        "resume",
        "checkpoint",
        "delegated-renew",
        "reserve-capacity",
        "retain-failed-export",
        "override-reviewer",
        "issue-task-credential",
    ] {
        assert!(help.contains(command), "{command}");
    }
    let invalid = Command::new(cli)
        .args([
            "--endpoint",
            "http://127.0.0.1:1",
            "--ca",
            "unused",
            "--token-file",
            "unused",
            "capabilities",
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    let error: serde_json::Value = serde_json::from_slice(&invalid.stderr).unwrap();
    assert_eq!(error["code"], "client_error");
    assert!(error["reason"].as_str().unwrap().contains("HTTPS"));
}
