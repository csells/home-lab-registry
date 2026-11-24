use std::{
    error::Error,
    fs::{self, File},
    io::{ErrorKind, Read},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Output, Stdio},
    time::Duration,
};

use clap::ValueEnum;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{process::Command, time::timeout};
use url::Url;

use crate::{
    client::{Method, RegistryClient, valid_id},
    enrollment::binding_valid,
    validation::runtime_report,
};

#[derive(Debug, PartialEq, Eq)]
pub struct DeploymentIdentity<'a> {
    owner: &'a str,
    binding: &'a Value,
    generation: &'a str,
}

fn identifier(value: &str, maximum: usize, punctuation: &[u8]) -> bool {
    (1..=maximum).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || punctuation.contains(&byte))
}

impl<'a> DeploymentIdentity<'a> {
    pub fn from_spec(spec: &'a Value) -> Result<Self, &'static str> {
        let resources = &spec["resources"];
        let generation = resources["deployment_generation"]
            .as_str()
            .filter(|value| identifier(value, 128, b"_.:-"))
            .ok_or("Deployment requires an immutable generation")?;
        let owner = spec["owner"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or("Deployment requires an owner")?;
        let binding = &resources["runtime_binding"];
        if binding["kind"] == "guest" {
            let valid_parent = binding["machine_id"]
                .as_str()
                .and_then(|id| id.strip_prefix("machine-"))
                .is_some_and(|name| {
                    name.bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
                        && name.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                        })
                });
            if !valid_parent
                || !binding["guest_id"]
                    .as_str()
                    .is_some_and(|id| identifier(id, 128, b"_.:-"))
            {
                return Err("Guest deployment requires an exact registered parent and guest ID");
            }
        } else if !binding_valid(binding) {
            return Err("Native or external deployment requires an exact provider binding");
        }
        Ok(Self {
            owner,
            binding,
            generation,
        })
    }

    fn matches(&self, record: &Value, state: &str) -> bool {
        let spec = &record["spec"];
        spec["lifecycle"] == state
            && DeploymentIdentity::from_spec(spec).is_ok_and(|actual| self == &actual)
    }
}

pub enum DeploymentCatalog {
    Direct(RegistryClient),
    External(PathBuf),
}

impl DeploymentCatalog {
    async fn get(&self, id: &str) -> Result<Value, Box<dyn Error>> {
        match self {
            Self::Direct(client) => Ok(client.request(Method::Get, &route(id)?, None, None)?),
            Self::External(path) => external(path, &["get", id], None).await,
        }
    }

    async fn put(&self, id: &str, spec: &Value, revision: i64) -> Result<(), Box<dyn Error>> {
        match self {
            Self::Direct(client) => {
                client.request(Method::Put, &route(id)?, Some(spec), Some(revision))?;
            }
            Self::External(path) => {
                external(
                    path,
                    &["put", id, "-", "--revision", &revision.to_string()],
                    Some(spec),
                )
                .await?;
            }
        }
        Ok(())
    }

    pub async fn register(&self, id: &str, spec: &Value) -> Result<Value, Box<dyn Error>> {
        route(id)?;
        let identity = DeploymentIdentity::from_spec(spec)?;
        if spec["lifecycle"] != "active" {
            return Err("Register an active deployment".into());
        }
        if let Some(parent) = identity.binding["machine_id"]
            .as_str()
            .filter(|_| identity.binding["kind"] == "guest")
        {
            let record = self.get(parent).await?;
            let parent = &record["spec"];
            if parent["lifecycle"] != "active"
                || !parent["capabilities"]
                    .as_array()
                    .is_some_and(|caps| caps.iter().any(|value| value == "machine"))
            {
                return Err("Guest parent is not an active registered machine".into());
            }
        }
        let write = self.put(id, spec, 0).await;
        let current = match (self.get(id).await, write) {
            (Ok(current), _) => current,
            (Err(_), Err(write_error)) => return Err(write_error),
            (Err(read_error), Ok(())) => return Err(read_error),
        };
        if !identity.matches(&current, "active") {
            return Err("Record ID belongs to another deployment generation".into());
        }
        Ok(current)
    }

    pub async fn retire(&self, id: &str, spec: &Value) -> Result<Value, Box<dyn Error>> {
        route(id)?;
        let identity = DeploymentIdentity::from_spec(spec)?;
        let mut current = self.get(id).await?;
        if DeploymentIdentity::from_spec(&current["spec"])? != identity {
            return Err("The record belongs to another deployment generation".into());
        }
        if current["spec"]["lifecycle"] == "retired" {
            return Ok(current);
        }
        let revision = current["revision"]
            .as_i64()
            .ok_or("Expected current registry revision")?;
        current["spec"]["lifecycle"] = json!("retired");
        let write = self.put(id, &current["spec"], revision).await;
        let current = self.get(id).await?;
        if identity.matches(&current, "retired") {
            return Ok(current);
        }
        write?;
        Err("Registry retirement was not verified".into())
    }
}

async fn external(
    path: &Path,
    args: &[&str],
    body: Option<&Value>,
) -> Result<Value, Box<dyn Error>> {
    use tokio::io::AsyncWriteExt;
    let mut command = Command::new(path);
    command
        .args(args)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if body.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command.spawn()?;
    let result = timeout(Duration::from_secs(50), async {
        if let Some(value) = body {
            let mut input = child.stdin.take().expect("piped stdin available");
            input.write_all(&serde_json::to_vec(value)?).await?;
        }
        child.wait_with_output().await
    })
    .await??;
    if !result.status.success() {
        return Err(
            "Registry client rejected the operation. Check the record before retrying.".into(),
        );
    }
    Ok(serde_json::from_slice(&result.stdout)?)
}

fn route(id: &str) -> Result<String, &'static str> {
    valid_id(id)
        .then(|| format!("/v1/services/{id}"))
        .ok_or("Invalid registry record identifier")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum RuntimeState {
    Present,
    Missing,
    Unverified,
}

impl RuntimeState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Unverified => "unverified",
        }
    }
}

pub fn classify_systemd(unit: &str, success: bool, stdout: &str) -> RuntimeState {
    let fields: std::collections::HashMap<_, _> = stdout
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    if fields.get("Id") != Some(&unit) {
        return RuntimeState::Unverified;
    }
    match fields.get("LoadState").copied() {
        Some("not-found") => RuntimeState::Missing,
        Some("loaded" | "masked") if success => RuntimeState::Present,
        _ => RuntimeState::Unverified,
    }
}

pub fn classify_launchd(
    target: &str,
    status: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> RuntimeState {
    if status == Some(113) && stderr.contains("Could not find service") {
        RuntimeState::Missing
    } else if status == Some(0) && stdout.lines().next() == Some(&format!("{target} = {{")) {
        RuntimeState::Present
    } else {
        RuntimeState::Unverified
    }
}

async fn local(mut command: Command) -> Option<Output> {
    command.kill_on_drop(true);
    timeout(Duration::from_secs(8), command.output())
        .await
        .ok()?
        .ok()
}

#[derive(Deserialize)]
pub struct Publisher {
    registry: String,
    record_id: String,
    token_file: PathBuf,
    ca_file: PathBuf,
    #[serde(default = "ttl")]
    ttl_seconds: u64,
    unit: Option<String>,
    domain: Option<String>,
    label: Option<String>,
}

fn ttl() -> u64 {
    900
}

pub async fn observe_systemd(unit: &str) -> Result<RuntimeState, Box<dyn Error>> {
    if !unit
        .strip_suffix(".service")
        .is_some_and(|name| identifier(name, 180, b"_.@:-"))
    {
        return Err("Use an exact systemd service unit name".into());
    }
    let mut command = Command::new("/usr/bin/systemctl");
    command.args(["show", "--property=Id,LoadState", "--", unit]);
    Ok(local(command)
        .await
        .map_or(RuntimeState::Unverified, |result| {
            classify_systemd(
                unit,
                result.status.success(),
                &String::from_utf8_lossy(&result.stdout),
            )
        }))
}

pub async fn observe_launchd(
    domain: &str,
    label: &str,
    plist: Option<&Path>,
) -> Result<RuntimeState, Box<dyn Error>> {
    let named_domain = ["gui/", "user/"]
        .iter()
        .find_map(|prefix| domain.strip_prefix(prefix))
        .is_some_and(|uid| {
            (1..=10).contains(&uid.len())
                && !uid.starts_with('0')
                && uid.bytes().all(|b| b.is_ascii_digit())
        });
    if (domain != "system" && !named_domain)
        || !identifier(label, 180, b"_.-")
        || !label
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
    {
        return Err("Use an exact launchd domain and service label".into());
    }
    if plist.is_some_and(|path| {
        !path.is_absolute()
            || path.extension().and_then(|extension| extension.to_str()) != Some("plist")
    }) {
        return Err("Use an absolute launchd plist path".into());
    }
    let target = format!("{domain}/{label}");
    let mut command = Command::new("/bin/launchctl");
    command.env("LC_ALL", "C").args(["print", &target]);
    let Some(result) = local(command).await else {
        return Ok(RuntimeState::Unverified);
    };
    let stdout = String::from_utf8_lossy(&result.stdout);
    let stderr = String::from_utf8_lossy(&result.stderr);
    let state = classify_launchd(&target, result.status.code(), &stdout, &stderr);
    if state == RuntimeState::Missing {
        let expected = format!("Could not find service \"{label}\" in domain");
        let mut domain_lookup = Command::new("/bin/launchctl");
        domain_lookup.args(["print", domain]);
        if !stderr.contains(&expected)
            || !local(domain_lookup)
                .await
                .is_some_and(|reply| reply.status.success())
        {
            return Ok(RuntimeState::Unverified);
        }
    }
    let Some(plist) = plist else {
        return Ok(state);
    };
    match (state, fs::symlink_metadata(plist)) {
        (RuntimeState::Present, Ok(metadata))
            if metadata.is_file()
                && stdout.lines().any(|line| {
                    line.trim()
                        .strip_prefix("path = ")
                        .is_some_and(|path| Path::new(path) == plist)
                }) =>
        {
            Ok(RuntimeState::Present)
        }
        (RuntimeState::Missing, Err(error))
            if error.kind() == ErrorKind::NotFound && plist.parent().is_some_and(Path::is_dir) =>
        {
            Ok(RuntimeState::Missing)
        }
        _ => Ok(RuntimeState::Unverified),
    }
}

impl Publisher {
    pub async fn systemd(&self) -> Result<RuntimeState, Box<dyn Error>> {
        let unit = self.unit.as_deref().ok_or("Missing systemd unit")?;
        observe_systemd(unit).await
    }

    pub async fn launchd(&self) -> Result<RuntimeState, Box<dyn Error>> {
        let domain = self.domain.as_deref().ok_or("Missing launchd domain")?;
        let label = self.label.as_deref().ok_or("Missing launchd label")?;
        observe_launchd(domain, label, None).await
    }

    pub fn publish(&self, state: RuntimeState) -> Result<Value, Box<dyn Error>> {
        let endpoint = Url::parse(&self.registry)?;
        if endpoint.path() != "/" {
            return Err("Use the authenticated registry HTTPS origin".into());
        }
        let route = format!("{}/runtime", route(&self.record_id)?);
        let file = File::open(&self.token_file)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 || metadata.len() > 512
        {
            return Err("Publisher token file must be private".into());
        }
        let mut token = String::new();
        file.take(512).read_to_string(&mut token)?;
        let token = token.trim();
        if token.len() < 32 || !identifier(token, 128, b"_-") {
            return Err("Invalid publisher token file".into());
        }
        let body = json!({"state":state.name(),"ttl_seconds":self.ttl_seconds});
        runtime_report(&body)?;
        let client = RegistryClient::from_value(
            json!({"url":self.registry,"ca_file":self.ca_file,"writer_token":token}),
        )?;
        let result = client.request(Method::Post, &route, Some(&body), None)?;
        if result["state"] != state.name() {
            return Err("Observed state did not match the registry acknowledgment".into());
        }
        Ok(result)
    }
}
