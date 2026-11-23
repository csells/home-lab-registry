use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Seek, Write},
    net::Ipv4Addr,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio_util::io::ReaderStream;

use crate::{
    Rejected,
    capacity::MIB,
    process::bounded,
    proxmox_dynamic::Shape,
    request::public_key,
    tokens::{hash, hex, valid_token},
    workspace::{MAX_ARCHIVE, archive_workspace, digest_to},
};

const SHARE: &str = "/Volumes/My Shared Files/workspace";
const PREPARE_SSH: &str = include_str!("tart_guest_ssh.sh");

#[derive(Deserialize)]
struct Settings {
    state: PathBuf,
    tart_home: PathBuf,
    tart_binary: PathBuf,
    template: String,
    template_config_sha256: String,
    identity: String,
    shape: Shape,
    guest_network: Ipv4Net,
    interface: String,
    max_running: usize,
    disk_reserve_mib: i64,
    memory_floor_mib: i64,
    storage_floor_mib: i64,
    guest_user: String,
    control_token_file: PathBuf,
    capture_token_file: PathBuf,
    port: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Operation {
    Clone,
    Start,
    Stop,
    Delete,
}

impl Operation {
    fn parse(value: &str) -> Result<Self, Rejected> {
        match value {
            "clone" => Ok(Self::Clone),
            "start" => Ok(Self::Start),
            "stop" => Ok(Self::Stop),
            "delete" => Ok(Self::Delete),
            _ => Err(invalid_action()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Clone => "clone",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Delete => "delete",
        }
    }

    fn reference(self, lease: &str) -> String {
        format!("{}:{lease}", self.label())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Outcome {
    Pending,
    Done,
    Unknown,
}

#[derive(Serialize, Deserialize)]
struct Row {
    lease: String,
    operations: BTreeMap<Operation, Outcome>,
    generation: Option<String>,
    #[serde(default)]
    captured: bool,
    #[serde(default)]
    discard: bool,
    #[serde(default)]
    deleted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Guest {
    name: String,
    source: String,
    running: bool,
}

fn unverified() -> Rejected {
    Rejected::new(
        "apple_worker_unverified",
        "The worker could not verify the request.",
        503,
    )
}

fn identity_changed() -> Rejected {
    Rejected::new(
        "apple_identity_changed",
        "The Tart guest or workspace does not match the enrolled lease.",
        409,
    )
}

fn invalid_action() -> Rejected {
    Rejected::new(
        "apple_operation_identity",
        "Invalid operation or task reference.",
        400,
    )
}

fn occupied(reason: &str) -> Rejected {
    Rejected::new("apple_slot_occupied", reason, 409)
}

fn verify_headroom(capacity: &Value) -> Result<(), Rejected> {
    for field in ["memory_after_start", "storage_after_writes"] {
        let actual = capacity[field].as_i64().ok_or_else(unverified)?;
        let floor = capacity[format!("floor_{field}")]
            .as_i64()
            .ok_or_else(unverified)?;
        if actual < floor {
            return Err(Rejected::new(
                "apple_host_headroom",
                "The Apple worker cannot preserve its host headroom.",
                409,
            ));
        }
    }
    Ok(())
}

fn lease_name(lease: &str) -> Result<String, Rejected> {
    if !hex(lease, 32) {
        return Err(Rejected::new(
            "invalid_lease",
            "Invalid Apple lease identity.",
            400,
        ));
    }
    Ok(format!("lease-{lease}"))
}

fn settings_valid(settings: &Settings) -> io::Result<()> {
    for path in [&settings.state, &settings.tart_home] {
        if path.canonicalize()? != *path || !path.is_dir() {
            return Err(io::Error::other(
                "Worker directories must be exact existing absolute paths.",
            ));
        }
    }
    let template = &settings.template;
    let interface = settings.interface.strip_prefix("en").unwrap_or_default();
    if !settings.tart_binary.is_absolute()
        || template.is_empty()
        || template.len() > 63
        || template.starts_with("lease-")
        || !template
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || !hex(&settings.identity, 32)
        || !hex(&settings.template_config_sha256, 64)
        || interface.is_empty()
        || !interface.bytes().all(|byte| byte.is_ascii_digit())
        || settings.port == 0
        || settings.max_running == 0
        || settings.shape.vcpus < 1
        || settings.shape.memory_mib < 1
        || settings.shape.disk_mib < 1
        || [
            settings.disk_reserve_mib,
            settings.memory_floor_mib,
            settings.storage_floor_mib,
        ]
        .into_iter()
        .any(|value| value < 0 || value > i64::MAX / MIB)
    {
        return Err(io::Error::other("Invalid dedicated Tart worker scope."));
    }
    Ok(())
}

struct Worker {
    settings: Settings,
    control: String,
    capture: String,
    inflight: Mutex<BTreeSet<String>>,
}

impl Worker {
    fn open(config: &Path) -> io::Result<Arc<Self>> {
        let settings: Settings = serde_json::from_reader(File::open(config)?)?;
        settings_valid(&settings)?;
        let token = |path: &Path| -> io::Result<String> {
            let raw = fs::read_to_string(path)?;
            let raw = raw.trim();
            if valid_token(raw, 43, 128) {
                Ok(format!("Bearer {raw}"))
            } else {
                Err(io::Error::other(
                    "Expected a scoped Tart worker token file.",
                ))
            }
        };
        let control = token(&settings.control_token_file)?;
        let capture = token(&settings.capture_token_file)?;
        if control.as_bytes().ct_eq(capture.as_bytes()).into() {
            return Err(io::Error::other(
                "Capture and control require distinct worker roles.",
            ));
        }
        Ok(Arc::new(Self {
            settings,
            control,
            capture,
            inflight: Mutex::default(),
        }))
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.settings.tart_binary);
        command
            .env("TART_HOME", &self.settings.tart_home)
            .env("TART_NO_AUTO_PRUNE", "1");
        command
    }

    fn tart(&self, args: &[&str], timeout: u64, input: Option<&File>) -> io::Result<Vec<u8>> {
        let out = bounded(
            self.command().args(args),
            input,
            Duration::from_secs(timeout),
            1024 * 1024,
        )?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(io::Error::other("Tart command failed."))
        }
    }

    fn guests(&self) -> Result<BTreeMap<String, Guest>, Rejected> {
        let list = self
            .tart(&["list", "--format", "json"], 12, None)
            .map_err(|_| unverified())?;
        let rows: Vec<Guest> = serde_json::from_slice(&list).map_err(|_| unverified())?;
        Ok(rows
            .into_iter()
            .filter(|row| row.source == "local")
            .map(|row| (row.name.clone(), row))
            .collect())
    }

    fn vm(&self, name: &str) -> PathBuf {
        self.settings.tart_home.join("vms").join(name)
    }

    fn workspace(&self, name: &str) -> PathBuf {
        self.settings.state.join(format!("{name}-workspace"))
    }

    fn journal(&self, name: &str) -> PathBuf {
        self.settings.state.join(format!("{name}.json"))
    }

    fn load(&self, lease: &str) -> Result<Option<Row>, Rejected> {
        let name = lease_name(lease)?;
        match File::open(self.journal(&name)) {
            Ok(file) => {
                let row: Row = serde_json::from_reader(file).map_err(|_| unverified())?;
                if row.lease != lease {
                    return Err(identity_changed());
                }
                Ok(Some(row))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(unverified()),
        }
    }

    fn save(&self, row: &Row) -> io::Result<()> {
        let mut staged = tempfile::NamedTempFile::new_in(&self.settings.state)?;
        serde_json::to_writer(&mut staged, row)?;
        staged.as_file().sync_all()?;
        staged.persist(self.journal(&format!("lease-{}", row.lease)))?;
        File::open(&self.settings.state)?.sync_all()
    }

    fn records(&self) -> Result<(Vec<Row>, bool), Rejected> {
        let mut rows = Vec::new();
        let mut workspaces = false;
        for entry in fs::read_dir(&self.settings.state).map_err(|_| unverified())? {
            let entry = entry.map_err(|_| unverified())?;
            let filename = entry.file_name();
            let Some(name) = filename
                .to_str()
                .and_then(|name| name.strip_prefix("lease-"))
            else {
                continue;
            };
            if let Some(lease) = name.strip_suffix(".json") {
                rows.push(self.load(lease)?.ok_or_else(unverified)?);
            }
            workspaces |= name.ends_with("-workspace");
        }
        Ok((rows, workspaces))
    }

    fn generation(&self, lease: &str) -> Result<String, Rejected> {
        let name = lease_name(lease)?;
        let root = self.vm(&name);
        let mut values = Vec::new();
        for (path, directory) in [
            (root.clone(), true),
            (root.join("config.json"), false),
            (root.join("disk.img"), false),
            (self.workspace(&name), true),
        ] {
            let info = fs::symlink_metadata(&path).map_err(|_| identity_changed())?;
            if (directory && !info.is_dir()) || (!directory && !info.is_file()) {
                return Err(identity_changed());
            }
            values.extend([info.dev().to_string(), info.ino().to_string()]);
        }
        // Retain the original generation encoding for existing durable journals.
        Ok(hash(&format!(
            "[\"{}\", \"{lease}\", [{}]]",
            self.settings.identity,
            values.join(", ")
        )))
    }

    fn owned(&self, lease: &str, generation: Option<&str>) -> Result<Row, Rejected> {
        let row = self.load(lease)?.ok_or_else(identity_changed)?;
        let actual = self.generation(lease)?;
        if row.deleted
            || row.generation.as_deref() != Some(actual.as_str())
            || generation.is_some_and(|provided| provided != actual)
        {
            return Err(identity_changed());
        }
        Ok(row)
    }

    fn idle(&self) -> Result<(), Rejected> {
        let (rows, workspaces) = self.records()?;
        if rows.iter().any(|row| !row.deleted) {
            return Err(occupied(
                "The Apple slot still has an owned guest or unresolved operation.",
            ));
        }
        if self
            .guests()?
            .keys()
            .any(|name| name.strip_prefix("lease-").is_some_and(|id| hex(id, 32)))
        {
            return Err(occupied(
                "A guest using the lease namespace already exists.",
            ));
        }
        if workspaces {
            return Err(occupied(
                "A prior workspace remains in the enrolled Apple scope.",
            ));
        }
        Ok(())
    }

    fn capacity(&self, allow_running: bool) -> Result<Value, Rejected> {
        let settings = &self.settings;
        let guests = self.guests()?;
        let template = || {
            Rejected::new(
                "apple_recipe_changed",
                "The pinned and stopped Tart template cannot be verified.",
                409,
            )
        };
        if !guests
            .get(&settings.template)
            .is_some_and(|guest| !guest.running)
        {
            return Err(template());
        }
        let config =
            File::open(self.vm(&settings.template).join("config.json")).map_err(|_| template())?;
        if digest_to(config, io::sink(), 1024 * 1024)
            .map_err(|_| template())?
            .0
            != settings.template_config_sha256
        {
            return Err(template());
        }
        if !allow_running
            && guests.values().filter(|guest| guest.running).count() >= settings.max_running
        {
            return Err(Rejected::new(
                "apple_capacity_exhausted",
                "The Mac already has the maximum verified running guests.",
                409,
            ));
        }
        let memory = bounded(
            &mut Command::new("/usr/bin/memory_pressure").arg("-Q"),
            None,
            Duration::from_secs(4),
            4096,
        )
        .map_err(|_| unverified())?;
        if !memory.status.success() {
            return Err(unverified());
        }
        let memory = String::from_utf8_lossy(&memory.stdout);
        let number = |after: &str| -> Result<i64, Rejected> {
            memory
                .split_once(after)
                .and_then(|(_, value)| {
                    value
                        .trim_start()
                        .split(|c: char| !c.is_ascii_digit())
                        .next()
                })
                .and_then(|value| value.parse().ok())
                .ok_or_else(unverified)
        };
        let percent = number("percentage:")?;
        let total = number("has ")?;
        let available = available_bytes(&settings.tart_home).map_err(|_| unverified())?;
        let memory_after_start = total
            .checked_mul(percent)
            .and_then(|bytes| {
                (bytes / 100).checked_sub(settings.shape.memory_mib.checked_mul(MIB)?)
            })
            .ok_or_else(unverified)?;
        let storage_after_writes = available
            .checked_sub(settings.disk_reserve_mib * MIB)
            .ok_or_else(unverified)?;
        Ok(json!({
            "shape":settings.shape,
            "memory_after_start":memory_after_start,
            "storage_after_writes":storage_after_writes,
            "floor_memory_after_start":settings.memory_floor_mib * MIB,
            "floor_storage_after_writes":settings.storage_floor_mib * MIB,
        }))
    }

    fn tasks(&self, operation: Operation, lease: Option<&str>) -> Result<Value, Rejected> {
        if let Some(lease) = lease {
            let found = self
                .load(lease)?
                .is_some_and(|row| row.operations.contains_key(&operation));
            return Ok(json!(
                found
                    .then(|| operation.reference(lease))
                    .into_iter()
                    .collect::<Vec<_>>()
            ));
        }
        if operation != Operation::Clone {
            return Err(invalid_action());
        }
        let (rows, _) = self.records()?;
        Ok(json!(
            rows.into_iter()
                .filter(|row| !row.deleted && row.operations.contains_key(&operation))
                .map(|row| operation.reference(&row.lease))
                .collect::<Vec<_>>()
        ))
    }

    fn dispatch(
        self: &Arc<Self>,
        inflight: &mut BTreeSet<String>,
        lease: &str,
        operation: Operation,
        generation: Option<&str>,
        discard: bool,
    ) -> Result<Value, Rejected> {
        let name = lease_name(lease)?;
        let prior = self.load(lease)?;
        let reference = operation.reference(lease);
        if prior
            .as_ref()
            .is_some_and(|row| !row.deleted && row.operations.contains_key(&operation))
        {
            if operation != Operation::Clone {
                self.owned(lease, generation)?;
            }
            return Ok(json!(reference));
        }

        let mut row = if operation == Operation::Clone {
            self.idle()?;
            verify_headroom(&self.capacity(false)?)?;
            if prior.is_some()
                || self.guests()?.contains_key(&name)
                || self.workspace(&name).exists()
            {
                return Err(identity_changed());
            }
            Row {
                lease: lease.to_owned(),
                operations: BTreeMap::new(),
                generation: None,
                captured: false,
                discard: false,
                deleted: false,
            }
        } else {
            let row = self.owned(lease, generation)?;
            let expected = if operation == Operation::Start {
                Operation::Clone
            } else {
                Operation::Start
            };
            if row.operations.get(&expected) != Some(&Outcome::Done) {
                return Err(Rejected::new(
                    "apple_operation_state",
                    "The prior Tart operation is not verified.",
                    409,
                ));
            }
            row
        };
        row.operations.insert(operation, Outcome::Pending);
        if operation == Operation::Delete && discard {
            row.discard = true;
        }
        self.save(&row).map_err(|_| unverified())?;
        inflight.insert(reference.clone());
        let worker = self.clone();
        thread::spawn(move || worker.run_operation(&name, operation, row));
        Ok(json!(reference))
    }

    fn run_operation(&self, name: &str, operation: Operation, mut row: Row) {
        let performed = self.perform_operation(name, operation, &mut row);
        let outcome = if let Err(error) = &performed {
            eprintln!("Tart {} held: {}", operation.label(), error.code());
            Outcome::Unknown
        } else {
            Outcome::Done
        };
        row.operations.insert(operation, outcome);
        if let Ok(mut inflight) = self.inflight.lock() {
            if let Err(error) = self.save(&row) {
                eprintln!("Could not commit Tart operation journal: {}", error.kind());
            }
            inflight.remove(&operation.reference(&row.lease));
        }
    }

    fn perform_operation(
        &self,
        name: &str,
        operation: Operation,
        row: &mut Row,
    ) -> Result<(), Rejected> {
        if operation != Operation::Clone {
            self.owned(&row.lease, row.generation.as_deref())?;
        }
        match operation {
            Operation::Clone => {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(self.workspace(name))
                    .map_err(|_| unverified())?;
                self.tart(&["clone", &self.settings.template, name], 150, None)
                    .map_err(|_| unverified())?;
                row.generation = Some(self.generation(&row.lease)?);
            }
            Operation::Start => self.start(name)?,
            Operation::Stop => {
                if self.guests()?.get(name).is_some_and(|guest| guest.running) {
                    self.tart(&["stop", name], 66, None)
                        .map_err(|_| unverified())?;
                }
                if !self.guests()?.get(name).is_some_and(|guest| !guest.running) {
                    return Err(unverified());
                }
            }
            Operation::Delete => {
                if !(row.captured || row.discard)
                    || !self.guests()?.get(name).is_some_and(|guest| !guest.running)
                {
                    return Err(Rejected::new(
                        "apple_operation_state",
                        "Deletion requires the exact stopped guest and its workspace disposition.",
                        409,
                    ));
                }
                self.tart(&["delete", name], 45, None)
                    .map_err(|_| unverified())?;
                if self.vm(name).try_exists().map_err(|_| unverified())?
                    || self.guests()?.contains_key(name)
                {
                    return Err(unverified());
                }
                fs::remove_dir_all(self.workspace(name)).map_err(|_| unverified())?;
                row.deleted = true;
            }
        }
        Ok(())
    }

    fn start(&self, name: &str) -> Result<(), Rejected> {
        verify_headroom(&self.capacity(false)?)?;
        let log = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.settings.state.join(format!("{name}.log")))
            .map_err(|_| unverified())?;
        let mut command = self.command();
        command
            .args([
                "run",
                "--no-graphics",
                "--no-audio",
                "--no-usb-accessories",
                "--no-clipboard",
                &format!("--net-bridged={}", self.settings.interface),
                &format!("--dir=workspace:{}", self.workspace(name).display()),
                name,
            ])
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|_| unverified())?)
            .stderr(log);
        // setsid is async-signal-safe, and detaches the guest from the worker's
        // session while leaving guest state discoverable across worker restarts.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let mut child = command.spawn().map_err(|_| unverified())?;
        let mut verify = || -> Result<(), Rejected> {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !self.guests()?.get(name).is_some_and(|guest| guest.running) {
                if child.try_wait().map_err(|_| unverified())?.is_some()
                    || Instant::now() >= deadline
                {
                    return Err(unverified());
                }
                thread::sleep(Duration::from_millis(250));
            }
            Ok(())
        };
        let result = verify();
        thread::spawn(move || {
            if let Err(error) = child.wait() {
                eprintln!("Could not reap Tart process: {}", error.kind());
            }
        });
        result
    }

    fn task(&self, inflight: &BTreeSet<String>, reference: &str) -> Result<Value, Rejected> {
        let (operation, lease) = reference.split_once(':').ok_or_else(invalid_action)?;
        let operation = Operation::parse(operation)?;
        let state = self
            .load(lease)?
            .and_then(|row| row.operations.get(&operation).copied());
        match state {
            Some(Outcome::Done) => Ok(json!(true)),
            Some(Outcome::Pending) if inflight.contains(reference) => Ok(json!(false)),
            _ => Err(Rejected::new(
                "apple_operation_unknown",
                "Tart operation is unresolved. The slot stays held.",
                409,
            )),
        }
    }

    fn request(
        self: &Arc<Self>,
        inflight: &mut BTreeSet<String>,
        data: &Value,
        action: &str,
    ) -> Result<Value, Rejected> {
        let lease = data["lease"].as_str();
        let generation = data["generation"].as_str();
        let text = |field: &str| data[field].as_str().ok_or_else(invalid_action);
        match action {
            "idle" => return self.idle().map(|()| json!(true)),
            "capacity" => return self.capacity(data["allow_running"] == true),
            "tasks" => return self.tasks(Operation::parse(text("operation")?)?, lease),
            "dispatch" => {
                return self.dispatch(
                    inflight,
                    text("lease")?,
                    Operation::parse(text("operation")?)?,
                    generation,
                    data["discard"] == true,
                );
            }
            "task" => return self.task(inflight, text("ref")?),
            _ => (),
        }
        let lease = text("lease")?;
        let name = lease_name(lease)?;
        let row = self.owned(lease, generation)?;
        match action {
            "describe" => Ok(json!({"generation":row.generation})),
            "status" => {
                let guests = self.guests()?;
                let running = guests.get(&name).ok_or_else(identity_changed)?.running;
                Ok(json!(if running { "running" } else { "stopped" }))
            }
            "address" => {
                let address = self
                    .tart(&["ip", &name, "--resolver", "agent"], 4, None)
                    .ok()
                    .and_then(|out| {
                        String::from_utf8_lossy(&out)
                            .trim()
                            .parse::<Ipv4Addr>()
                            .ok()
                    });
                if address.is_some_and(|address| !self.settings.guest_network.contains(&address)) {
                    return Err(Rejected::new(
                        "apple_guest_address_unverified",
                        "Tart returned an address outside the enrolled DHCP scope.",
                        409,
                    ));
                }
                Ok(json!(address))
            }
            "host-key" => {
                self.guest_user(&name)?;
                let rejected = || {
                    Rejected::new(
                        "apple_guest_key_unverified",
                        "The clone could not verify its independent SSH host identity.",
                        409,
                    )
                };
                let key = self
                    .tart(&["exec", &name, "/bin/sh", "-ec", PREPARE_SSH], 7, None)
                    .map_err(|_| rejected())?;
                let key = String::from_utf8_lossy(&key)
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                public_key(&json!(key))
                    .map(|key| json!(key))
                    .map_err(|_| rejected())
            }
            "guest-key" => {
                self.guest_user(&name)?;
                let key = public_key(&data["key"])?;
                let mut input =
                    tempfile::tempfile_in(&self.settings.state).map_err(|_| unverified())?;
                writeln!(input, "{key}")
                    .and_then(|()| input.rewind())
                    .map_err(|_| unverified())?;
                self.tart(&[
                    "exec", "-i", &name, "/bin/sh", "-ec",
                    "umask 077; mkdir -p ~/.ssh; cat > ~/.ssh/authorized_keys; test -d \"$1\"; mkdir -p \"$1/project\"",
                    "workspace", SHARE,
                ], 8, Some(&input)).map_err(|_| unverified())?;
                Ok(json!(true))
            }
            _ => Err(invalid_action()),
        }
    }

    fn guest_user(&self, name: &str) -> Result<(), Rejected> {
        let actual = self
            .tart(&["exec", name, "/usr/bin/id", "-un"], 4, None)
            .map_err(|_| unverified())?;
        if String::from_utf8_lossy(&actual).trim() != self.settings.guest_user {
            return Err(Rejected::new(
                "apple_guest_user_changed",
                "The guest agent user differs from the qualified recipe.",
                409,
            ));
        }
        Ok(())
    }

    fn capture(&self, data: &Value) -> Result<(File, u64), Rejected> {
        let lease = data["lease"].as_str().ok_or_else(invalid_action)?;
        let generation = data["generation"].as_str().ok_or_else(identity_changed)?;
        let name = lease_name(lease)?;
        let mut row = self.owned(lease, Some(generation))?;
        if row.operations.get(&Operation::Start) != Some(&Outcome::Done)
            || !self
                .guests()?
                .get(&name)
                .is_some_and(|guest| !guest.running)
            || row.operations.get(&Operation::Delete) == Some(&Outcome::Pending)
        {
            return Err(Rejected::new(
                "apple_guest_running",
                "Stop the exact owned clone before archive capture.",
                409,
            ));
        }
        let mut output = tempfile::tempfile_in(&self.settings.state).map_err(|_| unverified())?;
        archive_workspace(&self.workspace(&name).join("project"), &mut output)
            .map_err(|_| unverified())?;
        let size = output.stream_position().map_err(|_| unverified())?;
        if size == 0 || size > MAX_ARCHIVE {
            return Err(unverified());
        }
        output.rewind().map_err(|_| unverified())?;
        if self.generation(lease)? != generation {
            return Err(identity_changed());
        }
        row.captured = true;
        self.save(&row).map_err(|_| unverified())?;
        Ok((output, size))
    }
}

fn available_bytes(path: &Path) -> io::Result<i64> {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    // statvfs initializes the whole structure on success.
    let stat = unsafe {
        if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        stat.assume_init()
    };
    i64::try_from(u128::from(stat.f_bavail) * u128::from(stat.f_frsize))
        .map_err(|_| io::Error::other("Tart storage size exceeds the supported range."))
}

fn problem(error: Rejected) -> Response {
    (
        StatusCode::from_u16(error.status()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
        axum::Json(json!({"code":error.code(),"reason":error.reason()})),
    )
        .into_response()
}

enum WorkerResult {
    Report(Value),
    Capture(File, u64),
}

async fn handle(State(worker): State<Arc<Worker>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let headers = &parts.headers;
    if parts
        .uri
        .path_and_query()
        .is_none_or(|value| value.as_str() != "/")
        || headers
            .get(header::CONTENT_TYPE)
            .is_none_or(|value| value != "application/json")
    {
        return problem(Rejected::new("not_found", "Worker route unavailable.", 404));
    }
    let length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=4096).contains(value));
    let body = match tokio::time::timeout(Duration::from_secs(12), to_bytes(body, 4096)).await {
        Ok(Ok(body)) if length == Some(body.len()) => body,
        _ => return problem(unverified()),
    };
    let Ok(data) = serde_json::from_slice::<Value>(&body) else {
        return problem(unverified());
    };
    let Some(action) = data["action"].as_str().map(str::to_owned) else {
        return problem(unverified());
    };
    let capture = action == "capture";
    let expected = if capture {
        &worker.capture
    } else {
        &worker.control
    };
    let supplied = headers
        .get(header::AUTHORIZATION)
        .map_or(&[] as &[u8], |value| value.as_bytes());
    if !bool::from(expected.as_bytes().ct_eq(supplied)) {
        return problem(Rejected::new("unauthorized", "Worker role refused.", 401));
    }
    let result = tokio::task::spawn_blocking(move || {
        let mut inflight = worker.inflight.lock().map_err(|_| unverified())?;
        if capture {
            worker
                .capture(&data)
                .map(|(file, size)| WorkerResult::Capture(file, size))
        } else {
            let value = worker.request(&mut inflight, &data, &action)?;
            Ok(WorkerResult::Report(
                json!({"worker":worker.settings.identity,"value":value}),
            ))
        }
    })
    .await;
    match result {
        Ok(Ok(WorkerResult::Report(value))) => axum::Json(value).into_response(),
        Ok(Ok(WorkerResult::Capture(file, size))) => (
            [
                (header::CONTENT_TYPE, "application/gzip".to_owned()),
                (header::CONTENT_LENGTH, size.to_string()),
            ],
            Body::from_stream(ReaderStream::new(tokio::fs::File::from_std(file))),
        )
            .into_response(),
        Ok(Err(error)) => problem(error),
        Err(_) => problem(unverified()),
    }
}

pub async fn run(config: &Path, preflight: bool) -> Result<(), Box<dyn std::error::Error>> {
    let worker = Worker::open(config)?;
    if preflight {
        worker.idle()?;
        let capacity = worker.capacity(false)?;
        println!(
            "{}",
            json!({"template_verified":true,"slot_idle":true,"shape":capacity["shape"],"capacity":capacity})
        );
        return Ok(());
    }
    let listener =
        tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, worker.settings.port)).await?;
    axum::serve(
        listener,
        Router::new().route("/", post(handle)).with_state(worker),
    )
    .await?;
    Ok(())
}

pub fn fetch_capture(
    config: &Value,
    lease: &str,
    generation: &str,
    output: impl Write,
) -> io::Result<()> {
    use reqwest::{blocking::Client, redirect, retry};
    let refused = || io::Error::other("Invalid exact workspace identity or worker route.");
    let endpoint = config["url"]
        .as_str()
        .and_then(|value| crate::tart::tunnel(value).ok())
        .ok_or_else(refused)?;
    let secret = fs::read_to_string(config["token_file"].as_str().ok_or_else(refused)?)?;
    let secret = secret.trim();
    if !hex(lease, 32) || !hex(generation, 64) || !valid_token(secret, 43, 128) {
        return Err(refused());
    }
    let failed = || io::Error::other("The stopped Tart workspace could not be captured.");
    let client = Client::builder()
        .no_proxy()
        .redirect(redirect::Policy::none())
        .retry(retry::never())
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|_| failed())?;
    let response = client
        .post(endpoint)
        .bearer_auth(secret)
        .header(header::CONTENT_TYPE, "application/json")
        .body(json!({"action":"capture","lease":lease,"generation":generation}).to_string())
        .send()
        .map_err(|_| failed())?
        .error_for_status()
        .map_err(|_| failed())?;
    let length = response
        .content_length()
        .filter(|bytes| (1..=MAX_ARCHIVE).contains(bytes))
        .ok_or_else(failed)?;
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_none_or(|value| value != "application/gzip")
    {
        return Err(failed());
    }
    let (_, size) = digest_to(response, output, length)?;
    if size != length {
        return Err(io::Error::other("Tart workspace stream ended early."));
    }
    Ok(())
}
