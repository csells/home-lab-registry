use std::{
    error::Error,
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use lab_allocator::{
    bounded_command,
    task_client::{TaskClient, store_task_token},
    task_resume::discover,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const CREDENTIALS: &str = "/v1/task-credentials";
const TOKEN: &str = "task.token";
const MANIFEST: &str = "session.json";

#[derive(Parser)]
#[command(about = "Prepare the scoped access files for a fresh or resumed task.")]
struct Options {
    configuration: PathBuf,
    task: String,
    #[arg(required_unless_present = "reuse", conflicts_with = "reuse")]
    directory: Option<PathBuf>,
    #[arg(long)]
    reuse: bool,
    #[arg(long, default_value_t=3600, value_parser=clap::value_parser!(u64).range(1..))]
    seconds: u64,
}

#[derive(Deserialize)]
struct Settings {
    registry_client: PathBuf,
    registry_record: String,
    registry_config: Option<PathBuf>,
    ca_file: PathBuf,
    issuer_token_file: PathBuf,
    task_sessions: Option<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u64,
    endpoint: String,
    expires_at: f64,
}

fn problem(summary: &'static str) -> io::Error {
    io::Error::other(summary)
}

fn task_ref(value: &str) -> bool {
    (1..=96).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn private(path: &Path) -> io::Result<()> {
    let observed = fs::symlink_metadata(path)?;
    // geteuid reads the launcher's process identity without side effects.
    let uid = unsafe { libc::geteuid() };
    if !observed.is_dir() || observed.uid() != uid || observed.mode() & 0o077 != 0 {
        return Err(problem(
            "The task directory must be private to its launch user.",
        ));
    }
    Ok(())
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

impl Settings {
    fn client(&self) -> Result<TaskClient, Box<dyn Error>> {
        let endpoint = discover(
            &self.registry_client,
            self.registry_config.as_deref(),
            &self.registry_record,
        )?;
        Ok(TaskClient::new(
            &endpoint,
            self.ca_file.clone(),
            self.issuer_token_file.clone(),
        )?)
    }
}

fn write_manifest(
    directory: &Path,
    client: &TaskClient,
    grant: &mut Value,
) -> Result<(), Box<dyn Error>> {
    let expires_at = grant["expires_at"]
        .as_f64()
        .filter(|stamp| stamp.is_finite() && *stamp > now())
        .ok_or_else(|| problem("The task issuer did not return an active credential."))?;
    store_task_token(grant, &directory.join(TOKEN))?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.join(MANIFEST))?;
    serde_json::to_writer(
        &file,
        &Manifest {
            version: 1,
            endpoint: client.endpoint.clone(),
            expires_at,
        },
    )?;
    file.sync_all()?;
    Ok(())
}

fn prepare(
    settings: &Settings,
    options: &Options,
    directory: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    let parent = directory
        .parent()
        .ok_or_else(|| problem("Use an exact absolute path for the new task workspace."))?;
    let exact = parent.canonicalize()?.join(
        directory
            .file_name()
            .ok_or_else(|| problem("Invalid task directory."))?,
    );
    if !directory.is_absolute() || directory.as_os_str() != exact.as_os_str() {
        return Err(problem("Use an exact absolute path for the new task workspace.").into());
    }
    match directory.symlink_metadata() {
        Ok(_) => return Err(problem("The task directory already exists.").into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let client = settings.client()?;
    let pending = tempfile::Builder::new()
        .prefix(".task-")
        .tempdir_in(parent)?;
    fs::copy(&settings.ca_file, pending.path().join("ca.pem"))?;
    let key = pending.path().join("identity");
    let result = bounded_command(
        Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(key),
        None,
        Duration::from_secs(12),
        4096,
    )?;
    if !result.status.success() {
        return Err(problem("The task SSH key could not be created.").into());
    }
    let mut grant = client.request(
        CREDENTIALS,
        Some(&json!({"task_ref":options.task,"seconds":options.seconds})),
    )?;
    if let Err(error) = write_manifest(pending.path(), &client, &mut grant)
        .and_then(|()| fs::rename(pending.path(), directory).map_err(Into::into))
    {
        if let Err(problem) = client.request(
            CREDENTIALS,
            Some(&json!({"task_ref":options.task,"revoke":true})),
        ) {
            eprintln!(
                "Task credential rollback could not be confirmed: {}",
                problem.response["code"]
            );
        }
        return Err(error);
    }
    Ok(directory.join(MANIFEST))
}

fn refresh(
    settings: &Settings,
    options: &Options,
    directory: &Path,
) -> Result<PathBuf, Box<dyn Error>> {
    private(directory)?;
    let manifest = directory.join(MANIFEST);
    let token = directory.join(TOKEN);
    if manifest.symlink_metadata()?.is_symlink()
        || token.symlink_metadata().is_ok_and(|file| file.is_symlink())
    {
        return Err(problem("Task session files cannot be symbolic links.").into());
    }
    let current: Manifest = serde_json::from_reader(File::open(&manifest)?)?;
    if current.version != 1 || !current.expires_at.is_finite() {
        return Err(problem("Unsupported task session.").into());
    }
    if token.is_file() && current.expires_at - now() > (options.seconds as f64 / 4.0).min(300.0) {
        return Ok(manifest);
    }
    let client = settings.client()?;
    if current.endpoint != client.endpoint
        || fs::read(&settings.ca_file)? != fs::read(directory.join("ca.pem"))?
    {
        return Err(problem("The registered allocator identity changed for this task.").into());
    }
    let pending = tempfile::tempdir_in(directory)?;
    let mut grant = client.request(
        CREDENTIALS,
        Some(&json!({"task_ref":options.task,"seconds":options.seconds})),
    )?;
    write_manifest(pending.path(), &client, &mut grant)?;
    fs::rename(pending.path().join(TOKEN), token)?;
    fs::rename(pending.path().join(MANIFEST), &manifest)?;
    File::open(directory)?.sync_all()?;
    Ok(manifest)
}

fn run(options: Options) -> Result<PathBuf, Box<dyn Error>> {
    if !task_ref(&options.task) {
        return Err(problem("Expected a stable task identity from the launcher.").into());
    }
    let settings: Settings = serde_json::from_reader(File::open(&options.configuration)?)?;
    if let Some(directory) = &options.directory {
        return prepare(&settings, &options, directory);
    }
    let root = settings
        .task_sessions
        .as_ref()
        .ok_or_else(|| problem("Configure a private session directory."))?;
    if !root.is_absolute() || root.canonicalize()? != *root {
        return Err(problem("Use an existing, exact private session directory.").into());
    }
    private(root)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(format!("{}.lock", options.task)))?;
    lock.lock()?;
    let directory = root.join(&options.task);
    match directory.symlink_metadata() {
        Ok(_) => refresh(&settings, &options, &directory),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            prepare(&settings, &options, &directory)
        }
        Err(error) => Err(error.into()),
    }
}

fn main() -> ExitCode {
    let options = Options::parse();
    unsafe { libc::umask(0o077) };
    match run(options) {
        Ok(path) => {
            println!("{}", json!({"session":path}));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Task launcher could not prepare access: {error}");
            ExitCode::FAILURE
        }
    }
}
