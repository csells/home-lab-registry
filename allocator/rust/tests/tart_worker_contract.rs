use std::{fs, os::unix::fs::PermissionsExt, process::Command};

use serde_json::{Value, json};

#[test]
fn preflight_checks_the_pinned_recipe_and_refuses_unresolved_work() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().canonicalize().unwrap().join("store");
    let state = root.path().canonicalize().unwrap().join("state");
    let template = store.join("vms/qualified-template");
    fs::create_dir_all(&template).unwrap();
    fs::create_dir(&state).unwrap();
    fs::write(template.join("config.json"), b"{}").unwrap();
    let binary = root.path().join("tart");
    fs::write(&binary, "#!/bin/sh\ncase $1 in\nlist) printf '%s' '[{\"Name\":\"qualified-template\",\"Source\":\"local\",\"Running\":false}]' ;;\n*) exit 7 ;;\nesac\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let control = root.path().join("control");
    let capture = root.path().join("capture");
    fs::write(&control, "a".repeat(43)).unwrap();
    fs::write(&capture, "b".repeat(43)).unwrap();
    let path = root.path().join("config");
    let mut settings = json!({
        "state":state,"tart_home":store,"tart_binary":binary,"template":"qualified-template",
        "template_config_sha256":"44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
        "identity":"1".repeat(32), "shape":{"vcpus":1,"memory_mib":1,"disk_mib":1},
        "interface":"en0","guest_network":"192.0.2.0/24","max_running":1,
        "disk_reserve_mib":1,"memory_floor_mib":0,"storage_floor_mib":0,
        "guest_user":"task", "port":9294,"control_token_file":control,"capture_token_file":capture
    });
    let preflight = |settings: &Value| {
        fs::write(&path, serde_json::to_vec(settings).unwrap()).unwrap();
        Command::new(env!("CARGO_BIN_EXE_lab-tart-worker"))
            .arg(&path)
            .arg("--preflight")
            .output()
            .unwrap()
    };
    let out = preflight(&settings);
    #[cfg(target_os = "macos")]
    {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["slot_idle"], true);
    }
    #[cfg(not(target_os = "macos"))]
    assert!(!out.status.success());
    settings["capture_token_file"] = json!(root.path().join("control"));
    assert!(String::from_utf8_lossy(&preflight(&settings).stderr).contains("distinct"));
    settings["capture_token_file"] = json!(root.path().join("capture"));
    fs::write(template.join("config.json"), b"changed").unwrap();
    assert!(String::from_utf8_lossy(&preflight(&settings).stderr).contains("pinned and stopped"));
    fs::write(template.join("config.json"), b"{}").unwrap();
    let lease = "2".repeat(32);
    fs::write(
        state.join(format!("lease-{lease}.json")),
        format!(r#"{{"lease":"{lease}","operations":{{"clone":"pending"}}}}"#),
    )
    .unwrap();
    assert!(String::from_utf8_lossy(&preflight(&settings).stderr).contains("owned guest"));
}
