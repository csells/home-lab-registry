use std::{
    collections::HashMap,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::http::{Uri, header};
use reqwest::{
    Certificate, Client, ClientBuilder, Identity, Response, StatusCode, Url, redirect::Policy,
    tls::Version,
};
use rustls::{
    ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject},
};
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
    certificate_ip: Option<IpAddr>,
}

impl Target {
    pub fn parse(endpoint: &str, certificate_name: Option<&str>) -> Result<Self, &'static str> {
        Self::parse_url(endpoint, certificate_name, false)
    }

    pub(crate) fn guest(endpoint: &str, certificate_name: &str) -> Result<Self, &'static str> {
        Self::parse_url(endpoint, Some(certificate_name), true)
    }

    fn parse_url(
        endpoint: &str,
        certificate_name: Option<&str>,
        guest_override: bool,
    ) -> Result<Self, &'static str> {
        let uri: Uri = endpoint.parse().map_err(|_| "Invalid collector address")?;
        let mut https = Url::parse(endpoint).map_err(|_| "Invalid collector URL")?;
        let host = https.host_str().ok_or("Missing collector hostname")?;
        let dial_host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = uri
            .port_u16()
            .or_else(|| guest_override.then_some(443))
            .filter(|port| *port != 0)
            .ok_or("Missing collector port")?;
        if https.scheme() != "https"
            || https.query().is_some()
            || https.fragment().is_some()
            || !https.username().is_empty()
            || https.password().is_some()
            || uri
                .authority()
                .is_some_and(|authority| authority.as_str().contains('@'))
            || (!guest_override && (uri.path() != "/stream" || https.path() != "/stream"))
        {
            return Err("Invalid collector address");
        }
        let certificate_ip = certificate_name.and_then(|name| name.parse::<IpAddr>().ok());
        if let Some(identity) = certificate_name.filter(|_| certificate_ip.is_none()) {
            let first_is_valid = identity
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric);
            if !first_is_valid
                || identity.len() > 253
                || !identity
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
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
            certificate_ip,
        })
    }

    pub(crate) async fn open(
        &self,
        private: &Path,
        path: &str,
        accept: &'static str,
        read_timeout: Duration,
    ) -> Result<Response, OpenError> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (roots, cert, key) = tokio::try_join!(
            tokio::fs::read(private.join("stream-ca.pem")),
            tokio::fs::read(private.join("stream-client.pem")),
            tokio::fs::read(private.join("stream-client-key.pem")),
        )
        .map_err(|_| "Collector credentials are unavailable")?;
        let mut builder = Client::builder()
            .no_proxy()
            .https_only(true)
            .redirect(Policy::none())
            .connect_timeout(CONNECTION_TIMEOUT)
            .read_timeout(read_timeout);
        builder = match self.certificate_ip {
            Some(ip) => ip_identity(builder, &roots, &cert, &key, ip)?,
            None => {
                let roots =
                    Certificate::from_pem_bundle(&roots).map_err(|_| "Invalid collector CA")?;
                let mut identity_pem = cert;
                identity_pem.push(b'\n');
                identity_pem.extend_from_slice(&key);
                let identity = Identity::from_pem(&identity_pem)
                    .map_err(|_| "Invalid collector client identity")?;
                builder
                    .tls_backend_rustls()
                    .tls_version_min(Version::TLS_1_3)
                    .tls_certs_only(roots)
                    .identity(identity)
            }
        };
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
                return Err("Collector address lookup returned no addresses".into());
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
            return Err(OpenError::Http(response.status()));
        }
        if response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_none_or(|content| !content.trim().eq_ignore_ascii_case(accept))
        {
            return Err("Unexpected collector response type".into());
        }
        Ok(response)
    }
}

#[derive(Debug)]
pub(crate) enum OpenError {
    Transport(&'static str),
    Http(StatusCode),
}

impl OpenError {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::Transport(reason) => reason,
            Self::Http(_) => "Unexpected collector HTTP status",
        }
    }
}

impl From<&'static str> for OpenError {
    fn from(reason: &'static str) -> Self {
        Self::Transport(reason)
    }
}

fn ip_identity(
    builder: ClientBuilder,
    roots: &[u8],
    cert: &[u8],
    key: &[u8],
    ip: IpAddr,
) -> Result<ClientBuilder, &'static str> {
    let mut store = RootCertStore::empty();
    for certificate in CertificateDer::pem_slice_iter(roots) {
        store
            .add(certificate.map_err(|_| "Invalid collector CA")?)
            .map_err(|_| "Invalid collector CA")?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = WebPkiServerVerifier::builder_with_provider(Arc::new(store), provider.clone())
        .build()
        .map_err(|_| "Invalid collector CA")?;
    let certificates = CertificateDer::pem_slice_iter(cert)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid collector client certificate")?;
    let key = PrivateKeyDer::from_pem_slice(key).map_err(|_| "Invalid collector client key")?;
    let mut tls = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| "Unable to configure collector TLS")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(IpIdentity {
            verifier,
            ip: ServerName::IpAddress(ip.into()),
        }))
        .with_client_auth_cert(certificates, key)
        .map_err(|_| "Invalid collector client identity")?;
    // The dial host can be DNS, but this collector's configured TLS identity is an IP.
    tls.enable_sni = false;
    Ok(builder.use_preconfigured_tls(tls))
}

#[derive(Debug)]
struct IpIdentity {
    verifier: Arc<WebPkiServerVerifier>,
    ip: ServerName<'static>,
}

impl ServerCertVerifier for IpIdentity {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.verifier
            .verify_server_cert(end_entity, intermediates, &self.ip, ocsp, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier
            .verify_tls12_signature(message, cert, signature)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier
            .verify_tls13_signature(message, cert, signature)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.verifier.supported_verify_schemes()
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
        .await
        .map_err(|error| error.reason())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::{ServerConfig, ServerConnection, StreamOwned, server::WebPkiClientVerifier};
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        process::Command,
    };

    #[tokio::test]
    async fn an_ip_san_is_verified_independently_of_the_dial_host() {
        let root = tempfile::tempdir().unwrap();
        let cert = root.path().join("stream-client.pem");
        let key = root.path().join("stream-client-key.pem");
        let command = Command::new("openssl")
            .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout"])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .args([
                "-days",
                "1",
                "-subj",
                "/CN=collector",
                "-addext",
                "subjectAltName=IP:192.0.2.13",
                "-addext",
                "basicConstraints=critical,CA:FALSE",
            ])
            .output()
            .unwrap();
        assert!(
            command.status.success(),
            "{}",
            String::from_utf8_lossy(&command.stderr)
        );
        std::fs::copy(&cert, root.path().join("stream-ca.pem")).unwrap();

        let certificate = CertificateDer::from_pem_file(&cert).unwrap();
        let mut client_roots = RootCertStore::empty();
        client_roots.add(certificate.clone()).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier =
            WebPkiClientVerifier::builder_with_provider(Arc::new(client_roots), provider.clone())
                .build()
                .unwrap();
        let tls = Arc::new(
            ServerConfig::builder_with_provider(provider)
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_client_cert_verifier(verifier)
                .with_single_cert(
                    vec![certificate],
                    PrivateKeyDer::from_pem_file(key).unwrap(),
                )
                .unwrap(),
        );

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = format!("https://{}/stream", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            for status in [200, 200, 403] {
                let (tcp, _) = listener.accept().unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let stream = StreamOwned::new(ServerConnection::new(tls.clone()).unwrap(), tcp);
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                if stream.read_line(&mut line).is_err() {
                    continue;
                }
                assert!(line.starts_with("GET /guests/stream "));
                loop {
                    line.clear();
                    if stream.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                }
                write!(stream.get_mut(), "HTTP/1.1 {status} fixture\r\nContent-Type: application/x-ndjson\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
        });

        for (ip, expected) in [
            ("192.0.2.13", Some(200)),
            ("192.0.2.14", None),
            ("192.0.2.13", Some(403)),
        ] {
            let target = Target::parse(&endpoint, Some(ip)).unwrap();
            let result = target
                .open(
                    root.path(),
                    "/guests/stream",
                    "application/x-ndjson",
                    Duration::from_secs(3),
                )
                .await;
            match expected {
                Some(200) => assert!(result.is_ok(), "{result:?}"),
                Some(403) => assert!(
                    matches!(result, Err(OpenError::Http(StatusCode::FORBIDDEN))),
                    "{result:?}"
                ),
                _ => assert!(
                    matches!(
                        result,
                        Err(OpenError::Transport("Collector connection failed"))
                    ),
                    "{result:?}"
                ),
            }
        }
        worker.join().unwrap();
    }
}
