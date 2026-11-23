use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use serde_json::json;

#[test]
fn storage_role_accepts_only_its_managed_snapshot_and_exact_command() {
    let dir = tempfile::tempdir().unwrap();
    let restic = dir.path().join("restic");
    let archive = format!("lease-{}.tar.gz", "a".repeat(32));
    let snapshot = "b".repeat(64);
    let tag = format!("lease-export-{}", "c".repeat(32));
    let inventory =
        json!([{"id":snapshot,"paths":[format!("/{archive}")],"tags":["lease-final-export",tag]}]);
    fs::write(&restic, format!("#!/bin/sh\ncase \"$1\" in\nsnapshots) printf '%s' '{inventory}' ;;\ndump) printf 'verified workspace' ;;\nforget) printf '{{}}' ;;\n*) exit 9 ;;\nesac\n")).unwrap();
    fs::set_permissions(&restic, fs::Permissions::from_mode(0o700)).unwrap();
    let config = dir.path().join("storage.json");
    let run = |role: &str, args: &[&str]| {
        fs::write(
            &config,
            json!({"role":role,"restic_command":[restic]}).to_string(),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_lab-restic-scope"))
            .arg(&config)
            .args(args)
            .output()
            .unwrap()
    };

    let archive = format!("/{archive}");
    let restored = run("read", &["dump", &snapshot, &archive]);
    assert!(restored.status.success());
    assert_eq!(restored.stdout, b"verified workspace");
    assert_eq!(
        run("read", &["forget", &snapshot, "--json"]).status.code(),
        Some(2)
    );
    assert_eq!(
        run("write", &["dump", &"d".repeat(64), &archive])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run(
            "write",
            &[
                "dump",
                &snapshot,
                &format!("/lease-{}.tar.gz", "d".repeat(32))
            ]
        )
        .status
        .code(),
        Some(2)
    );
    assert!(
        run("expire", &["forget", &snapshot, "--json"])
            .status
            .success()
    );
    assert_eq!(
        run("expire", &["forget", "--prune", "--json"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        run("expire", &["dump", &snapshot, &archive]).status.code(),
        Some(2)
    );
}
