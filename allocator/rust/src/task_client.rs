use std::{
    error::Error,
    fmt,
    fs::{self, File},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
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

fn quote(config: &mut String, key: &str, value: &str) {
    config.push_str(key);
    config.push_str(" = \"");
    for ch in value.chars() {
        match ch {
            '"' => config.push_str("\\\""),
            '\\' => config.push_str("\\\\"),
            '\n' => config.push_str("\\n"),
            '\r' => config.push_str("\\r"),
            '\t' => config.push_str("\\t"),
            _ => config.push(ch),
        }
    }
    config.push_str("\"\n");
}

fn path_text(path: &Path) -> Result<&str, TaskError> {
    path.to_str()
        .ok_or_else(|| TaskError::client("Credential or output path is not valid text"))
}

fn curl(
    origin: &str,
    ca: &Path,
    token: &str,
    path: &str,
    mut command: Command,
    options: &[(&str, &str)],
) -> Result<(Vec<u8>, u16), TaskError> {
    if !path.starts_with("/v1/") || path.contains(['#', '\r', '\n', '\0']) {
        return Err(TaskError::client("Invalid allocator route"));
    }
    if !valid_token(token, 32, 128) {
        return Err(TaskError::client("Invalid task credential file"));
    }
    let mut configuration = String::new();
    quote(&mut configuration, "url", &format!("{origin}{path}"));
    quote(&mut configuration, "cacert", path_text(ca)?);
    quote(
        &mut configuration,
        "header",
        &format!("Authorization: Bearer {token}"),
    );
    for (field, value) in options {
        quote(&mut configuration, field, value);
    }
    let mut child = command
        .args([
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--noproxy",
            "*",
            "--tlsv1.3",
            "--config",
            "-",
            "--write-out",
            "\n%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let written = child
        .stdin
        .take()
        .expect("piped stdin available")
        .write_all(configuration.as_bytes());
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).replace(token, "[REDACTED]");
        return Err(TaskError::client(format!(
            "Allocator transport failed (curl {}): {}",
            output
                .status
                .code()
                .map_or_else(|| "signal".into(), |code| code.to_string()),
            detail.trim()
        )));
    }
    written?;
    let mut response = output.stdout;
    let split = response
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(|| TaskError::client("Allocator response has no HTTP status"))?;
    let status = std::str::from_utf8(&response[split + 1..])
        .ok()
        .and_then(|text| text.parse::<u16>().ok())
        .ok_or_else(|| TaskError::client("Allocator response has an invalid HTTP status"))?;
    response.truncate(split);
    Ok((response, status))
}

fn options(seconds: &str, max_bytes: &str) -> Command {
    let mut command = Command::new("curl");
    command.args([
        "--disable",
        "--connect-timeout",
        "10",
        "--max-time",
        seconds,
        "--max-filesize",
        max_bytes,
    ]);
    command
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
        let mut fields = vec![("header", "Content-Type: application/json")];
        let serialized;
        if let Some(payload) = payload {
            serialized = payload.to_string();
            fields.push(("data-binary", &serialized));
        }
        let (body, status) = curl(
            &self.endpoint,
            &self.ca,
            token.trim(),
            path,
            options("20", "8388608"),
            &fields,
        )?;
        json_response(body, status)
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
        let (body, status) = curl(
            &origin,
            &self.ca,
            ticket,
            "/v1/checkpoint",
            options("290", "8388608"),
            &[
                ("header", "Content-Type: application/gzip"),
                ("data-binary", &format!("@{}", path_text(archive.path())?)),
            ],
        )?;
        json_response(body, status)
    }

    pub fn download(&self, grant: &Value, destination: &Path) -> Result<Value, TaskError> {
        ensure_output_available(destination)?;
        let (origin, ticket) = transfer(grant)?;
        let expected = grant["sha256"]
            .as_str()
            .filter(|value| hex(value, 64))
            .ok_or_else(|| TaskError::client("Invalid checkpoint digest"))?;
        let directory = output_directory(destination);
        let headers = tempfile::NamedTempFile::new_in(directory)?;
        let mut partial = tempfile::NamedTempFile::new_in(directory)?;
        let (_, status) = curl(
            &origin,
            &self.ca,
            ticket,
            "/v1/checkpoint",
            options("150", "1073741824"),
            &[
                ("dump-header", path_text(headers.path())?),
                ("output", path_text(partial.path())?),
            ],
        )?;
        if !(200..300).contains(&status) {
            let mut result = Vec::new();
            File::open(partial.path())?
                .take(4096)
                .read_to_end(&mut result)?;
            return Err(problem(&result, status));
        }
        let declared = content_length(&fs::read_to_string(headers.path())?)?;
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
