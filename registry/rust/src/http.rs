use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::{Semaphore, watch};

use crate::{Catalog, Problem, enrollment::Enrolled, validation::valid_id};

#[derive(Clone)]
pub struct RegistryHttp(Arc<Shared>, Arc<Enrolled>);

pub(crate) struct Shared {
    pub(crate) catalog: Catalog,
    reader: HeaderValue,
    writer: HeaderValue,
    changes: Changes,
    shutdown: watch::Receiver<bool>,
}

struct Changes {
    epoch: String,
    state: Mutex<Journal>,
    updated: watch::Sender<u64>,
    permits: Arc<Semaphore>,
}

#[derive(Default)]
struct Journal {
    version: u64,
    recent: VecDeque<(u64, Option<String>)>,
}

impl Default for Changes {
    fn default() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4().simple().to_string(),
            state: Mutex::new(Journal::default()),
            updated: watch::Sender::new(0),
            permits: Arc::new(Semaphore::new(16)),
        }
    }
}

impl Changes {
    fn committed(&self, health_id: Option<String>) {
        let mut journal = self.state.lock().expect("registry change journal");
        journal.version += 1;
        let version = journal.version;
        if journal.recent.len() == 128 {
            journal.recent.pop_front();
        }
        journal.recent.push_back((version, health_id));
        self.updated.send_replace(version);
    }

    fn cursor(&self, header: Option<&str>) -> Result<Option<u64>, Problem> {
        let Some(header) = header else {
            return Ok(None);
        };
        let (epoch, version) = header
            .split_once(':')
            .ok_or_else(|| Problem::new(400, "Invalid catalog change cursor"))?;
        let valid = epoch.len() == 32
            && epoch
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
            && (1..=19).contains(&version.len())
            && (version == "0" || !version.starts_with('0'))
            && version.bytes().all(|byte| byte.is_ascii_digit());
        let version = version
            .parse::<u64>()
            .ok()
            .filter(|_| valid)
            .ok_or_else(|| Problem::new(400, "Invalid catalog change cursor"))?;
        Ok((epoch == self.epoch).then_some(version))
    }

    fn next(&self, last: Option<u64>, health: bool) -> (u64, Value) {
        let journal = self.state.lock().expect("registry change journal");
        let current = journal.version;
        if last == Some(current) {
            return (current, json!({"heartbeat":true}));
        }
        let mut event = json!({"epoch":self.epoch,"version":current});
        if let Some(last) = last.filter(|last| health && *last < current)
            && journal
                .recent
                .front()
                .is_some_and(|(first, _)| *first <= last + 1)
        {
            let changed: Option<BTreeSet<&str>> = journal
                .recent
                .iter()
                .filter(|(sequence, _)| *sequence > last)
                .map(|(_, id)| id.as_deref())
                .collect();
            if let Some(changed) = changed.filter(|ids| !ids.is_empty()) {
                event["health"] = json!(changed);
            }
        }
        (current, event)
    }
}

impl RegistryHttp {
    pub fn new(
        catalog: Catalog,
        reader: &str,
        writer: &str,
        shutdown: watch::Receiver<bool>,
    ) -> Result<Self, Problem> {
        if reader.is_empty() || writer.is_empty() || reader == writer {
            return Err(Problem::new(
                500,
                "Registry credentials require distinct reader and writer tokens",
            ));
        }
        Ok(Self(
            Arc::new(Shared {
                catalog,
                reader: token_header(reader)?,
                writer: token_header(writer)?,
                shutdown,
                changes: Changes::default(),
            }),
            Arc::new(Enrolled::default()),
        ))
    }

    pub(crate) fn with_enrollment(mut self, enrolled: Enrolled) -> Self {
        self.1 = Arc::new(enrolled);
        self
    }

    pub fn router(&self) -> Router {
        Router::new().fallback(handler).with_state(self.clone())
    }

    pub(crate) fn committed(&self, health_id: Option<String>) {
        self.0.changes.committed(health_id);
    }

    fn stream(&self, headers: &HeaderMap, health: bool) -> Result<Response, Problem> {
        let cursor = headers
            .get("Last-Event-ID")
            .map(|header| header.to_str())
            .transpose()
            .map_err(|_| Problem::new(400, "Invalid catalog change cursor"))?;
        let mut last = self.0.changes.cursor(cursor)?;
        let permit = self
            .0
            .changes
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| Problem::new(503, "Catalog change subscription limit reached"))?;
        let state = self.0.clone();
        let mut stop = state.shutdown.clone();
        let mut updated = state.changes.updated.subscribe();
        let body = async_stream::stream! {
            let _permit = permit;
            while !*stop.borrow() {
                let _ = updated.borrow_and_update();
                let (version, event) = state.changes.next(last, health);
                let mut bytes = serde_json::to_vec(&event).expect("catalog change JSON");
                bytes.push(b'\n');
                last = Some(version);
                yield Ok::<Bytes, Infallible>(Bytes::from(bytes));
                tokio::select! {
                    _ = stop.wait_for(|stopping| *stopping) => break,
                    _ = tokio::time::timeout(Duration::from_secs(15), updated.changed()) => {},
                }
            }
        };
        Ok((
            [
                (header::CONTENT_TYPE, "application/x-ndjson"),
                (header::CACHE_CONTROL, "no-store"),
                (header::HeaderName::from_static("x-accel-buffering"), "no"),
            ],
            Body::from_stream(body),
        )
            .into_response())
    }
}

pub(crate) fn token_header(token: &str) -> Result<HeaderValue, Problem> {
    let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| Problem::new(500, "Invalid registry credential configuration"))?;
    header.set_sensitive(true);
    Ok(header)
}

pub(crate) fn new_auth(headers: &HeaderMap) -> Option<&HeaderValue> {
    let mut auth = headers.get_all(header::AUTHORIZATION).iter();
    let first = auth.next()?;
    auth.next().is_none().then_some(first)
}

pub(crate) fn same_auth(provided: Option<&HeaderValue>, stored: &HeaderValue) -> bool {
    provided.is_some_and(|header| bool::from(header.as_bytes().ct_eq(stored.as_bytes())))
}

pub(crate) async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, Problem> + Send + 'static,
) -> Result<T, Problem> {
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|_| Problem::new(500, "Internal error"))?
}

fn reply(status: u16, value: Value) -> Response {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (status, [(header::CACHE_CONTROL, "no-store")], Json(value)).into_response()
}

async fn handler(State(state): State<RegistryHttp>, request: Request) -> Response {
    let result = handle(state, request).await;
    let mut response = match result {
        Ok(response) => response,
        Err(error) => reply(error.status(), json!({"error":error.message()})),
    };
    response
        .headers_mut()
        .insert(header::SERVER, HeaderValue::from_static("LabRegistry/1"));
    response
}

async fn handle(state: RegistryHttp, request: Request) -> Result<Response, Problem> {
    let method = request.method();
    let path = request.uri().path();
    if *method == Method::GET && path == "/healthz" {
        return Ok(reply(200, json!({"ok":true})));
    }
    let auth = new_auth(request.headers());
    if *method == Method::POST {
        let parts: Vec<_> = path.trim_matches('/').split('/').collect();
        if let ["v1", collection, id, action] = parts.as_slice()
            && valid_id(id)
        {
            let id = id.to_string();
            if *collection == "machines" && *action == "presence" {
                let auth = auth.cloned();
                let (report, changed) = state
                    .1
                    .announcement(state.0.catalog.clone(), &id, auth.as_ref(), body(request))
                    .await?;
                if changed {
                    state.committed(None);
                }
                return Ok(reply(200, report));
            }
            if *collection == "services" && *action == "runtime" {
                let binding = state
                    .1
                    .runtime(&id, auth)
                    .ok_or_else(|| Problem::new(401, "Enrolled runtime publisher required"))?;
                let value = body(request).await?;
                let db = state.0.catalog.clone();
                let report = blocking(move || {
                    let (report, changed) = db.report_runtime(&id, &value, &binding)?;
                    if changed {
                        state.committed(None);
                    }
                    Ok(report)
                })
                .await?;
                return Ok(reply(200, report));
            }
        }
    }
    let writer = same_auth(auth, &state.0.writer);
    let reader = same_auth(auth, &state.0.reader);
    if !writer && !reader {
        return Err(Problem::new(401, "Authentication required"));
    }
    if *method != Method::GET && !writer {
        return Err(Problem::new(403, "Write credential required"));
    }
    let mut query = BTreeMap::<String, Vec<String>>::new();
    for (key, value) in form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes())
    {
        if !value.is_empty() {
            query
                .entry(key.into_owned())
                .or_default()
                .push(value.into_owned());
        }
    }
    let catalog = state.0.catalog.clone();
    if *method == Method::GET {
        match path {
            "/v1/export" => return Ok(reply(200, blocking(move || catalog.export()).await?)),
            "/v1/services" => {
                return Ok(reply(
                    200,
                    json!({"services":blocking(move || catalog.listing(&query)).await?}),
                ));
            }
            "/v1/catalog/changes" => {
                let health = query.get("health").is_some_and(|values| values == &["1"]);
                return state.stream(request.headers(), health);
            }
            "/v1/health" => {
                let ids = query
                    .remove("id")
                    .filter(|ids| {
                        (1..=128).contains(&ids.len()) && ids.iter().all(|id| valid_id(id))
                    })
                    .ok_or_else(|| Problem::new(400, "Supply 1 to 128 registry record IDs"))?;
                return Ok(reply(
                    200,
                    json!({"health":blocking(move || catalog.health(&ids)).await?}),
                ));
            }
            _ => {}
        }
    }
    let elements: Vec<_> = path.trim_matches('/').split('/').collect();
    match elements.as_slice() {
        ["v1", "services", id] if valid_id(id) && *method == Method::GET => {
            let id = id.to_string();
            Ok(reply(200, blocking(move || catalog.get(&id)).await?))
        }
        ["v1", "services", id, "history"] if valid_id(id) && *method == Method::GET => {
            let id = id.to_string();
            Ok(reply(
                200,
                json!({"history":blocking(move || catalog.history(&id)).await?}),
            ))
        }
        ["v1", "services", id] if valid_id(id) && *method == Method::PUT => {
            let id = id.to_string();
            let revision = revision(&request, true)?
                .ok_or_else(|| Problem::new(428, "If-Match revision required; use 0 to create"))?;
            let json = body(request).await?;
            let updated = blocking(move || {
                let value = catalog.put(&id, json, revision)?;
                state.committed(None);
                Ok(value)
            })
            .await?;
            Ok(reply(if revision == 0 { 201 } else { 200 }, updated))
        }
        ["v1", "services", id, "observe"] if valid_id(id) && *method == Method::POST => {
            let id = id.to_string();
            let revision = revision(&request, false)?;
            let json = body(request).await?;
            let target = id.clone();
            let updated = blocking(move || {
                let value = catalog.observe(&id, json, revision)?;
                state.committed(Some(target));
                Ok(value)
            })
            .await?;
            Ok(reply(200, updated))
        }
        _ => Err(Problem::new(404, "Unknown route")),
    }
}

fn revision(request: &Request, quoted: bool) -> Result<Option<i64>, Problem> {
    request
        .headers()
        .get(header::IF_MATCH)
        .map(|revision| {
            let revision = revision
                .to_str()
                .map_err(|_| Problem::new(400, "Invalid revision"))?;
            let revision = if quoted {
                revision.trim_matches('"')
            } else {
                revision
            };
            revision
                .trim()
                .parse::<i64>()
                .map_err(|_| Problem::new(400, "Invalid revision"))
        })
        .transpose()
}

pub(crate) async fn body(request: Request) -> Result<Value, Problem> {
    const LIMIT: usize = 65536;
    let length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|text| text.trim().parse::<i64>().ok())
                .ok_or_else(|| Problem::new(400, "Invalid content length"))
        })
        .transpose()?
        .unwrap_or(0);
    if !(1..=LIMIT as i64).contains(&length) {
        return Err(Problem::new(413, "Body must be 1..65536 bytes"));
    }
    let bytes = to_bytes(request.into_body(), LIMIT)
        .await
        .map_err(|_| Problem::new(400, "Invalid JSON"))?;
    if bytes.len() != length as usize {
        return Err(Problem::new(400, "Invalid JSON"));
    }
    serde_json::from_slice(&bytes).map_err(|_| Problem::new(400, "Invalid JSON"))
}
