use std::{
    fs::{self, File},
    net::IpAddr,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    process::{bounded, shell_arg},
    request::{APPLE, LINUX, public_key},
    task_client::{TaskClient, TaskError, check_lease_id, endpoint_from_record},
    tokens::hex,
    workspace::APPLE_WORKSPACE,
};

fn error(message: impl Into<String>) -> TaskError {
    TaskError::client(message)
}

pub fn discover(client: &Path, config: Option<&Path>, record: &str) -> Result<String, TaskError> {
    let mut command = Command::new(client);
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    command.args(["get", record]);
    let output = bounded(&mut command, None, Duration::from_secs(50), 2_097_152)?;
    if !output.status.success() {
        return Err(error(
            "Registry discovery failed; verify access and the registered lease record",
        ));
    }
    let entry = serde_json::from_slice(&output.stdout)?;
    endpoint_from_record(&entry)
}

fn run(command: &mut Command, input: Option<&File>, duration: u64) -> Result<Vec<u8>, TaskError> {
    let output = bounded(command, input, Duration::from_secs(duration), 65_536)?;
    if !output.status.success() {
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".into(), |code| code.to_string());
        return Err(error(format!(
            "The scoped task command failed ({code}). Check the recorded lease before retrying."
        )));
    }
    Ok(output.stdout)
}

fn helpers(recipe: &str, directory: Option<&Path>) -> Result<PathBuf, TaskError> {
    let same_platform = match recipe {
        LINUX => cfg!(all(target_os = "linux", target_arch = "x86_64")),
        APPLE => cfg!(all(target_os = "macos", target_arch = "aarch64")),
        _ => return Err(error("Unknown restore helper recipe")),
    };
    let executable = std::env::current_exe()?;
    let bin = executable
        .parent()
        .ok_or_else(|| error("Could not locate the task client directory"))?;
    let manifest = directory
        .map(Path::to_owned)
        .or_else(|| std::env::var_os("LAB_RESTORE_HELPERS").map(PathBuf::from))
        .unwrap_or_else(|| bin.join("restore-helpers"));
    let path = manifest.join(recipe).join("lab-restore-workspace");
    if path.is_file() {
        return Ok(path);
    }
    let local = bin.join("lab-restore-workspace");
    if same_platform && local.is_file() {
        return Ok(local);
    }
    Err(error(format!(
        "The restore helper for {recipe} is missing. Install the matching platform artifact."
    )))
}

pub struct Resume<'a> {
    pub predecessor: &'a str,
    pub task: &'a str,
    pub request_key: &'a str,
    pub seconds: u64,
    pub identity: &'a Path,
    pub checkpoint: Option<&'a str>,
    pub helpers: Option<&'a Path>,
}

fn ssh(binding: &Value, key: &Path, known: &Path) -> Result<Command, TaskError> {
    let host = binding["host"]
        .as_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .ok_or_else(|| error("Invalid replacement SSH address"))?;
    let alias = binding["host_key_alias"]
        .as_str()
        .filter(|value| {
            (1..=64).contains(&value.len())
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        })
        .ok_or_else(|| error("Invalid replacement SSH host alias"))?;
    let username = binding["user"]
        .as_str()
        .filter(|user| {
            (1..=32).contains(&user.len())
                && user
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_lowercase() || b == b'_')
                && user
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b))
        })
        .ok_or_else(|| error("Invalid replacement SSH user"))?;
    let pubkey =
        public_key(&binding["host_key"]).map_err(|_| error("Invalid replacement SSH host key"))?;
    fs::write(known, format!("{alias} {pubkey}\n"))?;
    let mut command = Command::new("ssh");
    command
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "IdentityAgent=none",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "ClearAllForwardings=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
        ])
        .arg(format!("UserKnownHostsFile={}", known.display()))
        .args(["-o", &format!("HostKeyAlias={alias}"), "-i"])
        .arg(key)
        .arg(format!("{username}@{host}"));
    Ok(command)
}

pub fn resume(client: &TaskClient, input: Resume<'_>) -> Result<Value, TaskError> {
    let source = check_lease_id(input.predecessor)?;
    if let Some(checkpoint) = input.checkpoint {
        check_lease_id(checkpoint)?;
    }
    let mut probe = Command::new("ssh-keygen");
    probe.args(["-y", "-f"]).arg(input.identity);
    let bytes = run(&mut probe, None, 8)?;
    let key = String::from_utf8_lossy(&bytes)
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let key = public_key(&json!(key)).map_err(|_| {
        error("Use an independently generated Ed25519 identity for the replacement")
    })?;
    let predecessor = client.request(&format!("/v1/leases/{source}"), None)?;
    let recipe = predecessor["recipe"]
        .as_str()
        .ok_or_else(|| error("Predecessor recipe is missing"))?;
    let helper = helpers(recipe, input.helpers)?;
    let payload = json!({"task":input.task,"request_key":input.request_key,"seconds":input.seconds,
        "ssh_public_key":key,"reclaim_on_expiry":true,"resume_from":source,"checkpoint_id":input.checkpoint});
    let mut lease = client.request("/v1/leases", Some(&payload))?;
    let id = check_lease_id(lease["id"].as_str().unwrap_or_default())?.to_owned();
    let route = format!("/v1/leases/{id}");
    eprintln!("Replacement lease {id}; retry with the same request key after interruption.");
    let deadline = Instant::now() + Duration::from_secs(220);
    while lease["status"].as_str().is_some_and(|value| {
        matches!(
            value,
            "cloning" | "resizing" | "starting" | "waiting-for-guest-ip"
        )
    }) && Instant::now() < deadline
    {
        thread::sleep(Duration::from_secs(1));
        lease = client.request(&route, None)?;
    }
    if lease["status"] != "ready" || lease["recipe"] != recipe {
        return Err(error(
            "Replacement lease is not ready on the expected recipe. Inspect it before retrying.",
        ));
    }
    let point = &lease["resume_point"];
    if point["lease"] != source {
        return Err(error(
            "Replacement lineage differs from the requested predecessor",
        ));
    }
    let digest = point["sha256"]
        .as_str()
        .filter(|value| hex(value, 64))
        .ok_or_else(|| error("Invalid acknowledged recovery digest"))?;
    let workspace = match &lease["workspace"] {
        Value::Null => None,
        Value::String(path) if path == APPLE_WORKSPACE => Some(path.as_str()),
        _ => return Err(error("Invalid replacement workspace path")),
    };
    let mut result = json!({"lease":id,"workspace":workspace.unwrap_or("~/workspace"),"recovery_point":point,
        "coverage":if point["capture"]=="final"{"Declared workspace at final capture."}
            else{"Declared checkpoint only. Later unacknowledged changes are unavailable."}});
    if let Some(stamp) = lease["restore_completed_at"].as_f64() {
        result["restore"] = json!("previously-restored");
        result["acknowledged_at"] = json!(stamp);
        return Ok(result);
    }
    let recovery = client.request(
        &format!("/v1/leases/{source}/checkpoint-recovery"),
        Some(&json!({"checkpoint_id":point["checkpoint_id"]})),
    )?;
    if recovery["sha256"] != digest {
        return Err(error("The authorized recovery point changed"));
    }

    let stage = tempfile::tempdir()?;
    let archive = stage.path().join("checkpoint");
    let fingerprints = stage.path().join("known_hosts");
    client.download(&recovery, &archive)?;
    let binding = &lease["ssh"];
    let mut install = ssh(binding, input.identity, &fingerprints)?;
    install.arg(concat!(
        "umask 077; p=$(mktemp \"$HOME/.lab-restore.XXXXXXXX\") || exit 1; ",
        "if cat >\"$p\" && chmod 700 \"$p\"; then basename \"$p\"; ",
        "else rm -f \"$p\"; exit 1; fi"
    ));
    let remote = run(&mut install, Some(&File::open(helper)?), 35)?;
    let remote = String::from_utf8_lossy(&remote);
    let file = remote.trim();
    if !file.strip_prefix(".lab-restore.").is_some_and(|suffix| {
        suffix.len() == 8 && suffix.bytes().all(|b| b.is_ascii_alphanumeric())
    }) {
        return Err(error("Guest returned an invalid restore helper path"));
    }
    let arguments = format!(
        "{digest} {id}{}",
        workspace
            .map(|path| format!(" {}", shell_arg(path)))
            .unwrap_or_default()
    );
    let mut command = ssh(binding, input.identity, &fingerprints)?;
    command.arg(format!(
        "/bin/sh -c {}",
        shell_arg(&format!(
            "p=\"$HOME/{file}\"; trap 'rm -f \"$p\"' 0; \"$p\" {arguments}"
        ))
    ));
    let bytes = run(&mut command, Some(&File::open(archive)?), 180)?;
    let acknowledgment = String::from_utf8_lossy(&bytes);
    let acknowledgment = acknowledgment.trim();
    if !matches!(acknowledgment, "restored" | "previously-restored") {
        return Err(error("The guest did not acknowledge the restore"));
    }
    client.request(
        &format!("{route}/restore-complete"),
        Some(&json!({"sha256":digest})),
    )?;
    result["restore"] = json!(acknowledgment);
    Ok(result)
}
