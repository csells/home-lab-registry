use lab_registry::deployment::{
    DeploymentIdentity, RuntimeState, classify_launchd, classify_systemd,
};
use serde_json::json;

#[test]
fn registered_generation_includes_owner_and_provider_coordinates() {
    let mut spec = json!({"owner":"ops","resources":{"deployment_generation":"release-a",
        "runtime_binding":{"kind":"guest","machine_id":"machine-test","guest_id":"101"}}});
    let initial = spec.clone();
    let original = DeploymentIdentity::from_spec(&initial).unwrap();
    spec["resources"]["deployment_generation"] = json!("release-b");
    let replacement = DeploymentIdentity::from_spec(&spec).unwrap();
    assert_ne!(original, replacement);
    spec["resources"]["runtime_binding"]["machine_id"] = json!("unregistered-hostname");
    assert!(DeploymentIdentity::from_spec(&spec).is_err());
    spec["resources"]["runtime_binding"] =
        json!({"kind":"native","provider":"systemd","instance":"service-test.service"});
    assert!(DeploymentIdentity::from_spec(&spec).is_ok());
    spec["resources"]["runtime_binding"]["instance"] = json!("bad\ninstance");
    assert!(DeploymentIdentity::from_spec(&spec).is_err());
}

#[test]
fn runtime_observation_matches_exact_service_identity_and_preserves_unknowns() {
    use RuntimeState::*;
    assert_eq!(
        classify_systemd("web.service", true, "Id=web.service\nLoadState=masked"),
        Present
    );
    assert_eq!(
        classify_systemd("web.service", false, "Id=web.service\nLoadState=not-found"),
        Missing
    );
    assert_eq!(
        classify_systemd("web.service", true, "Id=alias.service\nLoadState=loaded"),
        Unverified
    );
    assert_eq!(
        classify_systemd("web.service", false, "Id=web.service\nLoadState=loaded"),
        Unverified
    );
    assert_eq!(
        classify_launchd(
            "gui/501/test.example",
            Some(0),
            "gui/501/test.example = {\n",
            ""
        ),
        Present
    );
    assert_eq!(
        classify_launchd(
            "gui/501/test.example",
            Some(113),
            "",
            "Could not find service"
        ),
        Missing
    );
    assert_eq!(
        classify_launchd("gui/501/test.example", Some(113), "", "not authorized"),
        Unverified
    );
}
