use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::http::{Uri, header};
use reqwest::{Certificate, Client, Identity, Response, Url, redirect::Policy, tls::Version};
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::{
    metrics::{Frames, History},
    unix_time,
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub(crate) dial_host: String,
    pub(crate) port: u16,
    pub(crate) https: Url,
}

impl Target {
    pub fn parse(endpoint: &str, certificate_name: Option<&str>) -> Result<Self, &'static str> {
        let uri: Uri = endpoint.parse().map_err(|_| "Invalid collector address")?;
        let mut https = Url::parse(endpoint).map_err(|_| "Invalid collector URL")?;
        let host = https.host_str().ok_or("Missing collector hostname")?;
        let dial_host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = uri
            .port_u16()
            .filter(|port| *port != 0)
            .ok_or("Missing collector port")?;
        if https.scheme() != "https"
            || https.path() != "/stream"
            || https.query().is_some()
            || https.fragment().is_some()
            || !https.username().is_empty()
            || https.password().is_some()
            || uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
            || uri.path() != "/stream"
        {
            return Err("Invalid collector address");
        }
        if let Some(identity) = certificate_name {
            let first_is_valid = identity
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric);
            if !first_is_valid
                || identity.len() > 253
                || !identity
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
                || identity.parse::<IpAddr>().is_ok()
            {
                return Err("Collector identity must be a DNS name");
            }
            https
                .set_host(Some(identity))
                .map_err(|_| "Invalid collector identity")?;
            // WHATWG URLs also recognize numbers and non-canonical IPv4 spellings.
            if https
                .host_str()
                .is_some_and(|name| name.parse::<IpAddr>().is_ok())
            {
                return Err("Collector identity must be a DNS name");
            }
        }
        Ok(Self {
            dial_host,
            port,
            https,
        })
    }

    pub async fn open(
        &self,
        private: &Path,
        path: &str,
        accept: &'static str,
        read_timeout: Duration,
    ) -> Result<Response, &'static str> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (roots, cert, key) = tokio::try_join!(
            tokio::fs::read(private.join("stream-ca.pem")),
            tokio::fs::read(private.join("stream-client.pem")),
            tokio::fs::read(private.join("stream-client-key.pem")),
        )
        .map_err(|_| "Collector credentials are unavailable")?;
        let roots = Certificate::from_pem_bundle(&roots).map_err(|_| "Invalid collector CA")?;
        let mut identity_pem = cert;
        identity_pem.push(b'\n');
        identity_pem.extend_from_slice(&key);
        let identity =
            Identity::from_pem(&identity_pem).map_err(|_| "Invalid collector client identity")?;
        let mut builder = Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(Policy::none())
            .tls_version_min(Version::TLS_1_3)
            .tls_certs_only(roots)
            .identity(identity)
            .connect_timeout(CONNECTION_TIMEOUT)
            .read_timeout(read_timeout);
        let certificate_host = self.https.host_str().expect("validated collector host");
        if certificate_host != self.dial_host && certificate_host != format!("[{}]", self.dial_host)
        {
            let addresses = tokio::time::timeout(
                CONNECTION_TIMEOUT,
                tokio::net::lookup_host((self.dial_host.as_str(), self.port)),
            )
            .await
            .map_err(|_| "Collector address lookup timed out")?
            .map_err(|_| "Collector address lookup failed")?
            .collect::<Vec<_>>();
            if addresses.is_empty() {
                return Err("Collector address lookup returned no addresses");
            }
            builder = builder.resolve_to_addrs(certificate_host, &addresses);
        }
        let client = builder
            .build()
            .map_err(|_| "Unable to configure collector TLS")?;
        let mut endpoint = self.https.clone();
        endpoint.set_path(path);
        let response = client
            .get(endpoint)
            .header(header::ACCEPT, accept)
            .send()
            .await
            .map_err(|_| "Collector connection failed")?;
        if response.status() != reqwest::StatusCode::OK {
            return Err("Unexpected collector HTTP status");
        }
        if response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_none_or(|content| !content.trim().eq_ignore_ascii_case(accept))
        {
            return Err("Unexpected collector response type");
        }
        Ok(response)
    }
}

#[derive(Clone)]
pub struct LiveMetrics(Arc<Shared>);

struct Shared {
    private: PathBuf,
    state: Mutex<MetricsState>,
}

#[derive(Default)]
struct MetricsState {
    targets: HashMap<String, Target>,
    streams: HashMap<String, Arc<Active>>,
}

struct Active {
    latest: watch::Sender<Value>,
    stop: watch::Sender<bool>,
    count: Mutex<usize>,
}

pub struct Viewer {
    id: String,
    owner: LiveMetrics,
    active: Arc<Active>,
    pub readings: watch::Receiver<Value>,
    pub(crate) stopped: watch::Receiver<bool>,
}

impl LiveMetrics {
    pub fn new(private: PathBuf) -> Self {
        Self(Arc::new(Shared {
            private,
            state: Mutex::new(MetricsState::default()),
        }))
    }

    pub fn configure(&self, targets: HashMap<String, Target>) {
        let mut state = self.0.state.lock().expect("metrics configuration lock");
        let previous = std::mem::replace(&mut state.targets, targets.clone());
        state.streams.retain(|id, stream| {
            let retain = targets
                .get(id)
                .is_some_and(|target| previous.get(id) == Some(target));
            if !retain {
                stream.stop.send_replace(true);
            }
            retain
        });
    }

    pub fn readings(&self) -> HashMap<String, Value> {
        let state = self.0.state.lock().expect("metrics configuration lock");
        state
            .targets
            .keys()
            .map(|id| {
                (
                    id.clone(),
                    state
                        .streams
                        .get(id)
                        .map(|stream| stream.latest.borrow().clone())
                        .unwrap_or_else(|| json!({"status":"waiting"})),
                )
            })
            .collect()
    }

    pub fn subscribe(&self, id: &str) -> Option<Viewer> {
        let mut state = self.0.state.lock().expect("metrics configuration lock");
        let target = state.targets.get(id)?.clone();
        let active = state.streams.entry(id.into()).or_insert_with(|| {
            let active = Arc::new(Active {
                latest: watch::Sender::new(json!({"status":"unavailable","error":"Connecting to collector","history":[],"interval_seconds":5})),
                stop: watch::Sender::new(false),
                count: Mutex::new(0),
            });
            tokio::spawn(run(target, self.0.private.clone(), Arc::clone(&active)));
            active
        });
        *active.count.lock().expect("metrics subscribers lock") += 1;
        Some(Viewer {
            id: id.into(),
            owner: self.clone(),
            active: Arc::clone(active),
            readings: active.latest.subscribe(),
            stopped: active.stop.subscribe(),
        })
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        let mut state = self
            .owner
            .0
            .state
            .lock()
            .expect("metrics configuration lock");
        let mut readers = self.active.count.lock().expect("metrics subscribers lock");
        *readers -= 1;
        if *readers == 0 {
            self.active.stop.send_replace(true);
            if state
                .streams
                .get(&self.id)
                .is_some_and(|stream| Arc::ptr_eq(stream, &self.active))
            {
                state.streams.remove(&self.id);
            }
        }
    }
}

async fn run(target: Target, private: PathBuf, stream: Arc<Active>) {
    let mut stop = stream.stop.subscribe();
    let mut history = History::default();
    let mut delay = Duration::from_secs(1);
    loop {
        let collected = tokio::select! {
            _ = stop.wait_for(|stop| *stop) => break,
            result = read_stream(&target, &private, &mut history, &stream.latest, &mut delay) => result,
        };
        if let Err(reason) = collected {
            tracing::warn!(reason, "Metrics stream disconnected while subscribed");
        }
        history.disconnected();
        stream.latest.send_replace(history.reading(unix_time()));
        tokio::select! {
            _ = stop.wait_for(|stop| *stop) => break,
            _ = tokio::time::sleep(delay) => {},
        }
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

async fn read_stream(
    target: &Target,
    private: &Path,
    history: &mut History,
    latest: &watch::Sender<Value>,
    retry: &mut Duration,
) -> Result<(), &'static str> {
    let mut response = target
        .open(
            private,
            "/stream",
            "application/x-ndjson",
            CONNECTION_TIMEOUT,
        )
        .await?;
    let mut frames = Frames::default();
    let mut last_frame = tokio::time::Instant::now();
    loop {
        let chunk = tokio::time::timeout_at(last_frame + CONNECTION_TIMEOUT, response.chunk())
            .await
            .map_err(|_| "Collector frame timed out")?
            .map_err(|_| "Collector stream interrupted")?
            .ok_or("Collector stream closed")?;
        frames.receive(&chunk, |frame| {
            if history.apply(frame)? {
                latest.send_replace(history.reading(unix_time()));
            }
            last_frame = tokio::time::Instant::now();
            *retry = Duration::from_secs(1);
            Ok(())
        })?;
    }
}
