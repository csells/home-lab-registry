use std::{
    error::Error,
    fmt,
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::{
    tokens::{hex, valid_token},
    workspace::{MAX_ARCHIVE, digest_to},
};

#[derive(Debug)]
pub struct TaskError {
    pub response: Value,
    pub status: Option<u16>,
}

impl TaskError {
    pub fn client(reason: impl Into<String>) -> Self {
        Self {
            response: json!({"code":"client_error","reason":reason.into()}),
            status: None,
        }
    }

    pub fn exit_code(&self) -> u8 {
        if self
            .status
            .is_some_and(|status| (400..500).contains(&status))
        {
            2
        } else {
            1
        }
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            self.response["reason"]
                .as_str()
                .or_else(|| self.response["code"].as_str())
                .unwrap_or("Allocator request failed"),
        )
    }
}
impl Error for TaskError {}

impl From<io::Error> for TaskError {
    fn from(error: io::Error) -> Self {
        Self::client(error.to_string())
    }
}

impl From<serde_json::Error> for TaskError {
    fn from(_: serde_json::Error) -> Self {
        Self::client("The input or allocator response was not valid JSON")
    }
}

pub fn verified_origin(endpoint: &str) -> Result<String, TaskError> {
    let invalid =
        || TaskError::client("Use the registered HTTPS endpoint with an independently obtained CA");
    let url = Url::parse(endpoint).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host().is_none()
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn endpoint_from_record(entry: &Value) -> Result<String, TaskError> {
    let spec = &entry["spec"];
    if spec["lifecycle"] != "active"
        || !spec["capabilities"]
            .as_array()
            .is_some_and(|caps| caps.iter().any(|cap| cap == "lease-pilot"))
    {
        return Err(TaskError::client(
            "Record does not describe an active lease pilot",
        ));
    }
    let endpoint = spec["endpoints"]["lease_api"]
        .as_str()
        .ok_or_else(|| TaskError::client("No registered lease API endpoint"))?;
    verified_origin(endpoint)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionManifest {
    version: u64,
    endpoint: String,
    expires_at: f64,
}

pub struct TaskSession {
    pub endpoint: String,
    pub ca: PathBuf,
    pub token: PathBuf,
    pub identity: PathBuf,
}

impl TaskSession {
    pub fn load(path: &Path, now: f64) -> Result<Self, TaskError> {
        let manifest: SessionManifest = serde_json::from_reader(File::open(path)?)?;
        if manifest.version != 1 || !manifest.expires_at.is_finite() {
            return Err(TaskError::client("Invalid launcher session manifest"));
        }
        if now >= manifest.expires_at {
            return Err(TaskError::client(
                "Task credential expired. Reconnect through the launcher.",
            ));
        }
        let directory = path
            .parent()
            .ok_or_else(|| TaskError::client("Invalid session path"))?;
        Ok(Self {
            endpoint: verified_origin(&manifest.endpoint)?,
            ca: directory.join("ca.pem"),
            token: directory.join("task.token"),
            identity: directory.join("identity.pub"),
        })
    }
}

struct Response {
    status: u16,
    headers: reqwest::header::HeaderMap,
}

struct Outgoing {
    mime: &'static str,
    body: reqwest::blocking::Body,
}

fn request_to(
    address: (&str, &str),
    ca: &Path,
    token: &str,
    input: Option<Outgoing>,
    budget: (u64, u64),
    output: &mut impl Write,
) -> Result<Response, TaskError> {
    use reqwest::{Certificate, blocking::Client, header, redirect, retry, tls::Version};
    use std::time::Duration;

    let (origin, path) = address;
    let (seconds, limit) = budget;
    if !path.starts_with("/v1/") || path.contains(['#', '\r', '\n', '\0']) {
        return Err(TaskError::client("Invalid allocator route"));
    }
    if !valid_token(token, 32, 128) {
        return Err(TaskError::client("Invalid task credential file"));
    }
    let transport = |_: reqwest::Error| TaskError::client("Allocator HTTPS transport failed");
    let _ = rustls::crypto::ring::default_provider().install_default();
    let roots = Certificate::from_pem_bundle(&fs::read(ca)?)
        .map_err(|_| TaskError::client("Invalid allocator CA file"))?;
    let client = Client::builder()
        .http1_only()
        .no_proxy()
        .https_only(true)
        .redirect(redirect::Policy::none())
        .retry(retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(seconds))
        .tls_backend_rustls()
        .tls_version_min(Version::TLS_1_3)
        .tls_certs_only(roots)
        .build()
        .map_err(transport)?;
    let target = format!("{origin}{path}");
    let request = match input {
        Some(request) => client
            .post(target)
            .header(header::CONTENT_TYPE, request.mime)
            .body(request.body),
        None => client.get(target),
    };
    let response = request.bearer_auth(token).send().map_err(transport)?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let header_size = headers
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 4)
        .sum::<usize>();
    if header_size > 65_536
        || response
            .content_length()
            .is_some_and(|length| length > limit)
    {
        return Err(TaskError::client(
            "Allocator response exceeded its size limit",
        ));
    }
    if io::copy(&mut response.take(limit + 1), output)? > limit {
        return Err(TaskError::client(
            "Allocator response exceeded its size limit",
        ));
    }
    Ok(Response { status, headers })
}

fn problem(body: &[u8], status: u16) -> TaskError {
    let response = serde_json::from_slice::<Value>(body)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({"code":"http_error","reason":format!("HTTP {status}")}));
    TaskError {
        response,
        status: Some(status),
    }
}

fn json_response(body: Vec<u8>, status: u16) -> Result<Value, TaskError> {
    if !(200..300).contains(&status) {
        return Err(problem(&body, status));
    }
    Ok(serde_json::from_slice(&body)?)
}

pub fn ensure_output_available(path: &Path) -> Result<(), TaskError> {
    match path.symlink_metadata() {
        Ok(_) => Err(TaskError::client("Output file already exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn output_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

pub fn store_task_token(grant: &mut Value, path: &Path) -> Result<(), TaskError> {
    ensure_output_available(path)?;
    let token = grant
        .as_object_mut()
        .and_then(|map| map.remove("token"))
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| valid_token(value, 32, 128))
        .ok_or_else(|| TaskError::client("Issuer returned an invalid token"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(output_directory(path))?;
    writeln!(temporary, "{token}")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|problem| problem.error)?;
    Ok(())
}

pub struct TaskClient {
    pub endpoint: String,
    pub ca: PathBuf,
    pub token_file: PathBuf,
}

impl TaskClient {
    pub fn new(endpoint: &str, ca: PathBuf, token_file: PathBuf) -> Result<Self, TaskError> {
        Ok(Self {
            endpoint: verified_origin(endpoint)?,
            ca,
            token_file,
        })
    }

    pub fn request(&self, path: &str, payload: Option<&Value>) -> Result<Value, TaskError> {
        let token = fs::read_to_string(&self.token_file)?;
        let request = payload.map(|payload| Outgoing {
            mime: "application/json",
            body: payload.to_string().into(),
        });
        let mut response = Vec::new();
        let result = request_to(
            (&self.endpoint, path),
            &self.ca,
            token.trim(),
            request,
            (20, 8 * 1024 * 1024),
            &mut response,
        )?;
        json_response(response, result.status)
    }

    pub fn upload(&self, route: &str, path: &Path, key: Option<&str>) -> Result<Value, TaskError> {
        let mut archive = tempfile::NamedTempFile::new()?;
        let (checksum, bytes) = digest_to(File::open(path)?, archive.as_file_mut(), MAX_ARCHIVE)?;
        archive.as_file().sync_all()?;
        let fallback = format!("sha256-{checksum}");
        let grant = self.request(
            route,
            Some(&json!({"size":bytes,"sha256":checksum,"request_key":key.unwrap_or(&fallback)})),
        )?;
        if let Some(existing) = grant.get("checkpoint") {
            return Ok(existing.clone());
        }
        let (origin, ticket) = transfer(&grant)?;
        let mut response = Vec::new();
        let result = request_to(
            (&origin, "/v1/checkpoint"),
            &self.ca,
            ticket,
            Some(Outgoing {
                mime: "application/gzip",
                body: reqwest::blocking::Body::sized(archive.reopen()?, bytes),
            }),
            (290, 8 * 1024 * 1024),
            &mut response,
        )?;
        json_response(response, result.status)
    }

    pub fn download(&self, grant: &Value, destination: &Path) -> Result<Value, TaskError> {
        ensure_output_available(destination)?;
        let (origin, ticket) = transfer(grant)?;
        let expected = grant["sha256"]
            .as_str()
            .filter(|value| hex(value, 64))
            .ok_or_else(|| TaskError::client("Invalid checkpoint digest"))?;
        let directory = output_directory(destination);
        let mut partial = tempfile::NamedTempFile::new_in(directory)?;
        let response = request_to(
            (&origin, "/v1/checkpoint"),
            &self.ca,
            ticket,
            None,
            (150, 1024 * 1024 * 1024),
            partial.as_file_mut(),
        )?;
        if !(200..300).contains(&response.status) {
            let mut result = Vec::new();
            partial.as_file_mut().rewind()?;
            partial.as_file_mut().take(4096).read_to_end(&mut result)?;
            return Err(problem(&result, response.status));
        }
        let declared = content_length(&response.headers)?;
        let actual = partial.as_file().metadata()?.len();
        if declared != actual {
            return Err(TaskError::client("Checkpoint download ended early"));
        }
        partial.as_file_mut().rewind()?;
        if digest_to(partial.as_file_mut(), io::sink(), declared)?.0 != expected {
            return Err(TaskError::client("Checkpoint digest did not match"));
        }
        partial.as_file().sync_all()?;
        partial
            .persist_noclobber(destination)
            .map_err(|problem| problem.error)?;
        Ok(json!({"path":destination,"sha256":expected}))
    }
}

fn content_length(headers: &reqwest::header::HeaderMap) -> Result<u64, TaskError> {
    let invalid = || TaskError::client("Invalid archive size");
    let mut values = headers.get_all(reqwest::header::CONTENT_LENGTH).iter();
    let length = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| (1..=1_073_741_824).contains(value))
        .ok_or_else(invalid)?;
    if values.next().is_some() {
        return Err(invalid());
    }
    Ok(length)
}

fn transfer(grant: &Value) -> Result<(String, &str), TaskError> {
    let endpoint = grant["endpoint"]
        .as_str()
        .ok_or_else(|| TaskError::client("Checkpoint endpoint is missing"))?;
    let ticket = grant["ticket"]
        .as_str()
        .filter(|ticket| valid_token(ticket, 43, 43))
        .ok_or_else(|| TaskError::client("Invalid checkpoint ticket"))?;
    Ok((verified_origin(endpoint)?, ticket))
}

pub fn check_lease_id(value: &str) -> Result<&str, TaskError> {
    hex(value, 32)
        .then_some(value)
        .ok_or_else(|| TaskError::client("Invalid lease ID"))
}

#[cfg(test)]
mod transport_tests {
    use super::*;
    use rustls::{
        ServerConfig, ServerConnection, StreamOwned,
        pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject},
    };
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        process::Command,
        sync::Arc,
        time::Duration,
    };

    #[test]
    fn private_ca_tls_and_bearer_auth_work_in_the_portable_client() {
        let root = tempfile::tempdir().unwrap();
        let certificate = root.path().join("tls.pem");
        let key = root.path().join("key.pem");
        let out = Command::new("openssl")
            .args(["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout"])
            .arg(&key)
            .arg("-out")
            .arg(&certificate)
            .args([
                "-days",
                "1",
                "-subj",
                "/CN=fixture",
                "-addext",
                "subjectAltName=IP:127.0.0.1",
                "-addext",
                "basicConstraints=critical,CA:FALSE",
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let tls = Arc::new(
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(
                    vec![CertificateDer::from_pem_file(&certificate).unwrap()],
                    PrivateKeyDer::from_pem_file(key).unwrap(),
                )
                .unwrap(),
        );
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let endpoint = format!("https://{}", listener.local_addr().unwrap());
        let token = "task-fixture-token-0123456789abcdef";
        let task_token = root.path().join("token");
        fs::write(&task_token, token).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut channel = BufReader::new(StreamOwned::new(
                ServerConnection::new(tls).unwrap(),
                stream,
            ));
            let mut lines = String::new();
            loop {
                let mut line = String::new();
                if channel.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
                lines.push_str(&line);
            }
            let request = lines.to_ascii_lowercase();
            assert!(request.starts_with("get /v1/capabilities http/1.1\r\n"));
            assert!(request.contains(&format!("authorization: bearer {token}\r\n")));
            let io = channel.get_mut();
            io.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}").unwrap();
            io.conn.send_close_notify();
            io.flush().unwrap();
        });
        let client = TaskClient::new(&endpoint, certificate, task_token).unwrap();
        let result = client.request("/v1/capabilities", None).unwrap();
        assert_eq!(result, json!({"ok":true}));
        server.join().unwrap();
    }
}
