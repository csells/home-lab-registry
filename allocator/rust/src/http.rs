use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use axum_server::{Handle, tls_rustls::RustlsConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;

use crate::{
    Controller, Provider, Rejected,
    tokens::{hash, valid_token},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RouteRole {
    Task,
    Review,
    Verifier,
    Alert,
    Recovery,
    Reservation,
    Issuer,
    Delegation,
}

impl RouteRole {
    pub fn for_path(parts: &[&str]) -> Self {
        match parts {
            ["v1", "lease-review"] => Self::Review,
            ["v1", "final-export"] | ["v1", "final-export", _] | ["v1", "checkpoint-expiry"] => {
                Self::Verifier
            }
            ["v1", "export-alerts"] => Self::Alert,
            [
                "v1",
                "redeem-checkpoint" | "redeem-upload" | "complete-upload",
            ] => Self::Recovery,
            ["v1", "capacity-reservations", ..] => Self::Reservation,
            ["v1", "task-credentials"] => Self::Issuer,
            ["v1", "delegated-renewal", _] => Self::Delegation,
            _ => Self::Task,
        }
    }
}

#[derive(Clone)]
pub struct Credentials(BTreeMap<RouteRole, Vec<(String, String)>>);

fn invalid_configuration() -> Rejected {
    Rejected::new(
        "configuration_invalid",
        "Check allocator listener and dedicated credential configuration.",
        503,
    )
}

impl Credentials {
    pub fn read(settings: &Value) -> Result<Self, Rejected> {
        let mut credentials: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let mut unique = BTreeSet::new();
        let mut register = |role, principal: &str, path: &Value| {
            if !valid_token(principal, 1, 64) {
                return Err(invalid_configuration());
            }
            let path = path.as_str().ok_or_else(invalid_configuration)?;
            let token = std::fs::read_to_string(path).map_err(|_| invalid_configuration())?;
            if !valid_token(token.trim(), 32, 128) {
                return Err(invalid_configuration());
            }
            let digest = hash(token.trim());
            if !unique.insert(digest.clone()) {
                return Err(invalid_configuration());
            }
            credentials
                .entry(role)
                .or_default()
                .push((principal.to_owned(), digest));
            Ok(())
        };
        for (role, key) in [
            (RouteRole::Task, "client_tokens"),
            (RouteRole::Reservation, "reservation_tokens"),
            (RouteRole::Issuer, "task_issuer_tokens"),
        ] {
            if let Some(map) = settings.get(key) {
                for (owner, token) in map.as_object().ok_or_else(invalid_configuration)? {
                    register(role, owner, token)?;
                }
            }
        }
        for (role, path) in [
            (RouteRole::Review, &settings["review_token_file"]),
            (
                RouteRole::Verifier,
                &settings["reclamation"]["verifier_token_file"],
            ),
            (
                RouteRole::Alert,
                &settings["reclamation"]["alert_token_file"],
            ),
            (
                RouteRole::Recovery,
                &settings["recovery"]["worker_token_file"],
            ),
        ] {
            if !path.is_null() {
                register(role, "worker", path)?;
            }
        }
        if !credentials.contains_key(&RouteRole::Task)
            || (credentials.contains_key(&RouteRole::Alert)
                && !credentials.contains_key(&RouteRole::Verifier))
        {
            return Err(invalid_configuration());
        }
        Ok(Self(credentials))
    }

    fn authorize(&self, role: RouteRole, secret: &str) -> Option<String> {
        let digest = hash(secret);
        let mut authorized = None;
        for (principal, expected) in self.0.get(&role)? {
            if bool::from(digest.as_bytes().ct_eq(expected.as_bytes())) {
                authorized = Some(principal.clone());
            }
        }
        authorized
    }
}

pub fn task_contract() -> Value {
    let text = json!({"type":"string","minLength":1});
    let short = json!({"type":"string","minLength":1,"maxLength":128});
    let positive = json!({"type":"integer","minimum":1});
    let duration = json!({"type":"integer","minimum":1,"maximum":31536000});
    let id = json!({"type":"string","pattern":"^[a-f0-9]{32}$"});
    json!({
        "api_version":1,
        "schema_dialect":"https://json-schema.org/draft/2020-12/schema",
        "capabilities":"/v1/capabilities",
        "operations":{
            "acquire":{"method":"POST","path":"/v1/leases","body_schema":"acquire"},
            "list":{"method":"GET","path":"/v1/leases"},
            "inspect":{"method":"GET","path":"/v1/leases/{id}"},
            "renew":{"method":"POST","path":"/v1/leases/{id}/renew","body_schema":"renew"},
            "release":{"method":"POST","path":"/v1/leases/{id}/release","body_schema":"release"}
        },
        "schemas":{
            "acquire":{"type":"object","required":["task","request_key","seconds","ssh_public_key"],
                "properties":{"task":short,"request_key":short,"seconds":duration,"ssh_public_key":text,
                    "recipe":text,"resume_from":id,"checkpoint_id":id,"reclaim_on_expiry":{"const":true},
                    "requirements":{"type":"object","additionalProperties":false,"properties":{
                        "memory_mib":positive,"disk_mib":positive,"vcpus":positive,
                        "gpu":text,"os":text,"architecture":text,"distribution":text,
                        "dependencies":{"type":"array","uniqueItems":true,"maxItems":16,
                            "items":{"type":"string","pattern":"^[a-z][a-z0-9-]{0,63}$"}}
                    }}
                },
                "dependentRequired":{"checkpoint_id":["resume_from"]}
            },
            "renew":{"type":"object","required":["seconds"],"properties":{"seconds":duration}},
            "release":{"type":"object","properties":{"preserve_checkpoint":{"const":true}}}
        }
    })
}

#[derive(Clone)]
struct HttpState {
    credentials: Arc<Credentials>,
    controller: Option<Arc<Mutex<Controller>>>,
}

pub fn router(credentials: Credentials, controller: Option<Arc<Mutex<Controller>>>) -> Router {
    Router::new().fallback(any(handler)).with_state(HttpState {
        credentials: Arc::new(credentials),
        controller,
    })
}

fn answer(status: StatusCode, data: Value) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(data)).into_response()
}

fn reject(error: Rejected) -> Response {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    answer(status, json!({"code":error.code(),"reason":error.reason()}))
}

fn invalid_body() -> Rejected {
    Rejected::new(
        "invalid_body",
        "Provide a JSON object up to 4096 bytes.",
        400,
    )
}

fn unavailable() -> Response {
    answer(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"code":"ledger_unavailable"}),
    )
}

fn unauthorized() -> Response {
    answer(StatusCode::UNAUTHORIZED, json!({"code":"unauthorized"}))
}

fn method_not_allowed() -> Response {
    answer(
        StatusCode::METHOD_NOT_ALLOWED,
        json!({"code":"method_not_allowed"}),
    )
}

async fn body(request: Request<Body>) -> Result<Value, Rejected> {
    const MAX: usize = 4096;
    let length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|length| (1..=MAX).contains(length))
        .ok_or_else(invalid_body)?;
    let bytes = tokio::time::timeout(Duration::from_secs(4), to_bytes(request.into_body(), MAX))
        .await
        .map_err(|_| invalid_body())?
        .map_err(|_| invalid_body())?;
    if bytes.len() != length {
        return Err(invalid_body());
    }
    let value = serde_json::from_slice::<Value>(&bytes)
        .map_err(|_| Rejected::new("invalid_body", "Provide valid JSON.", 400))?;
    if !value.is_object() {
        return Err(Rejected::new("invalid_body", "Provide a JSON object.", 400));
    }
    Ok(value)
}

fn review_query(query: &str) -> Option<(Option<String>, String)> {
    let mut before = None;
    let mut scope = None;
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "before" if before.is_none() => before = Some(value.into_owned()),
            "scope" if scope.is_none() => scope = Some(value.into_owned()),
            _ => return None,
        }
    }
    let scope = scope.unwrap_or_else(|| "all".to_owned());
    if scope == "inventory" && before.is_some() {
        return None;
    }
    Some((before, scope))
}

async fn handler(State(state): State<HttpState>, request: Request<Body>) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let parts: Vec<_> = path.trim_matches('/').split('/').collect();
    let query = request.uri().query().unwrap_or("").to_owned();
    let role = RouteRole::for_path(&parts);
    let mut headers = request.headers().get_all(header::AUTHORIZATION).iter();
    let secret = headers
        .next()
        .and_then(|header| header.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .unwrap_or("")
        .to_owned();
    if headers.next().is_some() {
        return unauthorized();
    }
    let owner = state.credentials.authorize(role, &secret);
    let mut temporary = false;
    let access = match (role, owner) {
        (RouteRole::Delegation, _) if method == Method::POST => secret.clone(),
        (_, Some(owner)) => owner,
        (RouteRole::Task, None) if valid_token(&secret, 43, 43) => {
            let Some(controller) = &state.controller else {
                return unauthorized();
            };
            match controller.lock().await.task_token(&secret) {
                Ok(Some(owner)) => {
                    temporary = true;
                    owner
                }
                Ok(None) => return unauthorized(),
                Err(error) => return reject(error),
            }
        }
        _ => return unauthorized(),
    };
    if !matches!(method, Method::GET | Method::POST)
        || role == RouteRole::Review && method != Method::GET
    {
        return method_not_allowed();
    }
    if method == Method::GET && parts == ["v1", "contract"] {
        return answer(StatusCode::OK, task_contract());
    }
    let data = if method == Method::POST {
        match body(request).await {
            Ok(data) => data,
            Err(error) => return reject(error),
        }
    } else {
        json!({})
    };
    let Some(controller) = state.controller else {
        return unavailable();
    };
    let mut controller = controller.lock_owned().await;
    tokio::task::spawn_blocking(move || {
        if temporary {
            match controller.task_token(&secret) {
                Ok(Some(owner)) if owner == access => {}
                Ok(_) => return unauthorized(),
                Err(error) => return reject(error),
            }
        }
        let parts: Vec<_> = path.trim_matches('/').split('/').collect();
        dispatch(&mut controller, method, &parts, &query, &access, &data)
    })
    .await
    .unwrap_or_else(|_| unavailable())
}

fn dispatch(
    controller: &mut Controller,
    method: Method,
    parts: &[&str],
    query: &str,
    owner: &str,
    data: &Value,
) -> Response {
    let mut status = StatusCode::OK;
    let result = match (method, parts) {
        (Method::GET, ["v1", "lease-review"]) => {
            let Some((before, scope)) = review_query(query) else {
                return answer(StatusCode::BAD_REQUEST, json!({"code":"invalid_query"}));
            };
            return match controller.review(before.as_deref(), &scope) {
                Ok(value) => answer(StatusCode::OK, value),
                Err(error) if error.status() == 400 => {
                    answer(StatusCode::BAD_REQUEST, json!({"code":error.code()}))
                }
                Err(error) => reject(error),
            };
        }
        (Method::GET, ["v1", "capabilities"]) => controller.capabilities(),
        (Method::GET, ["v1", "leases"]) => controller.list(owner),
        (Method::GET, ["v1", "leases", id]) => controller.inspect(id, owner),
        (Method::GET, ["v1", "capacity-reservations"]) => controller.reservations(owner),
        (Method::GET, ["v1", "export-alerts"]) => controller.export_alert(),
        (Method::GET, ["v1", "final-export"]) => controller.next_export(),
        (Method::GET, ["v1", "final-export", id]) => controller.pending_export(id),
        (Method::POST, ["v1", "final-export", id]) => controller.export_result(id, data),
        (Method::POST, ["v1", "export-alerts"]) => controller.acknowledge_alert(data),
        (Method::POST, ["v1", "task-credentials"]) => controller.issue(owner, data),
        (Method::POST, ["v1", "capacity-reservations"]) => controller.reserve(owner, data),
        (Method::POST, ["v1", "capacity-reservations", id, "release"]) => {
            controller.cancel_reservation(owner, id)
        }
        (Method::POST, ["v1", "delegated-renewal", id]) => controller.delegated_renew(id, owner),
        (Method::POST, ["v1", "redeem-checkpoint"]) => controller.redeem_recovery(data),
        (Method::POST, ["v1", "redeem-upload"]) => controller.redeem_upload(data),
        (Method::POST, ["v1", "complete-upload"]) => controller.complete_upload(data),
        (Method::POST, ["v1", "checkpoint-expiry"])
            if data.as_object().is_some_and(|fields| fields.is_empty()) =>
        {
            controller.claim_expiry()
        }
        (Method::POST, ["v1", "checkpoint-expiry"]) => controller.confirm_expiry(data),
        (Method::POST, ["v1", "leases"]) => {
            status = StatusCode::ACCEPTED;
            controller.acquire(data, owner)
        }
        (Method::POST, ["v1", "leases", id, "release"]) => {
            status = StatusCode::ACCEPTED;
            controller.release(id, owner, data)
        }
        (Method::POST, ["v1", "leases", id, "restore-complete"]) => {
            controller.finish_restore(id, owner, data)
        }
        (Method::POST, ["v1", "leases", id, "delegate-renewal"]) => {
            controller.delegate(id, owner, data)
        }
        (Method::POST, ["v1", "leases", id, "renew"]) => controller.renew(id, owner, data),
        (Method::POST, ["v1", "leases", id, "reviewer"]) => controller.reviewer(id, owner, data),
        (Method::POST, ["v1", "leases", id, "reviewer-hold"]) => {
            controller.hold(id, owner, data, true)
        }
        (Method::POST, ["v1", "leases", id, "override-reviewer"]) => {
            controller.override_reviewer(id, owner, data)
        }
        (Method::POST, ["v1", "leases", id, "hold"]) => controller.hold(id, owner, data, false),
        (Method::POST, ["v1", "leases", id, "export-disposition"]) => {
            controller.export_disposition(id, owner, data)
        }
        (Method::POST, ["v1", "leases", id, "retry-export"]) => controller.retry_export(id, owner),
        (Method::POST, ["v1", "leases", id, "checkpoint-recovery"]) => {
            controller.recovery_ticket(id, owner, data)
        }
        (Method::POST, ["v1", "leases", id, "checkpoint-upload"]) => {
            controller.grant_upload(id, owner, data)
        }
        (Method::POST, ["v1", "leases", id, "checkpoint-pin"]) => controller.pin(id, owner, data),
        _ => return answer(StatusCode::NOT_FOUND, json!({"code":"unknown_route"})),
    };
    match result {
        Ok(value) => answer(status, value),
        Err(error) => reject(error),
    }
}

fn tls(settings: &Value) -> Result<Option<RustlsConfig>, Box<dyn Error>> {
    if settings.is_null() {
        return Ok(None);
    }
    let cert = settings["cert_file"]
        .as_str()
        .ok_or_else(invalid_configuration)?;
    let key = settings["key_file"]
        .as_str()
        .ok_or_else(invalid_configuration)?;
    let certs = CertificateDer::pem_file_iter(cert)?.collect::<Result<_, _>>()?;
    let key = PrivateKeyDer::from_pem_file(key)?;
    let crypto = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(crypto)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(Some(RustlsConfig::from_config(Arc::new(config))))
}

pub async fn serve(config: PathBuf, adapter: Option<PathBuf>) -> Result<(), Box<dyn Error>> {
    let settings: Value = serde_json::from_slice(&std::fs::read(&config)?)?;
    let credentials = Credentials::read(&settings)?;
    let tls = tls(&settings["tls"])?;
    let address = settings
        .get("listen")
        .map_or(Some("127.0.0.1"), Value::as_str)
        .ok_or_else(invalid_configuration)?
        .parse::<IpAddr>()?;
    let port = settings
        .get("port")
        .map_or(Some(8093), Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(invalid_configuration)?;
    if tls.is_none() && address.to_string() != "127.0.0.1" {
        return Err(invalid_configuration().into());
    }
    let listener = std::net::TcpListener::bind(SocketAddr::new(address, port))?;
    let controller = tokio::task::spawn_blocking(move || {
        let provider = if let Some(adapter) = adapter {
            Provider::new(adapter, config)
        } else {
            Provider::native(&settings)?
        };
        Controller::open(&settings, provider)
    })
    .await??;
    let controller = Arc::new(Mutex::new(controller));
    let app = router(credentials, Some(controller.clone()));
    let handle = Handle::new();

    let shutdown = handle.clone();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let signals = tokio::spawn(async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
        let _ = stop.send(true);
        shutdown.graceful_shutdown(Some(Duration::from_secs(15)));
    });
    let checks = tokio::spawn(async move {
        let mut last_warning = None;
        let mut timer = tokio::time::interval(Duration::from_secs(2));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = stopped.changed() => break,
                _ = timer.tick() => {
                    let Ok(mut controller) = controller.clone().try_lock_owned() else { continue };
                    match tokio::task::spawn_blocking(move || controller.advance()).await {
                        Ok(Ok(())) => last_warning = None,
                        Ok(Err(error)) if last_warning.as_deref() != Some(error.code()) => {
                            tracing::warn!(code=error.code(), "Reclamation held for review");
                            last_warning = Some(error.code().to_owned());
                        }
                        Ok(Err(_)) => {}
                        Err(_) => {
                            tracing::error!("Reclamation worker stopped unexpectedly");
                            break;
                        }
                    }
                }
            }
        }
    });
    tracing::info!(tls = tls.is_some(), "Lease allocator listening");
    let result = match tls {
        Some(tls) => {
            axum_server::from_tcp_rustls(listener, tls)?
                .handle(handle)
                .serve(app.into_make_service())
                .await
        }
        None => {
            axum_server::from_tcp(listener)?
                .handle(handle)
                .serve(app.into_make_service())
                .await
        }
    };
    signals.abort();
    checks.abort();
    result?;
    Ok(())
}
