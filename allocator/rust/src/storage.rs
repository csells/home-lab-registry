use std::{
    error::Error,
    fmt, fs,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};
use wait_timeout::ChildExt;

use crate::{
    restic_scope::{Snapshot, TAG, archive_identity, command},
    task_client::TaskClient,
    tokens::hex,
    workspace::{MAX_ARCHIVE, digest_to},
};

const MAX_METADATA: u64 = 1024 * 1024;
const STORAGE: &str = "storage metadata";

#[derive(Debug)]
pub(crate) struct StorageError(String);

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl Error for StorageError {}

fn reason(message: impl Into<String>) -> StorageError {
    StorageError(message.into())
}

fn failed(stage: &str, cause: &str) -> StorageError {
    reason(format!("{stage}: {cause}."))
}

fn safe_diagnostic(stderr: &[u8]) -> &'static str {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    [
        ("expanded checkpoint exceeds", "workspace budget exceeded"),
        ("unsupported file type", "unsupported archive entry"),
        ("workspace is missing", "workspace missing or redirected"),
        ("outside", "path outside workspace"),
        (
            "insufficient temporary space",
            "insufficient temporary space",
        ),
        (
            "temporary storage budget",
            "temporary storage budget exceeded",
        ),
        ("no space left", "storage full"),
        ("permission denied", "permission denied"),
        ("connection refused", "connection refused"),
        ("timed out", "connection timeout"),
        ("slot identity changed", "slot identity changed"),
        (
            "stopped-slot source was incomplete",
            "stopped disk source was incomplete",
        ),
        ("another lease", "lease identity mismatch"),
        ("repository is already locked", "repository locked"),
    ]
    .into_iter()
    .find_map(|(fragment, label)| stderr.contains(fragment).then_some(label))
    .unwrap_or("unclassified command failure")
}

// Read two child pipes with one deadline. Keep only a bounded diagnostic tail.
fn drain(
    child: &mut Child,
    output: &mut File,
    limit: u64,
    deadline: Instant,
    stage: &str,
) -> Result<Vec<u8>, StorageError> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| failed(stage, "missing output pipe"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| failed(stage, "missing error pipe"))?;
    let mut channels = [
        libc::pollfd {
            fd: stdout.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: stderr.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let mut diagnostics = Vec::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 65536];
    while channels.iter().any(|poll| poll.fd >= 0) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(failed(stage, "command timed out"));
        }
        let millis = i32::try_from(remaining.as_millis())
            .unwrap_or(i32::MAX)
            .max(1);
        // poll borrows the two arrays of valid child pipe descriptors.
        let ready = unsafe { libc::poll(channels.as_mut_ptr(), channels.len() as _, millis) };
        if ready == 0 {
            return Err(failed(stage, "command timed out"));
        }
        if ready < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(failed(stage, "command pipe unavailable"));
        }
        for (index, channel) in channels.iter_mut().enumerate() {
            if channel.revents == 0 {
                continue;
            }
            if channel.revents & libc::POLLNVAL != 0 {
                return Err(failed(stage, "command pipe unavailable"));
            }
            let count = if index == 0 {
                stdout.read(&mut buffer)
            } else {
                stderr.read(&mut buffer)
            }
            .map_err(|_| failed(stage, "command pipe unavailable"))?;
            if count == 0 {
                channel.fd = -1;
            } else if index == 0 {
                total += count as u64;
                if total > limit {
                    return Err(failed(stage, "exceeded output limit"));
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(|_| failed(stage, "temporary file write failed"))?;
            } else {
                diagnostics.extend_from_slice(&buffer[..count]);
                if diagnostics.len() > 4096 {
                    diagnostics.drain(..diagnostics.len() - 4096);
                }
            }
        }
    }
    Ok(diagnostics)
}

pub(crate) fn collect(
    command: &mut Command,
    source: Option<&File>,
    output: &mut File,
    limit: u64,
    timeout: Duration,
    stage: &str,
) -> Result<(), StorageError> {
    let stdin = source
        .map(File::try_clone)
        .transpose()
        .map_err(|_| failed(stage, "input unavailable"))?
        .map_or_else(Stdio::null, Stdio::from);
    let deadline = Instant::now() + timeout;
    let mut job = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| failed(stage, "command could not start"))?;
    let complete = (|| {
        let diagnostics = drain(&mut job, output, limit, deadline, stage)?;
        let status = job
            .wait_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| failed(stage, "process status unavailable"))?
            .ok_or_else(|| failed(stage, "command timed out"))?;
        if !status.success() {
            let safe = safe_diagnostic(&diagnostics);
            let exit = status
                .code()
                .map_or_else(|| "signal".to_owned(), |code| code.to_string());
            return Err(reason(format!("{stage}: {safe} (exit {exit}).")));
        }
        output
            .rewind()
            .map_err(|_| failed(stage, "temporary file rewind failed"))
    })();
    if complete.is_err() && job.try_wait().ok().flatten().is_none() {
        if let Err(error) = job.kill().and_then(|()| job.wait().map(|_| ())) {
            eprintln!("Could not reap a storage subprocess: {}", error.kind());
        }
    }
    complete
}

fn metadata(
    base: &[String],
    args: &[&str],
    source: Option<&File>,
    timeout: Duration,
    stage: &str,
) -> Result<Vec<u8>, StorageError> {
    let mut job = command(base, args).ok_or_else(|| failed(stage, "command not configured"))?;
    let mut temp = temporary()?;
    collect(&mut job, source, &mut temp, MAX_METADATA, timeout, stage)?;
    let mut output = Vec::new();
    temp.read_to_end(&mut output)
        .map_err(|_| failed(stage, "temporary file read failed"))?;
    Ok(output)
}

pub(crate) fn temporary() -> Result<File, StorageError> {
    tempfile::tempfile().map_err(|_| reason("Private temporary file unavailable."))
}

pub(crate) fn checksum(source: &mut File, limit: u64) -> Result<(String, u64), StorageError> {
    source
        .rewind()
        .map_err(|_| reason("Checkpoint could not be read."))?;
    let digest = digest_to(&mut *source, io::sink(), limit)
        .map_err(|_| reason("Checkpoint could not be hashed within its size budget."))?;
    source
        .rewind()
        .map_err(|_| reason("Checkpoint could not be read."))?;
    Ok(digest)
}

fn snapshots(base: &[String], tag: &str, timeout: Duration) -> Result<Vec<Snapshot>, StorageError> {
    let output = metadata(
        base,
        &["snapshots", "--tag", tag, "--json"],
        None,
        timeout,
        STORAGE,
    )?;
    let rows: Vec<Snapshot> = serde_json::from_slice(&output)
        .map_err(|_| reason("Backup storage returned invalid snapshot records."))?;
    if rows.len() > 1 {
        return Err(reason("The export challenge has ambiguous backup history."));
    }
    Ok(rows)
}

fn managed(snapshot: &Snapshot, path: &str, tag: &str) -> bool {
    hex(&snapshot.id, 64)
        && snapshot.paths == [path]
        && [TAG, tag]
            .iter()
            .all(|tag| snapshot.tags.iter().any(|value| value == tag))
}

pub(crate) fn readback(
    base: &[String],
    id: &str,
    path: &str,
    dest: &mut File,
    limit: u64,
    timeout: Duration,
) -> Result<(), StorageError> {
    if !hex(id, 64) || path.strip_prefix('/').and_then(archive_identity).is_none() {
        return Err(reason("Unexpected checkpoint identity."));
    }
    let mut dump = command(base, &["dump", id, path])
        .ok_or_else(|| failed("storage readback", "command not configured"))?;
    collect(&mut dump, None, dest, limit, timeout, "storage readback")
}

fn archive_validator(path: &Path) -> Result<Command, StorageError> {
    let path = path.join("lab-restore-workspace");
    if !path.is_file() {
        return Err(reason(
            "Install lab-restore-workspace alongside the storage worker.",
        ));
    }
    Ok(Command::new(path))
}

pub(crate) fn store(
    restic: &[String],
    archive: &mut File,
    filename: &str,
    challenge: &str,
    limit: u64,
    timeout: Duration,
    binaries: &Path,
) -> Result<Value, StorageError> {
    if !hex(challenge, 32)
        || archive_identity(filename)
            .is_none_or(|(_, point)| point.is_some_and(|point| point != challenge))
    {
        return Err(reason("Invalid archive or storage operation identity."));
    }
    let (expected, bytes) = checksum(archive, limit.min(MAX_ARCHIVE))?;
    if bytes == 0 {
        return Err(reason("Archive exceeds the supported restore size."));
    }
    {
        // The parent owns the temporary directory and removes any partial
        // plaintext if the child validator exceeds its process limits.
        let target = tempfile::tempdir()
            .map_err(|_| reason("Workspace validation directory unavailable."))?;
        let mut validator = archive_validator(binaries)?;
        validator.arg("--validate").arg(target.path());
        collect(
            &mut validator,
            Some(archive),
            &mut temporary()?,
            1024,
            timeout,
            "Workspace restore validation",
        )?;
    }
    archive
        .rewind()
        .map_err(|_| reason("Archive could not be read."))?;
    let path = format!("/{filename}");
    let tag = format!("lease-export-{challenge}");
    let existing = snapshots(restic, &tag, timeout)?;
    let snapshot = if let Some(snapshot) = existing.into_iter().next() {
        if !managed(&snapshot, &path, &tag) {
            return Err(reason(
                "The tagged snapshot belongs to a different checkpoint.",
            ));
        }
        snapshot.id
    } else {
        let data = metadata(
            restic,
            &[
                "backup",
                "--stdin",
                "--stdin-filename",
                filename,
                "--tag",
                TAG,
                "--tag",
                &tag,
                "--json",
            ],
            Some(archive),
            timeout,
            "storage backup",
        )?;
        let mut ids = Vec::new();
        for line in data
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
        {
            let record: Value = serde_json::from_slice(line)
                .map_err(|_| reason("Backup returned invalid JSON."))?;
            if record["message_type"] == "summary" {
                ids.push(
                    record["snapshot_id"]
                        .as_str()
                        .filter(|id| hex(id, 64))
                        .ok_or_else(|| reason("Backup did not return a full snapshot ID."))?
                        .to_owned(),
                );
            }
        }
        if ids.len() != 1 {
            return Err(reason("Backup did not report one snapshot."));
        }
        ids.remove(0)
    };
    let mut restored = temporary()?;
    readback(restic, &snapshot, &path, &mut restored, limit, timeout)?;
    if checksum(&mut restored, limit)?.0 != expected {
        return Err(reason("Downloaded checkpoint differs from the source."));
    }
    Ok(json!({"snapshot":snapshot,"path":path,"sha256":expected}))
}

fn default_limit() -> u64 {
    MAX_ARCHIVE
}
fn default_timeout() -> u64 {
    120
}

#[derive(Deserialize)]
pub(crate) struct StorageSettings {
    pub restic_command: Vec<String>,
    pub upload_command: Option<Vec<String>>,
    retention_command: Option<Vec<String>>,
    #[serde(default = "default_limit")]
    pub max_archive_bytes: u64,
    #[serde(default = "default_timeout")]
    command_timeout: u64,
}

#[derive(Deserialize)]
struct ExporterSettings {
    #[serde(flatten)]
    storage: StorageSettings,
    endpoint: String,
    ca_file: PathBuf,
    verifier_token_file: PathBuf,
    lock_file: PathBuf,
    slots: Option<std::collections::BTreeMap<String, CaptureSlot>>,
    #[serde(flatten)]
    primary: Value,
    #[serde(default)]
    purge_expired_checkpoints: bool,
}

#[derive(Deserialize)]
struct CaptureSlot {
    guest_id: Value,
    capture_command: Vec<String>,
    #[serde(default)]
    workspace_capture: bool,
    #[serde(default)]
    dynamic_capture: bool,
}

#[derive(Deserialize)]
struct ExportJob {
    lease_id: String,
    challenge: String,
    guest_id: String,
    slot_id: Option<String>,
    dynamic_source: Option<Vec<String>>,
    workspace_source: Option<String>,
    storage_challenge: String,
}

fn capture_command<'a>(slot: &'a CaptureSlot, job: &'a ExportJob) -> Result<Command, StorageError> {
    let guest = slot
        .guest_id
        .as_str()
        .map(str::to_owned)
        .or_else(|| slot.guest_id.as_u64().map(|id| id.to_string()));
    if !hex(&job.lease_id, 32)
        || !hex(&job.challenge, 32)
        || guest.as_deref() != Some(&job.guest_id)
    {
        return Err(reason("Export job does not match the configured slot."));
    }
    if slot.workspace_capture
        && (job.dynamic_source.is_some()
            || !job
                .workspace_source
                .as_ref()
                .is_some_and(|source| hex(source, 64)))
    {
        return Err(reason("Missing exact Apple workspace generation."));
    }
    if !slot.workspace_capture && job.workspace_source.is_some() {
        return Err(reason(
            "An Apple workspace requires the enrolled capture role.",
        ));
    }
    if slot.dynamic_capture
        && !job.dynamic_source.as_ref().is_some_and(|sources| {
            sources.len() == 4 && sources.iter().all(|source| !source.is_empty())
        })
    {
        return Err(reason("Missing enrolled dynamic capture identity."));
    }
    if !slot.dynamic_capture && job.dynamic_source.is_some() {
        return Err(reason(
            "A dynamic guest cannot use a fixed capture command.",
        ));
    }
    if !hex(&job.storage_challenge, 32) {
        return Err(reason("Invalid storage operation identity."));
    }
    let mut command = command(&slot.capture_command, &[&job.guest_id, &job.lease_id])
        .ok_or_else(|| reason("Missing scoped capture command."))?;
    if let Some(source) = &job.workspace_source {
        command.arg(source);
    }
    if let Some(sources) = &job.dynamic_source {
        command.args(sources);
    }
    Ok(command)
}

fn export(
    settings: &ExporterSettings,
    job: &ExportJob,
    binaries: &Path,
) -> Result<Value, StorageError> {
    let slot_id = job.slot_id.as_deref().unwrap_or("primary");
    let default;
    let slot = if let Some(slots) = &settings.slots {
        slots.get(slot_id)
    } else if slot_id == "primary" {
        default = serde_json::from_value(settings.primary.clone())
            .map_err(|_| reason("Could not verify the primary capture configuration."))?;
        Some(&default)
    } else {
        None
    }
    .ok_or_else(|| reason("Export job does not match the configured slot."))?;
    let mut capture = capture_command(slot, job)?;
    let storage = &settings.storage;
    let timeout = Duration::from_secs(storage.command_timeout);
    let mut archive = temporary()?;
    collect(
        &mut capture,
        None,
        &mut archive,
        storage.max_archive_bytes,
        timeout,
        "stopped disk capture",
    )?;
    let mut receipt = store(
        &storage.restic_command,
        &mut archive,
        &format!("lease-{}.tar.gz", job.lease_id),
        &job.storage_challenge,
        storage.max_archive_bytes,
        timeout,
        binaries,
    )?;
    receipt["outcome"] = json!("verified");
    receipt["challenge"] = json!(job.challenge);
    receipt["storage_managed"] = json!(true);
    receipt["restore_verified"] = json!(true);
    Ok(receipt)
}

#[derive(Deserialize)]
struct Expiry {
    lease_id: String,
    challenge: String,
    snapshot: Option<String>,
    checkpoint_id: Option<String>,
    path: String,
    claim: String,
    #[serde(default)]
    unacknowledged: bool,
}

fn expire(settings: &StorageSettings, job: Expiry) -> Result<Value, StorageError> {
    let scope = job.path.strip_prefix('/').and_then(archive_identity);
    let mut known = job.snapshot;
    if !hex(&job.lease_id, 32)
        || !hex(&job.challenge, 32)
        || !hex(&job.claim, 32)
        || known
            .as_ref()
            .map_or(!job.unacknowledged, |id| !hex(id, 64))
        || job
            .checkpoint_id
            .as_ref()
            .is_some_and(|id| id != &job.challenge)
        || scope != Some((&job.lease_id, job.checkpoint_id.as_deref()))
    {
        return Err(reason("Expiry job has an invalid checkpoint identity."));
    }
    let restic = settings
        .retention_command
        .as_ref()
        .filter(|args| !args.is_empty())
        .unwrap_or(&settings.restic_command);
    let timeout = Duration::from_secs(settings.command_timeout);
    let tag = format!("lease-export-{}", job.challenge);
    if let Some(existing) = snapshots(restic, &tag, timeout)?.into_iter().next() {
        if !managed(&existing, &job.path, &tag)
            || known.as_ref().is_some_and(|known| known != &existing.id)
        {
            return Err(reason("Expiry would affect a different snapshot."));
        }
        known = Some(existing.id);
        metadata(
            restic,
            &["forget", known.as_deref().unwrap_or_default(), "--json"],
            None,
            timeout,
            "storage expiry",
        )?;
    }
    let output = metadata(
        restic,
        &["list", "snapshots"],
        None,
        timeout,
        "storage expiry readback",
    )?;
    let inventory = std::str::from_utf8(&output)
        .map_err(|_| reason("Backup storage did not return a valid snapshot inventory."))?;
    for id in inventory.split_ascii_whitespace() {
        if !hex(id, 64) {
            return Err(reason(
                "Backup storage did not return a valid snapshot inventory.",
            ));
        }
        if known.as_deref() == Some(id) {
            return Err(reason(
                "The expired checkpoint is still visible in storage.",
            ));
        }
    }
    if job.unacknowledged && !snapshots(restic, &tag, timeout)?.is_empty() {
        return Err(reason(
            "The expired checkpoint is still visible in storage.",
        ));
    }
    Ok(
        json!({"lease_id":job.lease_id,"checkpoint_id":job.checkpoint_id,"snapshot":known,
        "claim":job.claim,"absent":true,"unacknowledged":job.unacknowledged}),
    )
}

pub fn worker_lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(error),
    }
}

pub fn run_exporter(config: &Path) -> Result<bool, Box<dyn Error>> {
    let settings: ExporterSettings = serde_json::from_reader(File::open(config)?)?;
    let Some(_guard) = worker_lock(&settings.lock_file)? else {
        return Ok(true);
    };
    let client = TaskClient::new(
        &settings.endpoint,
        settings.ca_file.clone(),
        settings.verifier_token_file.clone(),
    )?;
    let current = client.request("/v1/final-export", None)?;
    let Some(job) = current.get("export").filter(|job| !job.is_null()) else {
        if settings.purge_expired_checkpoints {
            let expiry = client.request("/v1/checkpoint-expiry", Some(&json!({})))?;
            if let Some(job) = expiry.get("expiry").filter(|job| !job.is_null()) {
                let receipt = expire(&settings.storage, serde_json::from_value(job.clone())?)?;
                client.request("/v1/checkpoint-expiry", Some(&receipt))?;
                println!("Verified removal of the expired snapshot.");
            }
        }
        return Ok(true);
    };
    let lease = job["lease_id"]
        .as_str()
        .filter(|lease| hex(lease, 32))
        .ok_or_else(|| reason("Invalid lease identifier from the controller."))?;
    let challenge = job["challenge"]
        .as_str()
        .filter(|id| hex(id, 32))
        .ok_or_else(|| reason("Invalid export challenge from the controller."))?;
    let executable = std::env::current_exe()?;
    let binaries = executable
        .parent()
        .ok_or_else(|| reason("Storage worker location is unverified."))?;
    let route = format!("/v1/final-export/{lease}");
    let job =
        serde_json::from_value(job.clone()).map_err(|_| reason("Unexpected export job format."));
    match job.and_then(|job| export(&settings, &job, binaries)) {
        Ok(receipt) => {
            client.request(&route, Some(&receipt))?;
            println!("Verified final checkpoint stored and read back.");
            Ok(true)
        }
        Err(error) => {
            let explanation: String = error.to_string().chars().take(240).collect();
            client.request(
                &route,
                Some(&json!({"outcome":"failed","challenge":challenge,"reason":explanation})),
            )?;
            eprintln!("Final export failed: {explanation}");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exporter_never_routes_dynamic_sources_to_fixed_capture() {
        let slot = CaptureSlot {
            guest_id: json!(97),
            capture_command: vec!["fixed-capture".into()],
            workspace_capture: false,
            dynamic_capture: false,
        };
        let mut job = ExportJob {
            lease_id: "a".repeat(32),
            challenge: "b".repeat(32),
            guest_id: "97".into(),
            slot_id: None,
            dynamic_source: Some(vec!["unexpected source".into()]),
            workspace_source: None,
            storage_challenge: "a".repeat(32),
        };
        assert!(
            capture_command(&slot, &job)
                .unwrap_err()
                .to_string()
                .contains("fixed capture")
        );
        job.dynamic_source = None;
        let command = capture_command(&slot, &job).unwrap();
        assert_eq!(command.get_program(), "fixed-capture");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["97", &job.lease_id]
        );
    }

    #[test]
    fn process_limits_and_diagnostics_do_not_echo_secret_content() {
        let mut out = temporary().unwrap();
        let seconds = Duration::from_secs(5);
        let mut fail = Command::new("/bin/sh");
        fail.args(["-c", "printf 'private detail: no space left' >&2; exit 7"]);
        assert_eq!(
            collect(&mut fail, None, &mut out, 100, seconds, "backup")
                .unwrap_err()
                .to_string(),
            "backup: storage full (exit 7)."
        );
        let mut overflow = Command::new("/bin/sh");
        overflow.args(["-c", "printf 'longer than limit'"]);
        assert_eq!(
            collect(&mut overflow, None, &mut out, 1, seconds, "backup")
                .unwrap_err()
                .to_string(),
            "backup: exceeded output limit."
        );
    }

    #[test]
    fn existing_default_and_multislot_configuration_preserve_scope() {
        let settings: ExporterSettings = serde_json::from_value(json!({
            "endpoint":"https://example.invalid", "ca_file":"/cert","verifier_token_file":"/token",
            "lock_file":"/lock","restic_command":["storage"],"guest_id":97,"capture_command":["capture"]
        })).unwrap();
        assert_eq!(settings.storage.max_archive_bytes, MAX_ARCHIVE);
        assert!(settings.slots.is_none());
        let slot: CaptureSlot = serde_json::from_value(settings.primary).unwrap();
        assert_eq!(slot.guest_id, 97);
        assert_eq!(slot.capture_command, ["capture"]);
    }
}
