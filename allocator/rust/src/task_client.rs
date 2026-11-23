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
    headers: Vec<u8>,
}

struct Outgoing<'a> {
    mime: &'static str,
    size: u64,
    body: &'a mut dyn Read,
}

fn request_to(
    address: (&str, &str),
    ca: &Path,
    token: &str,
    mut input: Option<Outgoing<'_>>,
    budget: (u64, u64),
    output: &mut impl Write,
) -> Result<Response, TaskError> {
    use curl::easy::{Easy, List, ReadError, SslVersion};
    let (origin, path) = address;
    let (seconds, limit) = budget;
    if !path.starts_with("/v1/") || path.contains(['#', '\r', '\n', '\0']) {
        return Err(TaskError::client("Invalid allocator route"));
    }
    if !valid_token(token, 32, 128) {
        return Err(TaskError::client("Invalid task credential file"));
    }
    let transport = |error: curl::Error| {
        TaskError::client(format!(
            "Allocator HTTPS transport failed (curl {})",
            error.code()
        ))
    };
    let mut headers = List::new();
    headers
        .append(&format!("Authorization: Bearer {token}"))
        .map_err(transport)?;
    let mut client = Easy::new();
    client.url(&format!("{origin}{path}")).map_err(transport)?;
    client.follow_location(false).map_err(transport)?;
    client.noproxy("*").map_err(transport)?;
    client
        .connect_timeout(std::time::Duration::from_secs(10))
        .map_err(transport)?;
    client
        .timeout(std::time::Duration::from_secs(seconds))
        .map_err(transport)?;
    client.ssl_version(SslVersion::Tlsv13).map_err(transport)?;
    client.ssl_verify_peer(true).map_err(transport)?;
    client.ssl_verify_host(true).map_err(transport)?;
    client.ssl_cainfo_blob(&fs::read(ca)?).map_err(transport)?;
    // Only the independently supplied CA is trusted, including on builds with
    // a default system CA directory.
    let empty_ca_path = tempfile::tempdir()?;
    client.capath(empty_ca_path.path()).map_err(transport)?;
    client.max_filesize(limit).map_err(transport)?;
    if let Some(request) = &input {
        client.post(true).map_err(transport)?;
        client.post_field_size(request.size).map_err(transport)?;
        headers
            .append(&format!("Content-Type: {}", request.mime))
            .map_err(transport)?;
        headers.append("Expect:").map_err(transport)?;
    }
    client.http_headers(headers).map_err(transport)?;

    let mut response_headers = Vec::new();
    let mut remaining = limit;
    let mut failed_write = None;
    let mut failed_read = None;
    let status = {
        let mut transfer = client.transfer();
        if let Some(request) = input.as_mut() {
            transfer
                .read_function(|output| match request.body.read(output) {
                    Ok(size) => Ok(size),
                    Err(error) => {
                        failed_read = Some(error);
                        Err(ReadError::Abort)
                    }
                })
                .map_err(transport)?;
        }
        transfer
            .header_function(|line| {
                if response_headers.len() + line.len() > 65_536 {
                    return false;
                }
                response_headers.extend_from_slice(line);
                true
            })
            .map_err(transport)?;
        transfer
            .write_function(|chunk| {
                let Some(left) = remaining.checked_sub(chunk.len() as u64) else {
                    failed_write = Some(io::Error::other(
                        "Allocator response exceeded its size limit",
                    ));
                    return Ok(0);
                };
                remaining = left;
                if let Err(error) = output.write_all(chunk) {
                    failed_write = Some(error);
                    return Ok(0);
                }
                Ok(chunk.len())
            })
            .map_err(transport)?;
        transfer.perform()
    };
    if let Some(error) = failed_write.or(failed_read) {
        return Err(error.into());
    }
    status.map_err(transport)?;
    let status = u16::try_from(client.response_code().map_err(transport)?)
        .map_err(|_| TaskError::client("Allocator response has an invalid HTTP status"))?;
    Ok(Response {
        status,
        headers: response_headers,
    })
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
        let serialized = payload.map(|payload| payload.to_string());
        let mut source = serialized.as_deref().unwrap_or_default().as_bytes();
        let request = serialized.as_ref().map(|body| Outgoing {
            mime: "application/json",
            size: body.len() as u64,
            body: &mut source,
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
        archive.as_file_mut().rewind()?;
        let result = request_to(
            (&origin, "/v1/checkpoint"),
            &self.ca,
            ticket,
            Some(Outgoing {
                mime: "application/gzip",
                size: bytes,
                body: archive.as_file_mut(),
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
        let declared = content_length(&String::from_utf8_lossy(&response.headers))?;
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

fn content_length(headers: &str) -> Result<u64, TaskError> {
    let invalid = || TaskError::client("Invalid archive size");
    let block = headers
        .trim_end()
        .rsplit("\r\n\r\n")
        .next()
        .ok_or_else(invalid)?;
    let mut values = block
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"));
    let length = values
        .next()
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
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
