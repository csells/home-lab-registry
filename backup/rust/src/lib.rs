use std::{
    collections::BTreeSet,
    error::Error,
    fs::{self, File},
    io::{self, Read},
    net::{IpAddr, Ipv4Addr},
    path::{Component, Path},
    process::{Command, Output, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use flate2::read::MultiGzDecoder;
use ipnet::IpNet;
use lab_registry::client::{RegistryClient, SshTarget};
use serde_json::{Value, json};
use wait_timeout::ChildExt;

fn invalid(message: &'static str) -> io::Error {
    io::Error::other(message)
}

fn field<'a>(value: &'a Value, name: &str) -> io::Result<&'a str> {
    value[name]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("A required backup setting is missing."))
}

fn stamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn kind<'a>(spec: &'a Value, field: &str) -> Option<&'a str> {
    spec.get(field)
        .map_or(Some("qemu"), Value::as_str)
        .filter(|kind| matches!(*kind, "qemu" | "lxc"))
}

pub fn verified_address(
    binding: &Value,
    observation: &Value,
    now: f64,
) -> Result<Ipv4Addr, Box<dyn Error>> {
    let vmid = binding["vmid"]
        .as_u64()
        .filter(|num| *num > 0)
        .ok_or_else(|| invalid("Invalid guest identity."))?;
    let guest = kind(binding, "guest_kind")
        .filter(|kind| Some(*kind) == self::kind(observation, "kind"))
        .ok_or_else(|| invalid("Guest type changed."))?;
    let identity = if guest == "qemu" { "uuid" } else { "volume_id" };
    if observation["vmid"].as_u64() != Some(vmid)
        || observation[identity] != field(binding, identity)?
    {
        return Err(invalid("Guest identity changed.").into());
    }
    if observation["state"] != "running" {
        return Err(invalid("Guest is not running.").into());
    }
    if observation["observed_at"]
        .as_f64()
        .is_none_or(|observed| !(-2.0..=60.0).contains(&(now - observed)))
    {
        return Err(invalid("Stale provider observation.").into());
    }
    let mac = field(binding, "mac")?;
    let networks = binding["allowed_networks"]
        .as_array()
        .ok_or_else(|| invalid("Missing allowed source networks."))?
        .iter()
        .map(|entry| {
            let input = entry
                .as_str()
                .ok_or_else(|| invalid("Invalid allowed source network."))?;
            let network = input
                .parse::<IpNet>()
                .map_err(|_| invalid("Invalid allowed source network."))?;
            if network != network.trunc() {
                return Err(invalid("Use canonical allowed source networks."));
            }
            Ok(network)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let interfaces = observation["interfaces"]
        .as_array()
        .ok_or_else(|| invalid("Missing provider interfaces."))?;
    let mut candidates = BTreeSet::new();
    for nic in interfaces {
        if !nic["hardware-address"]
            .as_str()
            .is_some_and(|observed| observed.eq_ignore_ascii_case(mac))
        {
            continue;
        }
        let Some(addresses) = nic["ip-addresses"].as_array() else {
            continue;
        };
        for entry in addresses {
            let ip = field(entry, "ip-address")?.parse::<IpAddr>()?;
            if let IpAddr::V4(address) = ip
                && networks.iter().any(|network| network.contains(&ip))
            {
                candidates.insert(address);
            }
        }
    }
    if candidates.len() != 1 {
        return Err(invalid("Guest address unavailable or ambiguous.").into());
    }
    candidates
        .first()
        .copied()
        .ok_or_else(|| invalid("Guest address unavailable.").into())
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

pub fn guest_request<'a, 'b>(
    request: &'a str,
    guests: &'b Value,
) -> io::Result<(&'a str, &'b str)> {
    let fields = request.split_whitespace().collect::<Vec<_>>();
    let ["discover", id] = fields.as_slice() else {
        return Err(invalid("Expected scoped discovery request."));
    };
    if id.starts_with('0')
        || !id.bytes().all(|byte| byte.is_ascii_digit())
        || id.parse::<u64>().is_err()
    {
        return Err(invalid("Expected scoped discovery request."));
    }
    let kind = guests
        .get(*id)
        .and_then(Value::as_str)
        .filter(|kind| matches!(*kind, "qemu" | "lxc"))
        .ok_or_else(|| invalid("Guest is not authorized."))?;
    Ok((id, kind))
}

fn bounded(command: &mut Command, seconds: u64, limit: u64) -> io::Result<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let out = child
        .stdout
        .take()
        .ok_or_else(|| invalid("Source command did not provide output."))?;
    let read = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        out.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(invalid("Source command output exceeded its limit."));
        }
        Ok(bytes)
    });
    let Some(status) = child.wait_timeout(Duration::from_secs(seconds))? else {
        child.kill()?;
        child.wait()?;
        return Err(invalid("Source command timed out."));
    };
    let stdout = read
        .join()
        .map_err(|_| invalid("Source command read failed."))??;
    Ok(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

fn query(command: &mut Command, seconds: u64) -> Result<Value, Box<dyn Error>> {
    let output = bounded(command, seconds, 4 * 1024 * 1024)?;
    if !output.status.success() {
        return Err(invalid("Scoped source observation failed.").into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

pub fn observe(node: &str, guest: &str, kind: &str) -> Result<Value, Box<dyn Error>> {
    if !identifier(node)
        || guest.parse::<u64>().ok().is_none_or(|id| id == 0)
        || !matches!(kind, "qemu" | "lxc")
    {
        return Err(invalid("Invalid configured provider scope.").into());
    }
    let prefix = format!("/nodes/{node}/{kind}/{guest}");
    let get = |suffix: &str| {
        query(
            Command::new("pvesh").args([
                "get",
                &format!("{prefix}/{suffix}"),
                "--output-format",
                "json",
            ]),
            20,
        )
    };
    let config = get("config")?;
    let state = get("status/current")?;
    let running = field(&state, "status")? == "running";
    let mut result =
        json!({"vmid":guest.parse::<u64>()?,"kind":kind,"state":state["status"],"interfaces":[]});
    if kind == "qemu" {
        result["uuid"] = json!(
            field(&config, "smbios1")?
                .split(',')
                .find_map(|part| part.strip_prefix("uuid="))
                .ok_or_else(|| invalid("Missing QEMU identity."))?
        );
        if running {
            result["interfaces"] = query(
                Command::new("qm").args(["guest", "cmd", guest, "network-get-interfaces"]),
                20,
            )?;
        }
    } else {
        result["volume_id"] = json!(
            field(&config, "rootfs")?
                .split(',')
                .next()
                .unwrap_or_default()
        );
        if running {
            let interfaces = query(
                Command::new("pct").args(["exec", guest, "--", "ip", "-j", "addr", "show"]),
                20,
            )?;
            let interfaces = interfaces
                .as_array()
                .ok_or_else(|| invalid("Invalid provider interfaces."))?;
            result["interfaces"] = json!(interfaces.iter().map(|nic| {
                let addresses = nic["addr_info"].as_array().into_iter().flatten()
                    .map(|addr| json!({"ip-address":addr["local"]})).collect::<Vec<_>>();
                json!({"name":nic["ifname"],"hardware-address":nic["address"].as_str().unwrap_or_default(),"ip-addresses":addresses})
            }).collect::<Vec<_>>());
        }
    }
    result["observed_at"] = json!(stamp());
    Ok(result)
}

fn ssh_args(base: &Path, key: &str) -> io::Result<Vec<String>> {
    if !identifier(key) {
        return Err(invalid("Invalid credential reference."));
    }
    Ok(vec![
        "-i".into(),
        base.join("private")
            .join(key)
            .to_string_lossy()
            .into_owned(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-o".into(),
        "IdentityAgent=none".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=yes".into(),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "GlobalKnownHostsFile=/dev/null".into(),
        "-o".into(),
        format!(
            "UserKnownHostsFile={}",
            base.join("private/known_hosts").display()
        ),
    ])
}

fn ssh_target_args(target: &SshTarget) -> [String; 4] {
    [
        "-p".into(),
        target.port.to_string(),
        "-o".into(),
        format!("HostKeyAlias={}", target.host_key_alias),
    ]
}

fn valid_user(name: &str) -> bool {
    identifier(name)
        && name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
}

fn ssh_port(spec: &Value) -> io::Result<Option<u16>> {
    spec.get("port")
        .map(|value| {
            value
                .as_u64()
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port > 0)
                .ok_or_else(|| invalid("Invalid SSH port."))
        })
        .transpose()
}

fn discover(
    base: &Path,
    client: &RegistryClient,
    spec: &Value,
) -> Result<(SshTarget, Option<String>), Box<dyn Error>> {
    let binding = &spec["provider"];
    if binding["kind"] != "proxmox-ssh" {
        return Err(invalid("Unsupported discovery provider.").into());
    }
    let vmid = binding["vmid"]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or_else(|| invalid("Invalid guest identity."))?;
    let host = field(binding, "host_id")?;
    let observation = client.ssh_target(host, field(binding, "username")?, None)?;
    let mut cmd = Command::new("ssh");
    cmd.args(ssh_args(base, field(binding, "key")?)?)
        .args(ssh_target_args(&observation))
        .arg(&observation.destination)
        .arg(format!("discover {vmid}"));
    let address = verified_address(binding, &query(&mut cmd, 30)?, stamp())?;
    let user = field(spec, "username")?;
    let alias = field(spec, "host_key_alias")?;
    if !valid_user(user) || !identifier(alias) {
        return Err(invalid("Invalid SSH user or host identity.").into());
    }
    let proxy = if let Some(user) = binding.get("hop_username") {
        let hop = client.ssh_target(
            host,
            user.as_str()
                .ok_or_else(|| invalid("Invalid SSH hop user."))?,
            None,
        )?;
        let mut args = vec!["ssh".to_owned()];
        args.extend(ssh_args(base, field(binding, "hop_key")?)?);
        args.extend(ssh_target_args(&hop));
        args.extend(["-W".into(), "%h:%p".into(), hop.destination]);
        Some(
            args.iter()
                .map(|arg| format!("'{}'", arg.replace('\'', "'\"'\"'")))
                .collect::<Vec<_>>()
                .join(" "),
        )
    } else {
        None
    };
    Ok((
        SshTarget {
            destination: format!("{user}@{address}"),
            port: ssh_port(spec)?.unwrap_or(22),
            host_key_alias: alias.to_owned(),
        },
        proxy,
    ))
}

fn validate_tar(input: impl Read) -> io::Result<()> {
    let mut archive = tar::Archive::new(input);
    for entry in archive.entries()? {
        let _member = entry?;
    }
    Ok(())
}

pub fn verify_archive(path: &Path) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("Invalid archive filename."))?;
    if path.metadata()?.len() < 100 {
        return Err(invalid("Export unexpectedly small."));
    }
    if name.ends_with(".tar.gz") {
        lab_workspace::decode_gzip(File::open(path)?, &mut io::sink(), u64::MAX)?;
        validate_tar(MultiGzDecoder::new(File::open(path)?))?;
    } else if name.ends_with(".tar") {
        validate_tar(File::open(path)?)?;
    }
    Ok(())
}

fn leaf(name: &str) -> bool {
    let mut path = Path::new(name).components();
    matches!(path.next(),Some(Component::Normal(part)) if part == name) && path.next().is_none()
}

fn enough_space(path: &Path) -> io::Result<bool> {
    use std::{ffi::CString, mem::MaybeUninit, os::unix::ffi::OsStrExt};
    let path = CString::new(path.as_os_str().as_bytes())?;
    let mut stats = MaybeUninit::<libc::statvfs>::uninit();
    let values = unsafe {
        if libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) != 0 {
            return Err(io::Error::last_os_error());
        }
        stats.assume_init()
    };
    Ok(u128::from(values.f_bavail) * u128::from(values.f_frsize) >= (2_u128 << 30))
}

pub fn collect(base: &Path, name: &str, clean: bool) -> Result<(), Box<dyn Error>> {
    let manifest: Value =
        serde_json::from_reader(File::open(base.join("deployment/manifest.json"))?)?;
    let spec = manifest
        .get(name)
        .filter(|_| leaf(name))
        .ok_or_else(|| invalid("Unknown backup plan."))?;
    let parent = base.join("staging");
    let target = parent.join(name);
    if clean {
        match fs::remove_dir_all(&target) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
    if target.symlink_metadata().is_ok() {
        return Err(invalid(
            "Previous staging exists. Inspect the failed operation before retrying.",
        )
        .into());
    }
    if !enough_space(base)? {
        return Err(invalid("Insufficient temporary export space.").into());
    }
    let config = base.join("private/registry-reader.json");
    let registry = RegistryClient::from_value(serde_json::from_reader(File::open(config)?)?)?;
    let (source, proxy) = if spec.get("provider").is_some() {
        discover(base, &registry, spec)?
    } else {
        (
            registry.ssh_target(
                field(spec, "machine_id")?,
                field(spec, "username")?,
                ssh_port(spec)?,
            )?,
            None,
        )
    };
    let filename = spec
        .get("filename")
        .map_or(Some("config.tar.gz"), Value::as_str)
        .filter(|filename| leaf(filename))
        .ok_or_else(|| invalid("Invalid archive name."))?;
    fs::create_dir_all(&parent)?;
    if parent.symlink_metadata()?.is_symlink() {
        return Err(invalid("Backup staging root cannot be redirected.").into());
    }
    let incoming = tempfile::Builder::new()
        .prefix(&format!("{name}-incoming-"))
        .tempdir_in(&parent)?;
    let archive = incoming.path().join(filename);
    let file = File::create_new(&archive)?;
    let mut command = Command::new("ssh");
    let key = spec
        .get("export_key")
        .map_or(Some("collector-key"), Value::as_str)
        .ok_or_else(|| invalid("Invalid credential reference."))?;
    command
        .args(ssh_args(base, key)?)
        .args(ssh_target_args(&source));
    if let Some(proxy) = proxy {
        command.args(["-o", &format!("ProxyCommand={proxy}")]);
    }
    let mut child = command
        .arg(source.destination)
        .arg("export")
        .stdin(Stdio::null())
        .stdout(file)
        .spawn()?;
    let Some(status) = child.wait_timeout(Duration::from_secs(1800))? else {
        child.kill()?;
        child.wait()?;
        return Err(invalid("Scoped export timed out.").into());
    };
    if !status.success() {
        return Err(invalid("Scoped SSH export failed.").into());
    }
    verify_archive(&archive)?;
    let size = archive.metadata()?.len();
    fs::create_dir(&target)?;
    fs::rename(&archive, target.join(filename))?;
    println!("{name}: fresh export collected; {size} bytes");
    Ok(())
}
