use std::path::PathBuf;

use lab_allocator::Provider;
use serde_json::json;

#[test]
fn provider_boundary_reports_structured_rejections_and_does_not_interpret_arbitrary_actions() {
    let tmp = tempfile::tempdir().unwrap();
    let settings = tmp.path().join("config.json");
    std::fs::write(&settings, "{}").unwrap();
    let adapter = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../provider_adapter.py");
    let provider = Provider::new(adapter, settings);
    let error = provider
        .call(json!({"action":"shell","command":"unsupported"}))
        .unwrap_err();
    assert_eq!(error.status(), 400);
    assert_eq!(error.code(), "provider_operation_unknown");
}
