mod catalog;
mod guests;
mod reconcile;
mod runtime;
mod terminal;

pub use catalog::Registry;
pub mod collector;
pub mod inventory;
mod metrics;
mod operations;
mod upstream;

pub use operations::UpdateRequest;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
pub mod network;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::watch;

const CONTENT_POLICY: &str = "default-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'";
const SEC_FETCH_SITE: HeaderName = HeaderName::from_static("sec-fetch-site");
const X_ACCEL_BUFFERING: HeaderName = HeaderName::from_static("x-accel-buffering");

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Snapshot {
    pub machines: Vec<Value>,
    pub services: Vec<Value>,
    pub backups: Vec<Value>,
    pub error: Option<String>,
    pub fetched_at: Option<f64>,
    pub comparison: Option<Value>,
    pub inventory_accuracy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address_audit: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_issues: Option<Value>,
}

#[derive(Serialize)]
struct InventoryPayload<'a> {
    #[serde(flatten)]
    snapshot: &'a Snapshot,
    server_time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease_review_enabled: Option<bool>,
}

#[derive(Clone)]
pub struct Dashboard {
    shared: Arc<Shared>,
    private: PathBuf,
    access_target: Option<String>,
    reload_origins: bool,
    metrics: collector::LiveMetrics,
}

struct Shared {
    assets: PathBuf,
    authority: HeaderValue,
    origins: HashSet<HeaderValue>,
    lease_review_enabled: bool,
    latest: watch::Sender<Arc<Snapshot>>,
    shutdown: watch::Receiver<bool>,
    operations: operations::Operations,
    viewers: watch::Sender<usize>,
    terminal: terminal::Terminal,
}

#[derive(Deserialize)]
pub(crate) struct Login {
    username: String,
    password: String,
}

impl Dashboard {
    pub fn new(
        assets: PathBuf,
        authority: HeaderValue,
        origins: impl IntoIterator<Item = HeaderValue>,
        lease_review_enabled: bool,
        initial: Snapshot,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let (latest, _) = watch::channel(Arc::new(initial));
        let private = assets.join("private");
        Self {
            shared: Arc::new(Shared {
                assets,
                authority,
                origins: origins.into_iter().collect(),
                lease_review_enabled,
                latest,
                shutdown,
                operations: operations::Operations::default(),
                viewers: watch::Sender::new(0),
                terminal: terminal::Terminal::default(),
            }),
            metrics: collector::LiveMetrics::new(private.clone()),
            private,
            access_target: None,
            reload_origins: false,
        }
    }

    pub async fn from_directory(
        assets: PathBuf,
        private: PathBuf,
        scope: Option<String>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<Self, &'static str> {
        let login: Login = upstream::read_json(&private.join("login.json")).await?;
        if login.username.is_empty() || login.password.is_empty() {
            return Err("Dashboard login configuration is incomplete");
        }
        let credential = STANDARD.encode(format!("{}:{}", login.username, login.password));
        let mut auth = HeaderValue::from_str(&format!("Basic {credential}"))
            .map_err(|_| "Invalid dashboard credentials")?;
        auth.set_sensitive(true);
        let headers = read_origins(&private).await?;
        let lease_review_enabled = private.join("lease-review.json").is_file();
        let initial = Snapshot {
            error: Some("Loading registry".into()),
            ..Snapshot::default()
        };
        let mut dashboard = Self::new(
            assets,
            auth,
            headers,
            lease_review_enabled,
            initial,
            shutdown,
        )
        .with_access(private, scope);
        dashboard.reload_origins = true;
        Ok(dashboard)
    }

    pub fn with_access(mut self, private: PathBuf, target: Option<String>) -> Self {
        self.metrics = collector::LiveMetrics::new(private.clone());
        self.private = private;
        self.access_target = target;
        self
    }

    async fn allows_origin(&self, origin: &HeaderValue) -> bool {
        if self.reload_origins {
            return read_origins(&self.private)
                .await
                .is_ok_and(|origins| origins.contains(origin));
        }
        self.shared.origins.contains(origin)
    }

    pub fn metrics(&self) -> &collector::LiveMetrics {
        &self.metrics
    }

    pub fn router(&self) -> Router {
        Router::new().fallback(handle).with_state(self.clone())
    }

    pub fn publish(&self, snapshot: Snapshot) {
        self.publish_with_comparison(snapshot);
    }
}

fn unix_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time since Unix epoch")
        .as_secs_f64()
}

async fn handle(State(dashboard): State<Dashboard>, request: Request) -> Response {
    let headers = request.headers();
    if !authorized(headers, &dashboard.shared.authority) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Basic realm=\"Home lab preview\"")],
        )
            .into_response();
    }
    if request.method() == Method::POST {
        return perform_action(dashboard, request).await;
    }
    if request.method() != Method::GET {
        return StatusCode::NOT_IMPLEMENTED.into_response();
    }
    match request.uri().path() {
        "/api/metrics/stream" => {
            if !trusted_stream(headers, &dashboard).await {
                return StatusCode::FORBIDDEN.into_response();
            }
            let Some(machine) =
                form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
                    .find_map(|(key, value)| {
                        (key == "machine" && !value.is_empty()).then_some(value)
                    })
            else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let Some(viewer) = dashboard.metrics.subscribe(&machine) else {
                return StatusCode::NOT_FOUND.into_response();
            };
            metric_stream(viewer, dashboard.shared.shutdown.clone())
        }
        "/api/inventory/stream" => {
            if !trusted_stream(headers, &dashboard).await {
                return StatusCode::FORBIDDEN.into_response();
            }
            inventory_stream(&dashboard.shared)
        }
        "/api/leases" => {
            if !dashboard.shared.lease_review_enabled {
                return StatusCode::NOT_FOUND.into_response();
            }
            let Ok((scope, before)) =
                upstream::lease_query(request.uri().query().unwrap_or_default())
            else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            let data = upstream::lease_page(
                &dashboard.private.join("lease-review.json"),
                &scope,
                before.as_deref(),
            )
            .await;
            match data {
                Ok(value) => json_response(Json(value).into_response()),
                Err(reason) => {
                    tracing::warn!(reason, "Lease history unavailable");
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
            }
        }
        "/api/inventory"
        | "/api/reconciliation"
        | "/api/machines"
        | "/api/services"
        | "/api/backups" => snapshot_response(&dashboard.shared, request.uri().path()),
        path if path.starts_with("/terminal/") => {
            if !trusted_stream(headers, &dashboard).await {
                return StatusCode::FORBIDDEN.into_response();
            }
            let machine = path.trim_start_matches("/terminal/").trim_end_matches('/');
            let launch = dashboard.shared.terminal.launch(
                &dashboard.private,
                dashboard.access_target.as_deref(),
                machine,
            );
            match launch.await {
                Ok(destination) => (
                    StatusCode::FOUND,
                    [
                        (header::LOCATION, destination.as_str()),
                        (header::CACHE_CONTROL, "no-store, private"),
                        (header::REFERRER_POLICY, "no-referrer"),
                    ],
                )
                    .into_response(),
                Err(reason) => {
                    tracing::warn!(reason, "Terminal unavailable");
                    StatusCode::SERVICE_UNAVAILABLE.into_response()
                }
            }
        }
        path => asset_response(&dashboard.shared, path).await,
    }
}

fn authorized(headers: &HeaderMap, expected: &HeaderValue) -> bool {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    values
        .next()
        .is_some_and(|value| bool::from(value.as_bytes().ct_eq(expected.as_bytes())))
        && values.next().is_none()
}

async fn read_origins(private: &Path) -> Result<HashSet<HeaderValue>, &'static str> {
    let origins: Vec<String> = upstream::read_json(&private.join("origins.json")).await?;
    origins
        .iter()
        .map(|origin| HeaderValue::from_str(origin).map_err(|_| "Invalid dashboard origin"))
        .collect()
}

async fn trusted_stream(headers: &HeaderMap, dashboard: &Dashboard) -> bool {
    if headers
        .get_all(SEC_FETCH_SITE)
        .iter()
        .any(|value| value == "cross-site")
    {
        return false;
    }
    let mut provided = headers.get_all(header::ORIGIN).iter();
    match (provided.next(), provided.next()) {
        (None, _) => true,
        (Some(value), None) => dashboard.allows_origin(value).await,
        _ => false,
    }
}

fn one_header<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<&'a HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

async fn perform_action(dashboard: Dashboard, request: Request) -> Response {
    let action = match request.uri().path_and_query().map(|value| value.as_str()) {
        Some("/api/copy-password") => "copy-password",
        Some("/api/reconcile") => "reconcile",
        Some("/api/refresh") => "refresh-catalog",
        _ => return StatusCode::FORBIDDEN.into_response(),
    };
    let headers = request.headers();
    let Some(origin) = one_header(headers, &header::ORIGIN) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if !dashboard.allows_origin(origin).await
        || !one_header(headers, &HeaderName::from_static("x-lab-action"))
            .is_some_and(|value| value == action)
        || !one_header(headers, &header::CONTENT_TYPE)
            .is_some_and(|value| value == "application/json")
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match action {
        "copy-password" => copy_password(&dashboard, request.into_body()).await,
        "reconcile" => dashboard.reconcile(),
        "refresh-catalog" => dashboard.refresh_catalog().await,
        _ => unreachable!("the path defines a supported action"),
    }
}

#[derive(Deserialize)]
struct PasswordRequest {
    machine: String,
    login: String,
}

async fn copy_password(dashboard: &Dashboard, body: Body) -> Response {
    let Ok(bytes) = to_bytes(body, 1024).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(request) = serde_json::from_slice::<PasswordRequest>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if let Some(target) = &dashboard.access_target {
        let records = dashboard.shared.latest.borrow();
        let allowed = &request.machine == target
            && records.machines.iter().any(|record| {
                record["id"].as_str() == Some(target.as_str())
                    && record["access"].as_array().is_some_and(|routes| {
                        routes.iter().any(|route| {
                            route["id"].as_str() == Some(request.login.as_str())
                                && route["password_available"] == true
                        })
                    })
            });
        if !allowed {
            return StatusCode::NOT_FOUND.into_response();
        }
    }
    let contents = match tokio::fs::read(dashboard.private.join("machine-passwords.json")).await {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return StatusCode::NOT_FOUND.into_response();
        }
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let Ok(allowlist) = serde_json::from_slice::<Value>(&contents) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Some(value) = allowlist[&request.machine][&request.login]
        .as_str()
        .filter(|value| !value.is_empty())
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CACHE_CONTROL, "no-store, private"),
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::PRAGMA, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        value.to_owned(),
    )
        .into_response()
}

fn standard_headers(response: &mut Response) {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_POLICY),
    );
}

fn snapshot_response(state: &Shared, path: &str) -> Response {
    let snapshot = state.latest.borrow().clone();
    let response = match path {
        "/api/inventory" => Json(InventoryPayload {
            snapshot: &snapshot,
            server_time: unix_time(),
            lease_review_enabled: Some(state.lease_review_enabled),
        })
        .into_response(),
        "/api/reconciliation" => Json(json!({
            "inventory_accuracy": snapshot.inventory_accuracy,
            "error": snapshot.error,
            "fetched_at": snapshot.fetched_at,
            "comparison": snapshot.comparison,
        }))
        .into_response(),
        _ => {
            let name = path.trim_start_matches("/api/");
            let records = match name {
                "machines" => &snapshot.machines,
                "services" => &snapshot.services,
                "backups" => &snapshot.backups,
                _ => unreachable!("only known inventory collection routes dispatch here"),
            };
            Json(json!({ name: records, "error": snapshot.error, "fetched_at": snapshot.fetched_at })).into_response()
        }
    };
    json_response(response)
}

fn json_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    standard_headers(&mut response);
    response
}

struct InventoryViewer(watch::Sender<usize>);
impl Drop for InventoryViewer {
    fn drop(&mut self) {
        self.0.send_modify(|count| *count -= 1);
    }
}

fn inventory_stream(state: &Shared) -> Response {
    let mut latest = state.latest.subscribe();
    let mut shutdown = state.shutdown.clone();
    state.viewers.send_modify(|count| *count += 1);
    let observer = InventoryViewer(state.viewers.clone());
    let events = async_stream::stream! {
        let _observer = observer;
        while !*shutdown.borrow() {
            let snapshot = latest.borrow_and_update().clone();
            let event = Event::default().event("inventory").json_data(InventoryPayload {
                snapshot: &snapshot,
                server_time: unix_time(),
                lease_review_enabled: None,
            });
            yield event;
            tokio::select! {
                biased;
                result = shutdown.changed() => {
                    if result.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                result = latest.changed() => {
                    if result.is_err() {
                        break;
                    }
                }
            }
        }
    };
    sse_response(events, Duration::from_secs(15))
}

fn metric_stream(mut viewer: collector::Viewer, mut shutdown: watch::Receiver<bool>) -> Response {
    let events = async_stream::stream! {
        while !*shutdown.borrow() && !*viewer.stopped.borrow() {
            let snapshot = viewer.readings.borrow_and_update().clone();
            yield Event::default().event("metrics").json_data(snapshot);
            tokio::select! {
                biased;
                _ = shutdown.wait_for(|stopping| *stopping) => break,
                _ = viewer.stopped.wait_for(|stopping| *stopping) => break,
                result = viewer.readings.changed() => {
                    if result.is_err() {
                        break;
                    }
                }
            }
        }
    };
    sse_response(events, Duration::from_secs(5))
}

fn sse_response<S>(events: S, interval: Duration) -> Response
where
    S: tokio_stream::Stream<Item = Result<Event, axum::Error>> + Send + 'static,
{
    let mut response = Sse::new(events)
        .keep_alive(KeepAlive::new().interval(interval).text("keepalive"))
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(X_ACCEL_BUFFERING, HeaderValue::from_static("no"));
    response
}

async fn asset_response(state: &Shared, path: &str) -> Response {
    let (file, content_type) = match path {
        "/" | "/leases" => ("index.html", "text/html; charset=utf-8"),
        "/app.css" | "/leases.css" | "/resource-graph.css" => {
            (&path[1..], "text/css; charset=utf-8")
        }
        "/dashboard-react.js" | "/vendor/elk-worker.min.js" => {
            (&path[1..], "text/javascript; charset=utf-8")
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let Ok(bytes) = tokio::fs::read(state.assets.join(file)).await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let mut response = Body::from(bytes).into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    standard_headers(&mut response);
    response
}
