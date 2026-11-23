use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::Duration,
};

use clap::Parser;
use flate2::read::MultiGzDecoder;
use serde::Deserialize;
use serde_json::Value;
use wait_timeout::ChildExt;

use lab_allocator::workspace::MAX_ARCHIVE;

const MIB: u64 = 1024 * 1024;
const ROOT_TYPE: &str = "4f68bce3-e8cd-4db1-96e7-fbcaf984b709";
const MARKER: &str = "/etc/ssh/lease_authorized_keys";

#[derive(Parser)]
#[command(about = "Capture one declared workspace from an immutable stopped Linux slot.")]
struct Options {
    configuration: PathBuf,
    guest: String,
    lease: String,
    scope: Vec<String>,
}

#[derive(Deserialize)]
struct Settings {
    guest_id: Value,
    workspace: String,
    disk_bytes: u64,
    max_spool_bytes: u64,
    source_command: Vec<String>,
    spool_dir: PathBuf,
    #[serde(default)]
    dynamic_source: bool,
    appliance_memory_mib: Option<u32>,
}

fn reject(reason: &str) -> io::Error {
    io::Error::other(reason)
}

fn decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn disk_identity(volume: &str) -> bool {
    let Some((storage, target)) = volume.split_once(':') else {
        return false;
    };
    let Some((guest, disk)) = target.split_once('/') else {
        return false;
    };
    let Some((id, number)) = disk
        .strip_prefix("vm-")
        .and_then(|disk| disk.strip_suffix(".raw"))
        .and_then(|disk| disk.split_once("-disk-"))
    else {
        return false;
    };
    !storage.is_empty()
        && storage
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && decimal(guest)
        && decimal(id)
        && decimal(number)
}

impl Settings {
    fn scope(&mut self, args: &Options) -> io::Result<()> {
        let expected = self
            .guest_id
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.guest_id.as_u64().map(|id| id.to_string()));
        if expected.as_deref() != Some(&args.guest)
            || args.lease.len() != 32
            || !args
                .lease
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(reject(
                "Capture identity does not match the configured slot.",
            ));
        }
        let workspace = Path::new(&self.workspace);
        if !workspace.is_absolute()
            || workspace == Path::new("/")
            || workspace
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            || workspace.components().collect::<PathBuf>().as_os_str() != workspace.as_os_str()
            || self.workspace.contains('\0')
        {
            return Err(reject(
                "Configure an absolute workspace directory below the root.",
            ));
        }
        if self.source_command.is_empty() || self.source_command[0].is_empty() {
            return Err(reject("Configure a fixed stopped-slot source command."));
        }
        if !self.dynamic_source {
            return if args.scope.is_empty() {
                Ok(())
            } else {
                Err(reject(
                    "A fixed capture does not accept dynamic source parameters.",
                ))
            };
        }
        let [hardware, generation, volume, size] = args.scope.as_slice() else {
            return Err(reject("Invalid dynamic capture source identity."));
        };
        let valid_id =
            |value: &str| uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value);
        let requested = size
            .parse::<u64>()
            .ok()
            .filter(|value| (1..=9_999_999).contains(value))
            .filter(|_| decimal(size) && !size.starts_with('0'))
            .map(|size| size * MIB)
            .ok_or_else(|| reject("Invalid dynamic capture source identity."))?;
        if !valid_id(hardware) || !valid_id(generation) || !disk_identity(volume) {
            return Err(reject("Invalid dynamic capture source identity."));
        }
        if requested > self.disk_bytes {
            return Err(reject("Dynamic capture exceeds the enrolled disk scope."));
        }
        self.disk_bytes = requested;
        self.max_spool_bytes = self.max_spool_bytes.min(requested);
        self.source_command
            .extend(["read-dynamic".to_owned(), args.lease.clone()]);
        self.source_command.extend(args.scope.clone());
        Ok(())
    }
}

fn spool(settings: &Settings, image: &mut File) -> io::Result<()> {
    let budget = settings.max_spool_bytes;
    if budget == 0 || budget > settings.disk_bytes {
        return Err(reject("Invalid disk or temporary storage budget."));
    }
    if lab_allocator::available_disk(&settings.spool_dir)?.unsigned_abs() < budget {
        return Err(reject(
            "Insufficient temporary space for the configured capture budget.",
        ));
    }
    let mut source = Command::new("/usr/bin/timeout")
        .args(["--kill-after=5s", "180"])
        .args(&settings.source_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let result = (|| {
        let pipe = source
            .stdout
            .take()
            .ok_or_else(|| reject("Stopped source unavailable."))?;
        let mut gzip = MultiGzDecoder::new(pipe);
        let mut buffer = vec![0; MIB as usize];
        let (mut total, mut allocated) = (0_u64, 0_u64);
        loop {
            // Fill each block before counting it against the sparse image budget.
            let mut count = 0;
            while count < buffer.len() {
                let size = gzip.read(&mut buffer[count..])?;
                if size == 0 {
                    break;
                }
                count += size;
            }
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > settings.disk_bytes {
                return Err(reject("Source exceeded its registered disk size."));
            }
            let chunk = &buffer[..count];
            if chunk.iter().all(|byte| *byte == 0) {
                image.seek(SeekFrom::Current(count as i64))?;
            } else {
                allocated += count as u64;
                if allocated > budget {
                    return Err(reject("Source exceeded its temporary storage budget."));
                }
                image.write_all(chunk)?;
            }
        }
        if total != settings.disk_bytes
            || !source
                .wait_timeout(Duration::from_secs(10))?
                .is_some_and(|status| status.success())
        {
            return Err(reject("Stopped-slot source was incomplete."));
        }
        image.set_len(total)?;
        image.sync_all()
    })();
    if result.is_err() && source.try_wait()?.is_none() {
        // The timeout supervisor forwards this to its capture process group.
        unsafe { libc::kill(source.id() as libc::pid_t, libc::SIGTERM) };
        if source.wait_timeout(Duration::from_secs(6))?.is_none() {
            source.kill()?;
            source.wait()?;
        }
    }
    result
}

struct Appliance(String);

impl Appliance {
    fn start() -> io::Result<Self> {
        let response = lab_allocator::bounded_command(
            Command::new("guestfish").args(["--listen", "--ro"]),
            None,
            Duration::from_secs(10),
            4096,
        )?;
        let reply = String::from_utf8_lossy(&response.stdout);
        let id = reply
            .split(';')
            .next()
            .and_then(|text| text.trim().strip_prefix("GUESTFISH_PID="))
            .filter(|value| value.parse::<u32>().is_ok_and(|id| id > 1));
        if !response.status.success() || id.is_none() {
            return Err(reject("Could not start the read-only guestfish appliance."));
        }
        Ok(Self(id.unwrap_or_default().to_owned()))
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("guestfish");
        command.arg(format!("--remote={}", self.0)).args(args);
        command
    }

    fn read(&self, args: &[&str], seconds: u64, limit: u64) -> io::Result<String> {
        let out = lab_allocator::bounded_command(
            &mut self.command(args),
            None,
            Duration::from_secs(seconds),
            limit,
        )?;
        if !out.status.success() {
            return Err(reject("Read-only guest filesystem command failed."));
        }
        String::from_utf8(out.stdout).map_err(io::Error::other)
    }
}

impl Drop for Appliance {
    fn drop(&mut self) {
        if self.read(&["exit"], 10, 4096).is_err() {
            eprintln!("Read-only guestfish appliance did not acknowledge shutdown.");
        }
    }
}

fn capture(settings: &Settings, lease: &str, image: &Path) -> io::Result<()> {
    let path = image
        .to_str()
        .ok_or_else(|| reject("Use a textual spool directory path."))?;
    let appliance = Appliance::start()?;
    for args in [
        vec!["set-backend", "direct"],
        vec!["set-network", "false"],
        vec![
            "set-memsize",
            &settings.appliance_memory_mib.unwrap_or(512).to_string(),
        ],
        vec!["add-drive-opts", path, "readonly:true", "format:raw"],
    ] {
        appliance.read(&args, 15, 4096)?;
    }
    appliance.read(&["run"], 90, 4096)?;
    let devices = appliance.read(&["list-devices"], 10, 4096)?;
    let mut devices = devices.lines();
    let disk = devices
        .next()
        .filter(|disk| disk.starts_with("/dev/") && devices.next().is_none())
        .ok_or_else(|| reject("Expected one stopped source disk."))?;
    let parts = appliance.read(&["list-partitions"], 10, 4096)?;
    let mut roots = Vec::new();
    for part in parts.lines() {
        let number = appliance.read(&["part-to-partnum", part], 10, 4096)?;
        let kind = appliance.read(&["part-get-gpt-type", disk, number.trim()], 10, 4096)?;
        if kind.trim().eq_ignore_ascii_case(ROOT_TYPE) {
            roots.push(part);
        }
    }
    let [root] = roots.as_slice() else {
        return Err(reject(
            "Expected one Linux root partition in the stopped source.",
        ));
    };
    appliance.read(
        &[
            "mount-vfs",
            "ro,noload,nodev,nosuid,noexec",
            "ext4",
            root,
            "/",
        ],
        15,
        4096,
    )?;
    let inspect = |command, path| appliance.read(&[command, path], 10, 4096);
    let mode = inspect("statns", MARKER)?;
    if inspect("realpath", MARKER)?.trim() != MARKER
        || inspect("is-file", MARKER)?.trim() != "true"
        || !inspect("filesize", MARKER)?
            .trim()
            .parse::<u16>()
            .is_ok_and(|size| size <= 256)
        || !mode.lines().any(|line| {
            line.split_once(':')
                .is_some_and(|(field, value)| field.trim() == "st_uid" && value.trim() == "0")
        })
    {
        return Err(reject(
            "Missing protected lease identity on the source disk.",
        ));
    }
    let evidence = appliance.read(&["cat", MARKER], 10, 256)?;
    let fields = evidence.split_whitespace().collect::<Vec<_>>();
    if !matches!(fields.as_slice(), ["ssh-ed25519", _, owner] if *owner == format!("lease-{lease}"))
    {
        return Err(reject("The source disk belongs to another lease."));
    }
    if inspect("realpath", &settings.workspace)?.trim() != settings.workspace
        || inspect("is-dir", &settings.workspace)?.trim() != "true"
    {
        return Err(reject(
            "The declared workspace is missing or resolves outside its configured path.",
        ));
    }
    let mut archive = tempfile::tempfile()?;
    let mut command = appliance.command(&[
        "tar-out",
        &settings.workspace,
        "-",
        "compress:gzip",
        "numericowner:true",
    ]);
    lab_allocator::collect_capture(
        &mut command,
        &mut archive,
        MAX_ARCHIVE,
        Duration::from_secs(90),
    )?;
    io::copy(&mut archive, &mut io::stdout().lock())?;
    Ok(())
}

fn run(args: Options) -> Result<(), Box<dyn std::error::Error>> {
    let mut settings: Settings = serde_json::from_reader(File::open(&args.configuration)?)?;
    settings.scope(&args)?;
    let directory = tempfile::tempdir_in(&settings.spool_dir)?;
    let image = directory.path().join("slot.raw");
    {
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&image)?;
        spool(&settings, &mut output)?;
    }
    fs::set_permissions(&image, fs::Permissions::from_mode(0o400))?;
    capture(&settings, &args.lease, &image)?;
    Ok(())
}

fn main() -> ExitCode {
    let options = Options::parse();
    unsafe { libc::umask(0o077) };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Cold capture failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_storage_ids_and_dimensions_are_constrained_before_any_process() {
        assert!(disk_identity("scope-ssd:9730/vm-9730-disk-0.raw"));
        for name in [
            "zfs:9730/vm-9730-disk-0",
            "pool:../../../x",
            "a:9730/vm--disk-1.raw",
            "x:/vm-99-disk-2.raw",
        ] {
            assert!(!disk_identity(name), "{name}");
        }
    }
}
