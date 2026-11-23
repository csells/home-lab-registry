use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::Response,
};
use lab_registry::{Catalog, RegistryHttp};
use serde_json::{Value, json};
use tokio::sync::watch;
use tokio_stream::StreamExt;
use tower::ServiceExt;

const READER: &str = "Bearer reader-fixture";
const WRITER: &str = "Bearer writer-fixture";

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    token: Option<&str>,
    revision: Option<&str>,
    body: Option<&Value>,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("Authorization", token);
    }
    if let Some(revision) = revision {
        request = request.header("If-Match", revision);
    }
    let payload = body.map_or_else(Vec::new, |body| serde_json::to_vec(body).unwrap());
    if !payload.is_empty() {
        request = request.header("Content-Length", payload.len());
    }
    app.clone()
        .oneshot(request.body(Body::from(payload)).unwrap())
        .await
        .unwrap()
}

async fn as_json(response: Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap()
}

#[tokio::test]
async fn legacy_routes_keep_auth_revision_and_durable_mutation_semantics() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&tmp.path().join("catalog.db")).unwrap();
    let (_shutdown, stopped) = watch::channel(false);
    let app = RegistryHttp::new(catalog, "reader-fixture", "writer-fixture", stopped)
        .unwrap()
        .router();
    let spec = json!({"name":"Test","host":"local","owner":"fixtures","runbook":"fixture",
        "endpoints":{},"lifecycle":"active","capabilities":["test"],"dependencies":[]});
    assert_eq!(
        send(&app, "GET", "/healthz", None, None, None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        send(&app, "GET", "/v1/services", None, None, None)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            "PUT",
            "/v1/services/test",
            Some(READER),
            Some("0"),
            Some(&spec)
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            "PUT",
            "/v1/services/test",
            Some(WRITER),
            None,
            Some(&spec)
        )
        .await
        .status(),
        StatusCode::PRECONDITION_REQUIRED
    );
    let created = send(
        &app,
        "PUT",
        "/v1/services/test",
        Some(WRITER),
        Some("\"0\""),
        Some(&spec),
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(as_json(created).await["revision"], 1);
    assert_eq!(
        send(
            &app,
            "PUT",
            "/v1/services/test",
            Some(WRITER),
            Some("0"),
            Some(&spec)
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let docs = as_json(
        send(
            &app,
            "GET",
            "/v1/services?capability=test",
            Some(READER),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(docs["services"][0]["spec"], spec);
    let health = json!({"status":"healthy","evidence":"fixture"});
    assert_eq!(
        send(
            &app,
            "POST",
            "/v1/services/test/observe",
            Some(WRITER),
            Some("1"),
            Some(&health)
        )
        .await
        .status(),
        StatusCode::OK
    );
    let observations =
        as_json(send(&app, "GET", "/v1/health?id=test", Some(READER), None, None).await).await;
    assert_eq!(observations["health"]["test"]["status"], "healthy");
    assert_eq!(
        as_json(
            send(
                &app,
                "GET",
                "/v1/services/test/history",
                Some(READER),
                None,
                None
            )
            .await
        )
        .await["history"][0]["actor"],
        "writer"
    );
}

#[tokio::test]
async fn stream_delivers_health_only_batches_and_stops_with_the_registry() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&tmp.path().join("catalog.db")).unwrap();
    catalog
        .put(
            "test",
            json!({"name":"Test","host":"local","owner":"fixtures","runbook":"fixture",
        "endpoints":{},"lifecycle":"active","capabilities":["test"],"dependencies":[]}),
            0,
        )
        .unwrap();
    let (shutdown, stopped) = watch::channel(false);
    let app = RegistryHttp::new(catalog, "reader-fixture", "writer-fixture", stopped)
        .unwrap()
        .router();
    let response = send(
        &app,
        "GET",
        "/v1/catalog/changes?health=1",
        Some(READER),
        None,
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/x-ndjson");
    let mut stream = response.into_body().into_data_stream();
    let initial: Value = serde_json::from_slice(&stream.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(initial["version"], 0);
    let health = json!({"status":"healthy","evidence":"fixture"});
    assert_eq!(
        send(
            &app,
            "POST",
            "/v1/services/test/observe",
            Some(WRITER),
            Some("1"),
            Some(&health)
        )
        .await
        .status(),
        StatusCode::OK
    );
    let next: Value = serde_json::from_slice(&stream.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(next["version"], 1);
    assert_eq!(next["epoch"], initial["epoch"]);
    assert_eq!(next["health"], json!(["test"]));
    shutdown.send_replace(true);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn an_incomplete_catalog_request_expires_without_writing_a_record() {
    let tmp = tempfile::tempdir().unwrap();
    let catalog = Catalog::open(&tmp.path().join("catalog.db")).unwrap();
    let (_shutdown, stopped) = watch::channel(false);
    let app = RegistryHttp::new(catalog, "reader-fixture", "writer-fixture", stopped)
        .unwrap()
        .router();
    let never_finishes =
        futures_util::stream::pending::<Result<axum::body::Bytes, std::io::Error>>();
    let request = Request::put("/v1/services/no-write")
        .header("Authorization", WRITER)
        .header("If-Match", "0")
        .header("Content-Length", "32")
        .body(Body::from_stream(never_finishes))
        .unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(11), app.clone().oneshot(request)).await;
    assert_eq!(
        outcome.expect("body read deadline").unwrap().status(),
        StatusCode::REQUEST_TIMEOUT
    );
    assert_eq!(
        send(
            &app,
            "GET",
            "/v1/services/no-write",
            Some(READER),
            None,
            None
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
