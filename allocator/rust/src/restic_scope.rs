use std::{error::Error, fmt, fs, path::Path, process::Command, time::Duration};

use serde::Deserialize;

use crate::{process::bounded, tokens::hex};

pub(crate) const TAG: &str = "lease-final-export";

pub(crate) fn challenge_tag(tag: &str) -> bool {
    tag.strip_prefix("lease-export-")
        .is_some_and(|id| hex(id, 32))
}

pub(crate) fn archive_identity(filename: &str) -> Option<(&str, Option<&str>)> {
    let id = filename.strip_prefix("lease-")?.strip_suffix(".tar.gz")?;
    let (lease, checkpoint) = id.split_once('-').map_or((id, None), |(a, b)| (a, Some(b)));
    (hex(lease, 32) && checkpoint.is_none_or(|point| hex(point, 32))).then_some((lease, checkpoint))
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Read,
    Write,
    Expire,
}

#[derive(Deserialize)]
struct Settings {
    role: Role,
    restic_command: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct Snapshot {
    pub id: String,
    pub paths: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug)]
pub enum ScopeError {
    Denied,
    Verification,
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Denied => "Command is outside the storage credential scope.",
            Self::Verification => "Storage verification failed.",
        })
    }
}
impl Error for ScopeError {}

pub(crate) fn command(base: &[String], args: &[&str]) -> Option<Command> {
    let (executable, prefix) = base.split_first()?;
    (!executable.is_empty()).then(|| {
        let mut command = Command::new(executable);
        command.args(prefix).args(args);
        command
    })
}

fn verify_managed(base: &[String], snapshot: &str, path: Option<&str>) -> Result<(), ScopeError> {
    let mut command =
        command(base, &["snapshots", snapshot, "--json"]).ok_or(ScopeError::Verification)?;
    let output = bounded(&mut command, None, Duration::from_secs(30), 1024 * 1024)
        .map_err(|_| ScopeError::Verification)?;
    if !output.status.success() {
        return Err(ScopeError::Verification);
    }
    let rows: Vec<Snapshot> =
        serde_json::from_slice(&output.stdout).map_err(|_| ScopeError::Verification)?;
    let [row] = rows.as_slice() else {
        return Err(ScopeError::Denied);
    };
    let [location] = row.paths.as_slice() else {
        return Err(ScopeError::Denied);
    };
    if row.id != snapshot
        || location
            .strip_prefix('/')
            .and_then(archive_identity)
            .is_none()
        || path.is_some_and(|expected| expected != location)
        || !row.tags.iter().any(|tag| tag == TAG)
        || row.tags.iter().filter(|tag| challenge_tag(tag)).count() != 1
    {
        return Err(ScopeError::Denied);
    }
    Ok(())
}

pub fn run(config: &Path, arguments: &[String], ssh: Option<&str>) -> Result<i32, ScopeError> {
    let data = fs::File::open(config).map_err(|_| ScopeError::Verification)?;
    let settings: Settings = serde_json::from_reader(data).map_err(|_| ScopeError::Verification)?;
    let original;
    let arguments = if arguments == ["--ssh-original-command"] {
        original = shlex::split(
            ssh.filter(|value| (1..=1024).contains(&value.len()))
                .ok_or(ScopeError::Denied)?,
        )
        .ok_or(ScopeError::Denied)?;
        &original
    } else {
        arguments
    };
    let args = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    let base = &settings.restic_command;
    match (&settings.role, args.as_slice()) {
        (Role::Write | Role::Expire, ["snapshots", "--tag", tag, "--json"])
            if challenge_tag(tag) => {}
        (
            Role::Write,
            [
                "backup",
                "--stdin",
                "--stdin-filename",
                file,
                "--tag",
                TAG,
                "--tag",
                tag,
                "--json",
            ],
        ) if challenge_tag(tag)
            && archive_identity(file).is_some_and(|(_, point)| {
                point.is_none_or(|point| tag.strip_prefix("lease-export-") == Some(point))
            }) => {}
        (Role::Read | Role::Write, ["dump", id, path])
            if hex(id, 64) && path.strip_prefix('/').and_then(archive_identity).is_some() =>
        {
            verify_managed(base, id, Some(path))?;
        }
        (Role::Expire, ["list", "snapshots"]) => {}
        (Role::Expire, ["forget", id, "--json"]) if hex(id, 64) => {
            verify_managed(base, id, None)?;
        }
        _ => return Err(ScopeError::Denied),
    }
    let status = command(base, &args)
        .ok_or(ScopeError::Verification)?
        .status()
        .map_err(|_| ScopeError::Verification)?;
    Ok(status.code().unwrap_or(1))
}
