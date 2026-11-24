mod appliance;
pub mod collector;

use std::{
    collections::{BTreeSet, HashMap},
    error::Error,
    fs::File,
    io::{self, Read, Seek, Write},
    net::IpAddr,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ipnet::IpNet;
use lab_registry::client::{Method, RegistryClient};
use reqwest::{blocking::Client, redirect::Policy};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use wait_timeout::ChildExt;

pub(crate) fn error(reason: &'static str) -> io::Error {
    io::Error::other(reason)
}

pub(crate) fn timestamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn exact_network(spec: &str) -> io::Result<IpNet> {
    let network: IpNet = spec.parse().map_err(|_| error("Invalid LAN network."))?;
    if network != network.trunc() {
        return Err(error("Network must be a canonical CIDR."));
    }
    Ok(network)
}

pub fn compare(
    mut rows: Vec<Value>,
    reservations: &Value,
    networks: &[&str],
) -> io::Result<Vec<Value>> {
    let networks = networks
        .iter()
        .map(|network| exact_network(network))
        .collect::<Result<Vec<_>, _>>()?;
    let reservations = reservations
        .as_array()
        .ok_or_else(|| error("Invalid reservation evidence."))?;
    let mut authority = BTreeSet::new();
    for reservation in reservations {
        let ip = string(reservation, "ip")?;
        let mac = string(reservation, "mac")?;
        authority.insert((ip, mac.to_ascii_lowercase()));
    }
    let mut result = Vec::new();
    for mut row in rows.drain(..) {
        if !row.is_object() {
            return Err(error("Invalid collection evidence."));
        }
        let offline = row["scope"] == "offline";
        let address = row["address"]
            .as_str()
            .filter(|address| !address.is_empty());
        let mac = row["mac"].as_str().unwrap_or_default().to_ascii_lowercase();
        let mode = row["assignment"].as_str().unwrap_or_default();
        let (status, reason) = match address {
            None if offline
                && row["profile_verified"] == true
                && !mac.is_empty()
                && mode == "dhcp" =>
            {
                (
                    "offline-config-verified",
                    Some("Saved DHCP configuration read without booting"),
                )
            }
            None => ("unverified", None),
            Some(address) => match address.parse::<IpAddr>() {
                Err(_) => ("unverified", Some("Invalid address evidence")),
                Ok(ip) if !networks.iter().any(|net| net.contains(&ip)) && offline => (
                    "not-applicable",
                    Some("Saved address is outside the home LAN; eero reservation not applicable"),
                ),
                Ok(ip) if !networks.iter().any(|net| net.contains(&ip)) => continue,
                Ok(_) => {
                    let matches = authority.contains(&(address, mac.clone()));
                    let verified = if offline {
                        "offline-config-verified"
                    } else {
                        "pass"
                    };
                    match mode {
                        "static" if !matches && mac.is_empty() => (
                            "unverified",
                            Some("Static address found; interface MAC unresolved"),
                        ),
                        "static" if !matches => (
                            "violation",
                            Some("Static address has no matching reservation for this MAC"),
                        ),
                        "static" | "dhcp" if matches => {
                            (verified, Some("Reservation matches this interface MAC"))
                        }
                        "dhcp" => (verified, Some("DHCP; reservation optional")),
                        _ => ("unverified", Some("Assignment method not verified")),
                    }
                }
            },
        };
        row["status"] = json!(status);
        if let Some(reason) = reason {
            row["reason"] = json!(reason);
        }
        result.push(row);
    }
    Ok(result)
}

pub fn guest_identity(spec: &Value) -> (Option<String>, Option<String>) {
    let r = &spec["resources"];
    let binding = &r["runtime_binding"];
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_u64().map(|id| id.to_string()))
    };
    if binding["kind"] == "guest" {
        return (text(&binding["machine_id"]), text(&binding["guest_id"]));
    }
    let audit = &r["network_authority_audit"];
    let guest = [
        &r["guest_id"],
        &r["vmid"],
        &audit["guest_id"],
        &r["vm_name"],
        &r["deployment"]["vmid"],
    ]
    .into_iter()
    .find_map(text);
    let parent = text(&r["parent_machine"]).or_else(|| {
        guest.as_ref().map(|_| {
            let host = audit["host"]
                .as_str()
                .or_else(|| spec["host"].as_str())
                .unwrap_or_default();
            format!("machine-{host}")
        })
    });
    (parent, guest)
}

pub(crate) fn string<'a>(value: &'a Value, key: &str) -> io::Result<&'a str> {
    value[key]
        .as_str()
        .filter(|part| !part.is_empty())
        .ok_or_else(|| error("Incomplete network audit configuration."))
}

pub(crate) fn child(
    command: &mut Command,
    data: Option<&[u8]>,
    seconds: u64,
    limit: u64,
) -> io::Result<Output> {
    let stdin = match data {
        Some(bytes) => {
            let mut temp = tempfile::tempfile()?;
            temp.write_all(bytes)?;
            temp.rewind()?;
            Stdio::from(temp)
        }
        None => Stdio::null(),
    };
    let mut job = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = job
        .stdout
        .take()
        .ok_or_else(|| error("Read command did not provide output."))?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(error("Read command exceeded its output limit."));
        }
        Ok(bytes)
    });
    let Some(status) = job.wait_timeout(Duration::from_secs(seconds))? else {
        job.kill()?;
        job.wait()?;
        return Err(error("Read command timed out."));
    };
    let stdout = reader
        .join()
        .map_err(|_| error("Read command could not be collected."))??;
    Ok(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

const TRANSIENT_COLLECTOR: &str = r#"
umask 077
work=$(mktemp -d /tmp/lab-network.XXXXXXXX)
trap 'rm -f -- "$work/collect"; rmdir -- "$work"' EXIT
trap 'exit 1' HUP INT TERM
blocks=$(( $1 / 65536 ))
tail_bytes=$(( $1 % 65536 ))
dd bs=65536 count="$blocks" iflag=fullblock status=none > "$work/collect"
if test "$tail_bytes" -ne 0; then
    dd bs="$tail_bytes" count=1 iflag=fullblock status=none >> "$work/collect"
fi
test "$(wc -c < "$work/collect")" -eq "$1"
shift
chmod 700 "$work/collect"
"$work/collect" "$@"
"#;

fn read_collector(path: &Path) -> io::Result<Vec<u8>> {
    const LIMIT: u64 = 64 * 1024 * 1024;
    let mut bytes = Vec::new();
    File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > LIMIT {
        return Err(error("Invalid size for native collector artifact."));
    }
    Ok(bytes)
}

fn source_path(base: &Path, file: &Path) -> io::Result<PathBuf> {
    if file.is_absolute()
        || !file
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(error("Use a relative private file reference."));
    }
    Ok(base.join(file))
}

#[derive(Clone, Deserialize)]
struct Target {
    id: String,
    destination: String,
    key_file: PathBuf,
    known_hosts_file: PathBuf,
    host_key_alias: Option<String>,
    #[serde(default)]
    sudo: bool,
    jump: Option<Box<Target>>,
    collector_bin: Option<PathBuf>,
    collector_artifact: Option<PathBuf>,
    #[serde(skip)]
    tart_names: Vec<String>,
}

fn shell_argument(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

impl Target {
    fn args(&self, private: &Path) -> io::Result<Vec<String>> {
        let (user, host) = self
            .destination
            .split_once('@')
            .ok_or_else(|| error("Require a scoped SSH destination."))?;
        let safe = |text: &str| {
            !text.is_empty()
                && text
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        };
        if !safe(user) || user.starts_with('-') || (!safe(host) && host.parse::<IpAddr>().is_err())
        {
            return Err(error("Invalid pinned audit destination."));
        }
        let key = source_path(private, &self.key_file)?;
        let known = source_path(private, &self.known_hosts_file)?;
        let mut args = vec![
            "-T".into(),
            "-i".into(),
            key.to_string_lossy().into_owned(),
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
            "-o".into(),
            "IdentityAgent=none".into(),
            "-o".into(),
            "ConnectTimeout=8".into(),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
            "-o".into(),
            "GlobalKnownHostsFile=/dev/null".into(),
            "-o".into(),
            format!("UserKnownHostsFile={}", known.display()),
        ];
        if let Some(alias) = &self.host_key_alias {
            if !safe(alias) {
                return Err(error("Invalid pinned audit SSH identity."));
            }
            args.extend(["-o".into(), format!("HostKeyAlias={alias}")]);
        }
        if let Some(jump) = &self.jump {
            let mut proxy = vec!["ssh".to_owned()];
            proxy.extend(jump.args(private)?);
            proxy.extend(["-W".into(), "%h:%p".into(), jump.destination.clone()]);
            let line = proxy
                .iter()
                .map(|part| shell_argument(part))
                .collect::<Vec<_>>()
                .join(" ");
            args.extend(["-o".into(), format!("ProxyCommand={line}")]);
        }
        Ok(args)
    }

    fn call(
        &self,
        private: &Path,
        args: &[&str],
        data: Option<&[u8]>,
        seconds: u64,
    ) -> io::Result<Vec<u8>> {
        let binary = self
            .collector_bin
            .as_deref()
            .unwrap_or(Path::new("/usr/local/bin/lab-network-collect"));
        if !binary.is_absolute() {
            return Err(error("Use the exact deployed collector binary."));
        }
        let binary = binary
            .to_str()
            .ok_or_else(|| error("Invalid deployed collector path."))?;
        let mut command = if self.sudo {
            "sudo -n ".to_owned()
        } else {
            String::new()
        };
        command.push_str(&shell_argument(binary));
        for arg in args {
            command.push(' ');
            command.push_str(&shell_argument(arg));
        }
        let result = self.remote(private, &command, data, seconds)?;
        if result.status.success() {
            return Ok(result.stdout);
        }
        if result.status.code() == Some(255) {
            return Err(error("Pinned audit SSH command failed."));
        }
        // Retain errors from a deployed collector. Transient delivery is for
        // actually absent executables and does not broaden appliance authority.
        let probe = self.remote(
            private,
            &format!(
                "if test -e {}; then exit 1; fi; uname -sm",
                shell_argument(binary)
            ),
            None,
            12,
        )?;
        if !probe.status.success() {
            return Err(error("Pinned audit SSH command failed."));
        }
        let platform = match String::from_utf8_lossy(&probe.stdout).trim() {
            "Linux x86_64" => "linux-x86_64",
            "Linux aarch64" => "linux-aarch64",
            "Darwin arm64" => "macos-arm64",
            "Darwin x86_64" => "macos-x86_64",
            _ => return Err(error("Unknown native collector platform.")),
        };
        let artifact = match &self.collector_artifact {
            Some(file) => source_path(private, file)?,
            None => std::env::current_exe()?
                .parent()
                .ok_or_else(|| error("Native collector directory is unavailable."))?
                .join("network-collectors")
                .join(platform)
                .join("lab-network-collect"),
        };
        let mut payload = read_collector(&artifact)?;
        let bytes = payload.len();
        payload.extend_from_slice(data.unwrap_or_default());
        let mut command = if self.sudo {
            "sudo -n ".to_owned()
        } else {
            String::new()
        };
        command.push_str("/bin/sh -ec ");
        command.push_str(&shell_argument(TRANSIENT_COLLECTOR));
        command.push_str(&format!(" audit {bytes}"));
        for arg in args {
            command.push(' ');
            command.push_str(&shell_argument(arg));
        }
        let result = self.remote(private, &command, Some(&payload), seconds)?;
        if !result.status.success() {
            return Err(error("Transient native audit collector failed."));
        }
        Ok(result.stdout)
    }

    fn remote(
        &self,
        private: &Path,
        command: &str,
        data: Option<&[u8]>,
        seconds: u64,
    ) -> io::Result<Output> {
        child(
            Command::new("ssh")
                .args(self.args(private)?)
                .arg(&self.destination)
                .arg(command),
            data,
            seconds,
            4 * 1024 * 1024,
        )
    }

    fn collect(&self, private: &Path) -> Vec<Value> {
        let args = self
            .tart_names
            .iter()
            .flat_map(|name| ["--tart", name])
            .collect::<Vec<_>>();
        match self
            .call(private, &args, None, 240)
            .and_then(|body| serde_json::from_slice::<Vec<Value>>(&body).map_err(io::Error::other))
        {
            Ok(mut rows) => {
                for row in &mut rows {
                    if row.is_object() {
                        row["machine"] = json!(format!(
                            "{}{}",
                            self.id,
                            row["suffix"].as_str().unwrap_or_default()
                        ));
                    }
                }
                rows.into_iter().filter(Value::is_object).collect()
            }
            Err(_) => vec![json!({"machine":self.id,"status":"unverified",
                "reason":"Live collection failed; check pinned SSH access and collector availability"})],
        }
    }
}

#[derive(Deserialize)]
struct Audit {
    session_file: PathBuf,
    network_id: Value,
    lan_cidrs: Vec<String>,
    targets: Vec<Target>,
    #[serde(default)]
    appliances: Vec<appliance::Details>,
}

fn router_cookie(token: &str) -> io::Result<String> {
    if token.is_empty()
        || !token.bytes().all(
            |byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e),
        )
    {
        return Err(error("Invalid router session."));
    }
    Ok(format!("s={token}"))
}

fn router_reservations(config: &Audit) -> Result<Value, Box<dyn Error>> {
    let session: Value = serde_json::from_reader(File::open(&config.session_file)?)?;
    let token = string(&session, "session_token")?;
    let cookie = router_cookie(token)?;
    let id = config
        .network_id
        .as_str()
        .map(str::to_owned)
        .or_else(|| config.network_id.as_u64().map(|id| id.to_string()))
        .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| error("Invalid router network identity."))?;
    let client = Client::builder()
        .tls_backend_native()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(30))
        .build()?;
    let response = client
        .get(format!(
            "https://api-user.e2ro.com/2.2/networks/{id}/reservations"
        ))
        .header("Cookie", cookie)
        .header("User-Agent", "eero-api-rs/0.1")
        .header("Content-Type", "application/json")
        .send()
        .map_err(reqwest::Error::without_url)?
        .error_for_status()
        .map_err(reqwest::Error::without_url)?;
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(error("Router response exceeded size limit.").into());
    }
    let report: Value = serde_json::from_slice(&bytes)?;
    if report["meta"].get("code").is_some() && report["meta"]["code"] != 200 {
        return Err(error("Router rejected read.").into());
    }
    Ok(report["data"].clone())
}

fn add_missing_coverage(catalog: &[Value], targets: &[Target], rows: &mut Vec<Value>) {
    let coverage = targets
        .iter()
        .map(|target| target.id.as_str())
        .collect::<BTreeSet<_>>();
    let observed = rows
        .iter()
        .filter_map(|row| row["machine"].as_str().map(str::to_owned))
        .collect::<BTreeSet<_>>();
    for record in catalog {
        let spec = &record["spec"];
        let Some(id) = record["id"]
            .as_str()
            .filter(|_| spec["lifecycle"] == "active")
        else {
            continue;
        };
        let resources = &spec["resources"];
        let (parent, guest) = guest_identity(spec);
        let guest_covered = parent.zip(guest).is_some_and(|(parent, guest)| {
            ["qemu", "lxc", "tart"]
                .iter()
                .any(|kind| observed.contains(&format!("{parent}/{kind}/{guest}")))
        });
        let machine = resources["kind"]
            .as_str()
            .is_some_and(|kind| matches!(kind, "machine" | "virtual-machine" | "container"))
            || spec["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|cap| cap == "machine"));
        let audit = &resources["network_authority_audit"];
        if audit["firmware_source"].is_string() && !observed.contains(id) {
            let at = audit["at"].as_str().unwrap_or("unknown");
            rows.push(json!({"machine":id,"status":"unverified",
                "reason":format!("Installed firmware was not refreshed by this audit. Last recorded audit was {at}.")}));
        } else if machine && !coverage.contains(id) && !guest_covered {
            rows.push(json!({"machine":id,"status":"unverified",
                "reason":"No direct live collector; guest observations may appear under its hypervisor"}));
        }
        for kind in ["kvm", "amt"] {
            if audit.get(kind).is_some() {
                let at = audit[kind]["at"]
                    .as_str()
                    .or_else(|| audit["at"].as_str())
                    .unwrap_or("unknown");
                rows.push(json!({"machine":format!("{id}/{kind}"),"status":"unverified",
                    "reason":format!("Appliance configuration not refreshed by this collector; last recorded audit: {at}")}));
            }
        }
    }
}

pub fn run(private: &Path) -> Result<Value, Box<dyn Error>> {
    let mut config: Audit =
        serde_json::from_reader(File::open(private.join("network-audit.json"))?)?;
    let reservations = router_reservations(&config)?;
    let reader = RegistryClient::from_value(serde_json::from_reader(File::open(
        private.join("reader.json"),
    )?)?)?;
    let entries = reader.request(Method::Get, "/v1/services", None, None)?;
    let catalog = entries["services"]
        .as_array()
        .ok_or_else(|| error("Registry did not return a catalog."))?;
    let by_id = catalog
        .iter()
        .filter_map(|row| row["id"].as_str().map(|id| (id, &row["spec"])))
        .collect::<HashMap<_, _>>();
    for target in &mut config.targets {
        if let Some(spec) = by_id.get(target.id.as_str())
            && let Some(endpoint) = spec["endpoints"]["ssh"].as_str()
            && let Some(address) = Url::parse(endpoint)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
        {
            let ip: IpAddr = address.parse()?;
            let user = target
                .destination
                .split('@')
                .next()
                .ok_or_else(|| error("Invalid pinned audit username."))?;
            target.destination = format!("{user}@{ip}");
        }
        let host = target.id.strip_prefix("machine-").unwrap_or(&target.id);
        target.tart_names = catalog
            .iter()
            .filter(|row| row["spec"]["host"] == host)
            .filter_map(|row| {
                row["spec"]["resources"]["vm_name"]
                    .as_str()
                    .map(str::to_owned)
            })
            .collect();
    }
    let mut rows = Vec::new();
    for group in config.targets.chunks(3) {
        std::thread::scope(|scope| -> io::Result<()> {
            let jobs: Vec<_> = group
                .iter()
                .map(|target| scope.spawn(|| target.collect(private)))
                .collect();
            for job in jobs {
                rows.extend(
                    job.join()
                        .map_err(|_| error("Audit collector did not complete."))?,
                );
            }
            Ok(())
        })?;
    }
    add_missing_coverage(catalog, &config.targets, &mut rows);
    if !config.appliances.is_empty() {
        let credentials: Value =
            serde_json::from_reader(File::open(private.join("appliance-credentials.json"))?)?;
        for target in &mut config.appliances {
            let (machine, kind) = target
                .id
                .rsplit_once('/')
                .ok_or_else(|| error("Invalid appliance identity."))?;
            let spec = by_id
                .get(machine)
                .ok_or_else(|| error("Appliance endpoint missing from registry."))?;
            let entry = &spec["resources"]["access"][if kind == "kvm" { "console" } else { kind }];
            let endpoint = spec["endpoints"][kind]
                .as_str()
                .or_else(|| entry["url"].as_str())
                .or_else(|| entry.as_str())
                .ok_or_else(|| error("Appliance endpoint missing from registry."))?;
            target.url = endpoint.to_owned();
            rows.retain(|row| row["machine"] != target.id);
            if let Some(gateway) = &target.via_machine {
                let source = config
                    .targets
                    .iter()
                    .find(|item| &item.id == gateway)
                    .ok_or_else(|| error("Appliance vantage unavailable."))?;
                let payload = serde_json::to_vec(
                    &json!({"target":target,"credentials":credentials[&target.id]}),
                )?;
                let result = source
                    .call(private, &["--appliance"], Some(&payload), 90)
                    .and_then(|body| {
                        serde_json::from_slice::<Vec<Value>>(&body).map_err(io::Error::other)
                    });
                match result {
                    Ok(mut result) => {
                        for row in &mut result { row["vantage"] = json!(gateway); }
                        rows.extend(result);
                    }
                    Err(_) => rows.push(json!({"machine":target.id,"status":"unverified","reason":"Authenticated appliance read vantage unavailable"})),
                }
            } else {
                rows.extend(appliance::report(target, private, &credentials[&target.id]));
            }
        }
    }
    let mut result = compare(
        rows,
        &reservations,
        &config
            .lan_cidrs
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )?;
    let mut seen = BTreeSet::new();
    result.retain(|row| seen.insert(row.to_string()));
    let violations = result
        .iter()
        .filter(|row| row["status"] == "violation")
        .count();
    let unverified = result
        .iter()
        .filter(|row| row["status"] == "unverified")
        .count();
    let warnings = result
        .iter()
        .filter(|row| {
            row["warning"]
                .as_str()
                .is_some_and(|warning| !warning.is_empty())
        })
        .count();
    let status = if violations > 0 {
        "violations"
    } else if unverified > 0 {
        "incomplete"
    } else {
        "passed"
    };
    Ok(
        json!({"status":status,"rows":result,"violations":violations,"unverified":unverified,"warnings":warnings,"checked_at":timestamp(),
        "message":format!("{violations} violations · {unverified} unverified checks · {warnings} warnings"),
        "scope":"Fresh router reservations, live host and appliance settings, and read-only saved profiles. Offline verification is distinct from live verification; inaccessible configurations remain unverified."}),
    )
}

#[cfg(test)]
mod tests {
    use super::router_cookie;

    #[test]
    fn router_tokens_accept_cookie_characters_without_allowing_another_cookie() {
        assert_eq!(router_cookie("opaque:v1=0").unwrap(), "s=opaque:v1=0");
        for invalid in ["", "two words", "a;b=other", "line\r\nbreak", "quoted\""] {
            assert!(router_cookie(invalid).is_err());
        }
    }
}

#[cfg(test)]
#[test]
fn the_transient_collector_consumes_only_its_artifact_before_forwarding_private_input() {
    let binary = b"#!/bin/sh\ncat\n";
    let private = b"private-stdin\n\0\xef";
    let combined = [binary.as_slice(), private.as_slice()].concat();
    let command = &mut Command::new("/bin/sh");
    command.args([
        "-ec",
        TRANSIENT_COLLECTOR,
        "audit",
        &binary.len().to_string(),
        "--appliance",
    ]);
    let mut process = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    process.stdin.take().unwrap().write_all(&combined).unwrap();
    let actual = process.wait_with_output().unwrap();
    assert!(actual.status.success());
    assert_eq!(actual.stdout, private);
}
