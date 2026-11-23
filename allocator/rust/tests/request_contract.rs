use lab_allocator::{AcquireRequest, canonical_requirements};
use serde_json::{Value, json};

#[test]
fn acquire_canonicalizes_requirements_and_preserves_the_legacy_request_identity() {
    let input = json!({"task":"test task","request_key":"one","seconds":2700,"ssh_public_key":fixture_key(),
                    "requirements":{"vcpus":2,"dependencies":["voice","build"],"os":"linux"}});
    let request = AcquireRequest::parse(&input, true).unwrap();
    assert_eq!(request.recipe, None);
    assert_eq!(request.key("task"), "task/one");
    assert_eq!(request.seconds, 2700);
    assert_eq!(
        canonical_requirements(&request.requirements).unwrap(),
        r#"{"dependencies":["build","voice"],"os":"linux","vcpus":2}"#
    );
    assert_eq!(request.preferred_recipe(), "debian13-amd64");
}

#[test]
fn acquire_refuses_unqualified_work_and_noncheckpointed_leases_before_provider_admission() {
    let mut input = json!({"task":"test","request_key":"one","seconds":15,"ssh_public_key":fixture_key(),
                        "requirements":{"network":true}});
    assert_eq!(
        AcquireRequest::parse(&input, true).unwrap_err().code(),
        "unsupported_capability"
    );
    input["requirements"] = json!({"os":"macos"});
    assert_eq!(
        AcquireRequest::parse(&input, true)
            .unwrap()
            .preferred_recipe(),
        "macos-xcode-arm64"
    );
    input["checkpoint_id"] = json!("abcdef");
    assert_eq!(
        AcquireRequest::parse(&input, true).unwrap_err().code(),
        "invalid_source"
    );
    input.as_object_mut().unwrap().remove("checkpoint_id");
    input["reclaim_on_expiry"] = Value::Bool(false);
    let rejected = AcquireRequest::parse(&input, true).unwrap();
    assert!(!rejected.reclaim);
    assert_eq!(
        rejected.validate_new().unwrap_err().code(),
        "reclamation_required"
    );
}

fn fixture_key() -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut raw = b"\0\0\0\x0bssh-ed25519\0\0\0 ".to_vec();
    raw.extend([0u8; 32]);
    format!("ssh-ed25519 {}", STANDARD.encode(raw))
}
