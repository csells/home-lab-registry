use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use lab_allocator::http::{Credentials, RouteRole, router, task_contract};
use serde_json::json;
use tower::ServiceExt;

#[test]
fn task_api_schema_matches_the_python_client_contract() {
    let output = std::process::Command::new("python3")
        .args(["-c", "import json; from capabilities import task_contract; print(json.dumps(task_contract()))"])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
        .output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(task_contract(), expected);
}

#[tokio::test]
async fn only_task_credentials_can_read_the_task_contract() {
    let directory = tempfile::tempdir().unwrap();
    let task = directory.path().join("task");
    let review = directory.path().join("review");
    let verifier = directory.path().join("verifier");
    std::fs::write(&task, "a".repeat(43)).unwrap();
    std::fs::write(&review, "b".repeat(43)).unwrap();
    std::fs::write(&verifier, "c".repeat(43)).unwrap();
    let config = json!({"client_tokens":{"task":task}, "review_token_file":review,
        "reclamation":{"verifier_token_file":verifier}});
    let credentials = Credentials::read(&config).unwrap();
    let app = router(credentials, None);
    for (key, status) in [
        ('a', StatusCode::OK),
        ('b', StatusCode::UNAUTHORIZED),
        ('c', StatusCode::UNAUTHORIZED),
        ('d', StatusCode::UNAUTHORIZED),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::get("/v1/contract")
                    .header(
                        "Authorization",
                        format!("Bearer {}", key.to_string().repeat(43)),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
    std::fs::write(&verifier, "a".repeat(43)).unwrap();
    assert!(Credentials::read(&config).is_err());
}

#[test]
fn bare_and_child_endpoint_paths_require_their_specific_roles() {
    for (path, role) in [
        (&["v1", "lease-review"][..], RouteRole::Review),
        (&["v1", "final-export", "id"], RouteRole::Verifier),
        (&["v1", "checkpoint-expiry"], RouteRole::Verifier),
        (&["v1", "export-alerts"], RouteRole::Alert),
        (&["v1", "complete-upload"], RouteRole::Recovery),
        (
            &["v1", "capacity-reservations", "id", "release"],
            RouteRole::Reservation,
        ),
        (&["v1", "task-credentials"], RouteRole::Issuer),
        (&["v1", "delegated-renewal", "id"], RouteRole::Delegation),
        (&["v1", "leases", "id"], RouteRole::Task),
    ] {
        assert_eq!(RouteRole::for_path(path), role);
    }
}
