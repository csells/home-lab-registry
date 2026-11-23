use std::{path::PathBuf, time::Duration};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
    response::Response,
};
use lab_dashboard::{Dashboard, Snapshot};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_stream::StreamExt;
use tower::ServiceExt;

const TOKEN: &str = "Basic dGVzdDp0ZXN0";
const ORIGIN: &str = "https://lab.example.invalid";
const CSP: &str = "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'";

fn initial() -> Snapshot {
    Snapshot {
        machines: vec![json!({"id":"machine-test","name":"Host"})],
        services: vec![json!({"id":"service-test","name":"Service"})],
        ..Snapshot::default()
    }
}

fn setup() -> (Router, Dashboard, watch::Sender<bool>) {
    setup_assets(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."))
}

fn setup_assets(assets: PathBuf) -> (Router, Dashboard, watch::Sender<bool>) {
    let (shutdown, listening) = watch::channel(false);
    let dashboard = Dashboard::new(
        assets,
        TOKEN.parse().unwrap(),
        [ORIGIN.parse().unwrap()],
        true,
        initial(),
        listening,
    );
    (dashboard.router(), dashboard, shutdown)
}

async fn get(app: &Router, uri: &str, extra: &[(&str, &str)], authenticated: bool) -> Response {
    let mut request = Request::builder().uri(uri);
    if authenticated {
        request = request.header(header::AUTHORIZATION, TOKEN);
    }
    for (name, value) in extra {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body_json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap()
}

#[tokio::test]
async fn static_assets_keep_csp_and_cannot_bypass_the_route_allowlist() {
    let assets = tempfile::tempdir().unwrap();
    std::fs::write(
        assets.path().join("index.html"),
        include_bytes!("../../index.html"),
    )
    .unwrap();
    std::fs::create_dir(assets.path().join("vendor")).unwrap();
    std::fs::write(
        assets.path().join("vendor/elk-worker.min.js"),
        "// Worker asset test",
    )
    .unwrap();
    let (app, _, _) = setup_assets(assets.path().into());
    let response = get(&app, "/", &[], false).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers()[header::WWW_AUTHENTICATE],
        "Basic realm=\"Home lab preview\""
    );
    let response = get(&app, "/leases", &[], true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_SECURITY_POLICY], CSP);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/html; charset=utf-8"
    );
    let body = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("dashboard-react.js"));
    assert_eq!(
        get(&app, "/vendor/elk-worker.min.js", &[], true)
            .await
            .status(),
        StatusCode::OK
    );
    for path in [
        "/app.js",
        "/resource-graph.js",
        "/vendor/elk-api.js",
        "/private/login.json",
        "/vendor/%2e%2e/private/login.json",
    ] {
        assert_eq!(
            get(&app, path, &[], true).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn snapshot_projections_keep_public_shapes_and_reconciliation_receipts() {
    let (app, dashboard, _shutdown) = setup();
    dashboard.publish(Snapshot {
        fetched_at: Some(123.0),
        comparison: Some(json!({"id":"receipt","state":"complete"})),
        inventory_accuracy: Some(json!({"coverage":{"expected":1,"verified":1,"unconfigured":0}})),
        ..initial()
    });
    let response = get(&app, "/api/inventory", &[], true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_SECURITY_POLICY], CSP);
    let inventory = body_json(response).await;
    assert_eq!(inventory["lease_review_enabled"], true);
    assert!(inventory["server_time"].as_f64().unwrap() > 123.0);
    assert_eq!(inventory["machines"][0]["id"], "machine-test");
    let receipt = body_json(get(&app, "/api/reconciliation", &[], true).await).await;
    assert_eq!(receipt.as_object().unwrap().len(), 4);
    assert_eq!(receipt["comparison"]["id"], "receipt");
    let services = body_json(get(&app, "/api/services", &[], true).await).await;
    assert_eq!(
        services,
        json!({"services":[{"id":"service-test","name":"Service"}],"error":null,"fetched_at":123.0})
    );
}

#[tokio::test]
async fn inventory_stream_rejects_foreign_origins_and_recovers_with_only_the_latest_snapshot() {
    let (app, dashboard, shutdown) = setup();
    for headers in [
        [("Origin", "https://foreign.example.invalid")],
        [("Sec-Fetch-Site", "cross-site")],
    ] {
        assert_eq!(
            get(&app, "/api/inventory/stream", &headers, true)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = get(&app, "/api/inventory/stream", &[("Origin", ORIGIN)], true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let mut stream = response.into_body().into_data_stream();
    let first = String::from_utf8(stream.next().await.unwrap().unwrap().to_vec()).unwrap();
    assert!(first.contains("event: inventory\n"));
    assert!(first.contains("machine-test"));
    for sequence in 1..100 {
        dashboard.publish(Snapshot {
            fetched_at: Some(f64::from(sequence)),
            ..initial()
        });
    }
    let newest = String::from_utf8(stream.next().await.unwrap().unwrap().to_vec()).unwrap();
    assert!(newest.contains("\"fetched_at\":99.0"));
    drop(stream);
    let mut reconnected = get(&app, "/api/inventory/stream", &[], true)
        .await
        .into_body()
        .into_data_stream();
    let latest = String::from_utf8(reconnected.next().await.unwrap().unwrap().to_vec()).unwrap();
    assert!(latest.contains("\"fetched_at\":99.0"));
    shutdown.send(true).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(1), reconnected.next())
            .await
            .expect("shutdown closed SSE")
            .is_none()
    );
}

async fn copy_password(
    app: &Router,
    origin: &str,
    action: &str,
    content: &str,
    body: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::post("/api/copy-password")
                .header(header::AUTHORIZATION, TOKEN)
                .header(header::ORIGIN, origin)
                .header("x-lab-action", action)
                .header(header::CONTENT_TYPE, content)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn password_controls_validate_origin_intent_and_size_before_reading_the_allowlist() {
    let private = tempfile::tempdir().unwrap();
    std::fs::write(
        private.path().join("machine-passwords.json"),
        r#"{"machine-test":{"os":"demo-password"},"machine-other":{"os":"another-demo-password"}}"#,
    )
    .unwrap();
    let (_, shutdown) = watch::channel(false);
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let dashboard = Dashboard::new(
        assets,
        TOKEN.parse().unwrap(),
        [ORIGIN.parse().unwrap()],
        false,
        Snapshot {
            machines: vec![json!({"id":"machine-test","name":"Host","access":[{"id":"os","password_available":true}]})],
            ..Snapshot::default()
        },
        shutdown,
    ).with_access(private.path().to_path_buf(), Some("machine-test".into()));
    let app = dashboard.router();
    let request = r#"{"machine":"machine-test","login":"os"}"#;
    for (origin, action, content) in [
        (
            "https://foreign.example.invalid",
            "copy-password",
            "application/json",
        ),
        (ORIGIN, "refresh-catalog", "application/json"),
        (ORIGIN, "copy-password", "application/x-www-form-urlencoded"),
    ] {
        assert_eq!(
            copy_password(&app, origin, action, content, request)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let result = copy_password(&app, ORIGIN, "copy-password", "application/json", request).await;
    assert_eq!(result.status(), StatusCode::OK);
    assert_eq!(result.headers()[header::CACHE_CONTROL], "no-store, private");
    assert_eq!(
        result.headers()[header::CONTENT_TYPE],
        "text/plain; charset=utf-8"
    );
    assert_eq!(result.headers()[header::PRAGMA], "no-cache");
    assert_eq!(
        to_bytes(result.into_body(), 100).await.unwrap(),
        "demo-password"
    );
    let result = copy_password(
        &app,
        ORIGIN,
        "copy-password",
        "application/json",
        r#"{"machine":"machine-other","login":"os"}"#,
    )
    .await;
    assert_eq!(result.status(), StatusCode::NOT_FOUND);
    let result = copy_password(
        &app,
        ORIGIN,
        "copy-password",
        "application/json",
        &" ".repeat(1025),
    )
    .await;
    assert_eq!(result.status(), StatusCode::BAD_REQUEST);
}

async fn action(app: &Router, route: &str, intent: &str) -> Response {
    app.clone()
        .oneshot(
            Request::post(route)
                .header(header::AUTHORIZATION, TOKEN)
                .header(header::ORIGIN, ORIGIN)
                .header("x-lab-action", intent)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn comparison_accepts_one_request_and_only_its_own_completion_can_settle_it() {
    let (app, dashboard, _shutdown) = setup();
    let mut requests = dashboard.requested_updates();
    let response = action(&app, "/api/reconcile", "reconcile").await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let accepted = body_json(response).await;
    let id = accepted["id"].as_str().unwrap();
    assert_eq!(accepted["state"], "pending");
    requests.changed().await.unwrap();
    assert_eq!(requests.borrow().comparison_id.as_deref(), Some(id));
    assert_eq!(
        action(&app, "/api/reconcile", "reconcile").await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert!(!dashboard.finish_comparison("unrelated", true));
    let pending = body_json(get(&app, "/api/reconciliation", &[], true).await).await;
    assert_eq!(pending["comparison"], accepted);
    assert!(dashboard.finish_comparison(id, true));
    let complete = body_json(get(&app, "/api/reconciliation", &[], true).await).await;
    assert_eq!(complete["comparison"], json!({"id":id, "state":"complete"}));
}

#[tokio::test]
async fn refresh_waits_for_a_catalog_observation_covering_its_own_request() {
    let (app, dashboard, _shutdown) = setup();
    let mut requests = dashboard.requested_updates();
    let pending_app = app.clone();
    let mut refresh =
        tokio::spawn(async move { action(&pending_app, "/api/refresh", "refresh-catalog").await });
    requests.changed().await.unwrap();
    let accepted = requests.borrow().catalog_generation;
    assert!(accepted > 0);
    dashboard.finish_refresh(accepted - 1);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut refresh)
            .await
            .is_err()
    );
    dashboard.publish(Snapshot {
        fetched_at: Some(222.0),
        catalog: Some(json!({"checked":222.0,"feed":"connected","snapshot_at":null})),
        ..initial()
    });
    dashboard.finish_refresh(accepted);
    let completed = tokio::time::timeout(Duration::from_secs(1), refresh)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(completed.status(), StatusCode::OK);
    let snapshot = body_json(completed).await;
    assert_eq!(snapshot["catalog"]["checked"], 222.0);
}

#[tokio::test]
async fn metrics_open_only_for_configured_targets_and_stop_when_last_viewer_goes_away() {
    use lab_dashboard::collector::Target;
    let (app, dashboard, _shutdown) = setup();
    let target = Target::parse("https://192.0.2.12:9911/stream", None).unwrap();
    dashboard
        .metrics()
        .configure([("machine-test".to_owned(), target)].into());
    assert_eq!(
        dashboard.metrics().readings()["machine-test"]["status"],
        "waiting"
    );
    let response = get(
        &app,
        "/api/metrics/stream?machine=machine-test",
        &[("Sec-Fetch-Site", "cross-site")],
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        dashboard.metrics().readings()["machine-test"]["status"],
        "waiting"
    );
    let response = get(
        &app,
        "/api/metrics/stream?machine=not-configured",
        &[],
        true,
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = get(&app, "/api/metrics/stream?machine=machine-test", &[], true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut first = response.into_body().into_data_stream();
    let data = first.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&data).starts_with("event: metrics\n"));
    let response = get(&app, "/api/metrics/stream?machine=machine-test", &[], true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_ne!(
        dashboard.metrics().readings()["machine-test"]["status"],
        "waiting"
    );
    drop(first);
    assert_ne!(
        dashboard.metrics().readings()["machine-test"]["status"],
        "waiting"
    );
    drop(response);
    assert_eq!(
        dashboard.metrics().readings()["machine-test"]["status"],
        "waiting"
    );
}

#[tokio::test]
async fn the_private_allowlist_controls_new_requests_without_a_restart() {
    let root = tempfile::tempdir().unwrap();
    let origins = root.path().join("origins.json");
    let replacement = "https://replacement.example.invalid";
    std::fs::write(
        root.path().join("login.json"),
        r#"{"username":"test","password":"test"}"#,
    )
    .unwrap();
    std::fs::write(&origins, json!([ORIGIN]).to_string()).unwrap();
    let (_shutdown, rx) = watch::channel(false);
    let app = Dashboard::from_directory(root.path().into(), root.path().into(), None, rx)
        .await
        .unwrap()
        .router();
    let check =
        async |origin: &str| get(&app, "/api/inventory/stream", &[("origin", origin)], true).await;
    assert_eq!(check(ORIGIN).await.status(), StatusCode::OK);
    assert_eq!(check(replacement).await.status(), StatusCode::FORBIDDEN);
    std::fs::write(&origins, json!([replacement]).to_string()).unwrap();
    assert_eq!(check(ORIGIN).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(check(replacement).await.status(), StatusCode::OK);
    std::fs::write(&origins, "{").unwrap();
    assert_eq!(check(replacement).await.status(), StatusCode::FORBIDDEN);
}
