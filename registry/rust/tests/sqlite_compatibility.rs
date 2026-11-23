use std::{
    path::{Path, PathBuf},
    process::Command,
};

use lab_registry::Catalog;
use serde_json::{Value, json};

fn python(code: &str, paths: &[&Path]) {
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let result = Command::new("python3")
        .current_dir(cwd)
        .arg("-c")
        .arg(code)
        .args(paths)
        .output()
        .expect("Python is required for the transition compatibility test");
    assert!(
        result.status.success(),
        "Legacy command failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn spec(name: &str) -> Value {
    json!({
        "name":name, "host":"test", "owner":"fixtures", "runbook":"test",
        "capabilities":["test"], "dependencies":[], "endpoints":{}, "lifecycle":"active",
        "resources":{"runtime_binding":{"kind":"native","provider":"test","instance":"service"}},
    })
}

#[test]
fn python_catalog_remains_readable_and_rust_revisions_are_visible_to_legacy_clients() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("catalog.db");
    python(
        r#"
import sys, time
from registry import Catalog
c = Catalog(sys.argv[1])
c.put('service-test', dict(name='Original',host='test',owner='fixtures',runbook='test',
    capabilities=['test'],dependencies=[],endpoints={},lifecycle='active',resources={
        'runtime_binding':{'kind':'native','provider':'test','instance':'service'}
    }), 0)
c.observe('service-test', {'status':'healthy','evidence':'fixture check'}, revision=1)
c.report_runtime('service-test', {'state':'present','ttl_seconds':120}, {
    'kind':'native','provider':'test','instance':'service'
})
"#,
        &[&path],
    );
    let catalog = Catalog::open(&path).unwrap();
    let before = catalog.get("service-test").unwrap();
    assert_eq!(before["revision"], 1);
    assert_eq!(before["health"]["status"], "healthy");
    assert_eq!(before["runtime_observation"]["state"], "present");
    let error = catalog
        .put("service-test", spec("Wrong revision"), 0)
        .unwrap_err();
    assert_eq!(error.status(), 409);
    let mut candidate = spec("Renamed");
    candidate["resources"]["agent_presence"] = json!({"address":"192.0.2.70"});
    let after = catalog.put("service-test", candidate, 1).unwrap();
    assert_eq!(after["revision"], 2);
    assert_eq!(after["health"]["status"], "unknown");
    assert_eq!(after["runtime_observation"]["state"], "present");
    assert!(after["spec"]["resources"].get("agent_presence").is_none());
    python(
        r#"
import sys
from registry import Catalog
c = Catalog(sys.argv[1])
assert c.get('service-test')['spec']['name'] == 'Renamed'
history = c.history('service-test')
assert [row['revision'] for row in history] == [1, 2]
assert all(row['actor'] == 'writer' for row in history)
"#,
        &[&path],
    );
}

#[test]
fn export_restores_revision_history_with_existing_operator_tool_and_resets_observations() {
    let tmp = tempfile::tempdir().unwrap();
    let database = tmp.path().join("source.db");
    let restored = tmp.path().join("restored.db");
    let export = tmp.path().join("export.json");
    let catalog = Catalog::open(&database).unwrap();
    catalog.put("service-test", spec("Original"), 0).unwrap();
    catalog.put("service-test", spec("Renamed"), 1).unwrap();
    catalog
        .observe(
            "service-test",
            json!({"status":"healthy","evidence":"fixture check"}),
            Some(2),
        )
        .unwrap();
    std::fs::write(
        &export,
        serde_json::to_vec(&catalog.export().unwrap()).unwrap(),
    )
    .unwrap();
    python(
        r#"import runpy, sys
sys.argv[0] = 'restore.py'
runpy.run_path('restore.py', run_name='__main__')
"#,
        &[&export, &restored],
    );
    let recovery = Catalog::open(&restored).unwrap();
    assert_eq!(recovery.get("service-test").unwrap()["revision"], 2);
    assert_eq!(
        recovery.get("service-test").unwrap()["health"]["status"],
        "unknown"
    );
    assert_eq!(
        catalog.history("service-test").unwrap(),
        recovery.history("service-test").unwrap()
    );
}

#[test]
fn only_bound_routes_move_for_verified_presence_and_legacy_readers_share_the_record() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("catalog.db");
    let catalog = Catalog::open(&path).unwrap();
    let mut machine = spec("Machine");
    machine["capabilities"] = json!(["machine"]);
    machine["endpoints"] = json!({"os":"ssh://192.0.2.10:2222","kvm":"https://192.0.2.11/"});
    machine["resources"]["registration"] = json!({"endpoint_keys":["os"]});
    catalog.put("machine-test", machine, 0).unwrap();
    let mut guest = spec("Bound service");
    guest["endpoints"] = json!({"api":"http://192.0.2.10:9080/health"});
    guest["resources"]["address_bindings"] =
        json!([{"machine_id":"machine-test","endpoint_keys":["api"]}]);
    catalog.put("service-test", guest, 0).unwrap();
    let (report, changed) = catalog
        .announce_verified("machine-test", "192.0.2.80", "eth0")
        .unwrap();
    assert!(changed && report["accepted"] == true);
    let (_, changed) = catalog
        .announce_verified("machine-test", "192.0.2.80", "eth0")
        .unwrap();
    assert!(!changed);
    let machine = catalog.get("machine-test").unwrap();
    assert_eq!(machine["spec"]["endpoints"]["os"], "ssh://192.0.2.80:2222");
    assert_eq!(machine["spec"]["endpoints"]["kvm"], "https://192.0.2.11/");
    assert_eq!(
        machine["spec"]["resources"]["agent_presence"]["status"],
        "current"
    );
    assert_eq!(machine["revision"], 1);
    let guest = catalog.get("service-test").unwrap();
    assert_eq!(
        guest["spec"]["endpoints"]["api"],
        "http://192.0.2.80:9080/health"
    );
    python(
        "import sys; from registry import Catalog; assert Catalog(sys.argv[1]).get('service-test')['spec']['endpoints']['api']=='http://192.0.2.80:9080/health'",
        &[&path],
    );
    assert_eq!(
        catalog.export().unwrap()["services"][0]["spec"]["endpoints"]["os"],
        "ssh://192.0.2.10:2222"
    );
}
