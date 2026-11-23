use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use serde::Deserialize;
use serde_json::Value;
use url::Url;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Put,
    Post,
}

impl Method {
    fn name(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
        }
    }
}

#[derive(Debug)]
pub struct TransportError(String);

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransportError {}

fn failure(message: impl Into<String>) -> TransportError {
    TransportError(message.into())
}

#[derive(Deserialize)]
struct Settings {
    url: String,
    ca_file: PathBuf,
    reader_token: Option<String>,
    writer_token: Option<String>,
}

pub struct RegistryClient {
    base: String,
    settings: Settings,
}

pub fn expand_home(path: &Path) -> PathBuf {
    if let (Ok(rest), Some(home)) = (
        path.strip_prefix("~"),
        std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
    ) {
        return PathBuf::from(home).join(rest);
    }
    path.to_owned()
}

fn quote(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

impl RegistryClient {
    pub fn read_config(path: &Path) -> Result<Self, TransportError> {
        let file = File::open(expand_home(path))
            .map_err(|_| failure("Could not open registry credential config"))?;
        let attributes = file
            .metadata()
            .map_err(|_| failure("Could not read registry credential file permissions"))?;
        let private = crate::client_permissions::private(&file)
            .map_err(|_| failure("Could not read registry credential file permissions"))?;
        if !attributes.is_file() || !private {
            return Err(failure(if cfg!(windows) {
                "Credential config must have a private Windows ACL"
            } else {
                "Credential config must have mode 0600"
            }));
        }
        let value = serde_json::from_reader(file)
            .map_err(|_| failure("Registry credential config is not valid JSON"))?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, TransportError> {
        let settings: Settings = serde_json::from_value(value)
            .map_err(|_| failure("Registry configuration requires a URL and trusted CA file"))?;
        let base = Url::parse(&settings.url).map_err(|_| failure("Registry URL must use HTTPS"))?;
        if base.scheme() != "https"
            || base.host().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err(failure(
                "Registry URL must use HTTPS with no inline credentials, query or fragment",
            ));
        }
        Ok(Self {
            base: base.as_str().trim_end_matches('/').to_owned(),
            settings,
        })
    }

    pub fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        revision: Option<i64>,
    ) -> Result<Value, TransportError> {
        if !path.starts_with('/')
            || path.starts_with("//")
            || path.contains(['\r', '\n', '\0', '#'])
        {
            return Err(failure(
                "Registry route must be an absolute path on the configured endpoint",
            ));
        }
        let token = match method {
            Method::Get => &self.settings.reader_token,
            Method::Put | Method::Post => &self.settings.writer_token,
        }
        .as_deref()
        .filter(|token| !token.is_empty() && !token.bytes().any(|byte| byte.is_ascii_control()))
        .ok_or_else(|| failure("Registry credential for this operation is missing or invalid"))?;
        let ca = expand_home(&self.settings.ca_file);
        let ca = ca
            .to_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| failure("Registry CA file path is missing or invalid"))?;
        let mut options = String::new();
        let mut add = |name: &str, value: &str| {
            options.push_str(name);
            options.push_str(" = ");
            options.push_str(&quote(value));
            options.push('\n');
        };
        add("url", &format!("{}{path}", self.base));
        add("cacert", ca);
        add("request", method.name());
        add("header", &format!("Authorization: Bearer {token}"));
        add("header", "Content-Type: application/json");
        add("header", "User-Agent: HomeLabRegistry/1");
        if let Some(revision) = revision {
            add("header", &format!("If-Match: {revision}"));
        }
        if let Some(body) = body {
            add("data-binary", &body.to_string());
        }

        let attempts = if method == Method::Get { 3 } else { 1 };
        for attempt in 1..=attempts {
            let result = curl(&options)?;
            if result.status.success() {
                return response(&result.stdout);
            }
            if matches!(result.status.code(), Some(18 | 28 | 52 | 56)) && attempt < attempts {
                eprintln!(
                    "Registry transport interrupted; retrying read ({}/{attempts})",
                    attempt + 1
                );
                thread::sleep(Duration::from_millis(250 * attempt as u64));
                continue;
            }
            let detail = String::from_utf8_lossy(&result.stderr).replace(token, "[REDACTED]");
            let code = result
                .status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string());
            return Err(failure(format!(
                "Registry transport failed (curl {code}): {}; no partial response accepted",
                detail.trim()
            )));
        }
        unreachable!("at least one transport attempt")
    }
}

fn curl(config: &str) -> Result<std::process::Output, TransportError> {
    // Preserve the caller's proxy environment. curl reads the token on stdin.
    let mut process = Command::new("curl")
        .args([
            "--disable",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            "15",
            "--config",
            "-",
            "--write-out",
            "\n%{http_code}",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                failure("Registry client requires curl on PATH")
            } else {
                failure("Registry client could not launch curl")
            }
        })?;
    let written = process
        .stdin
        .take()
        .expect("piped stdin available")
        .write_all(config.as_bytes());
    let result = process
        .wait_with_output()
        .map_err(|_| failure("Registry transport could not complete"))?;
    if result.status.success() && written.is_err() {
        return Err(failure(
            "Registry client failed to pass the complete request to curl",
        ));
    }
    Ok(result)
}

fn response(data: &[u8]) -> Result<Value, TransportError> {
    let split = data
        .iter()
        .rposition(|byte| *byte == b'\n')
        .ok_or_else(|| failure("Registry transport returned no HTTP status"))?;
    let code = std::str::from_utf8(&data[split + 1..])
        .ok()
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| failure("Registry transport returned no HTTP status"))?;
    if !(200..300).contains(&code) {
        return Err(failure(format!(
            "Registry HTTP {code}; redirects are not followed; verify endpoint and access permissions"
        )));
    }
    serde_json::from_slice(&data[..split])
        .map_err(|_| failure("Registry response was not complete valid JSON"))
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct SshTarget {
    pub destination: String,
    pub port: u16,
    pub host_key_alias: String,
}

pub use crate::validation::valid_id;

pub fn resolve_ssh_record(
    record: &Value,
    machine: &str,
    username: &str,
    port: Option<u16>,
) -> Result<SshTarget, TransportError> {
    if !valid_id(machine) {
        return Err(failure("Invalid machine identity"));
    }
    let mut user = username.bytes();
    if !user
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !user.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(failure("Invalid SSH user"));
    }
    let record = &record["spec"];
    if record["lifecycle"] != "active"
        || record["resources"]["agent_presence"]["status"] != "current"
    {
        return Err(failure("Machine registration unavailable or stale"));
    }
    let invalid_endpoint = || failure("Invalid registered SSH endpoint");
    let endpoint = record["endpoints"]["ssh"]
        .as_str()
        .and_then(|value| Url::parse(value).ok())
        .filter(|url| {
            url.scheme() == "ssh" && url.username().is_empty() && url.password().is_none()
        })
        .ok_or_else(invalid_endpoint)?;
    let host = match endpoint.host().ok_or_else(invalid_endpoint)? {
        url::Host::Domain(host) => host.to_owned(),
        url::Host::Ipv4(host) => host.to_string(),
        url::Host::Ipv6(host) => host.to_string(),
    };
    let selected = port.or(endpoint.port()).unwrap_or(22);
    if selected == 0 {
        return Err(failure("Invalid SSH port"));
    }
    Ok(SshTarget {
        destination: format!("{username}@{host}"),
        port: selected,
        host_key_alias: machine.to_owned(),
    })
}

impl RegistryClient {
    pub fn ssh_target(
        &self,
        machine: &str,
        username: &str,
        port: Option<u16>,
    ) -> Result<SshTarget, TransportError> {
        if !valid_id(machine) {
            return Err(failure("Invalid machine identity"));
        }
        let record = self.request(Method::Get, &format!("/v1/services/{machine}"), None, None)?;
        resolve_ssh_record(&record, machine, username, port)
    }
}
