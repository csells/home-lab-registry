use std::{
    collections::BTreeMap,
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Cursor},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};
use yaml_rust2::YamlLoader;

use super::{appliance, child, error, string};

type Entries = Vec<Value>;
type Err = Box<dyn Error>;

fn run(program: &str, args: &[&str]) -> io::Result<String> {
    let result = child(Command::new(program).args(args), None, 20, 4 * 1024 * 1024)?;
    if !result.status.success() {
        return Err(error("Read command failed."));
    }
    String::from_utf8(result.stdout).map_err(io::Error::other)
}

fn json_command(program: &str, args: &[&str]) -> Result<Value, Err> {
    Ok(serde_json::from_str(&run(program, args)?)?)
}

fn dir_files(root: &Path, folder: &str, filter: impl Fn(&str) -> bool) -> io::Result<Vec<PathBuf>> {
    let directory = root.join(folder);
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let mut result = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(&filter) && entry.path().is_file() {
            result.push(entry.path());
        }
    }
    result.sort();
    Ok(result)
}

fn ifupdown(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = ["etc/network/interfaces", "etc/network/interfaces.new"]
        .iter()
        .map(|path| root.join(path))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.extend(dir_files(root, "etc/network/interfaces.d", |_| true)?);
    Ok(paths)
}

fn profile_text(path: &Path) -> io::Result<String> {
    // Configuration contents stay on the system being inspected. Only
    // normalized address evidence is returned to the operator.
    if path.metadata()?.len() > 1024 * 1024 {
        return Err(error("Network profile exceeded its read limit."));
    }
    fs::read_to_string(path)
}

pub fn linux_links(interfaces: &Value, links: &Value) -> Result<Vec<Value>, Err> {
    let macs = links
        .as_array()
        .ok_or_else(|| error("Invalid link inventory."))?
        .iter()
        .filter_map(|link| {
            Some((
                link["ifname"].as_str()?,
                link["address"].as_str().unwrap_or_default(),
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let interfaces = interfaces
        .as_array()
        .ok_or_else(|| error("Invalid interface inventory."))?;
    let mut result = Vec::new();
    for link in interfaces {
        let name = string(link, "ifname")?;
        let mac = macs
            .get(name)
            .copied()
            .or_else(|| link["address"].as_str())
            .unwrap_or_default();
        for address in link["addr_info"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|addr| addr["family"] == "inet")
        {
            let mode = if address["dynamic"] == true
                || address["flags"]
                    .as_array()
                    .is_some_and(|flags| flags.iter().any(|flag| flag == "dynamic"))
            {
                "dhcp"
            } else {
                "unknown"
            };
            result.push(json!({"address":string(address,"local")?,"mac":mac,"interface":name,"assignment":mode,"source":"live interfaces"}));
        }
    }
    Ok(result)
}

fn ip_literal(text: &str) -> Option<Ipv4Addr> {
    text.trim().split('/').next()?.parse().ok()
}

fn address_line(line: &str) -> Option<&str> {
    let (key, value) = line
        .trim()
        .split_once(|ch: char| ch == '=' || ch == ':' || ch.is_whitespace())?;
    let lower = key.to_ascii_lowercase();
    if matches!(lower.as_str(), "address" | "addresses" | "ip_address")
        || lower.strip_prefix("address").is_some_and(numeric)
    {
        Some(value.trim().trim_start_matches(['=', ' ']))
    } else {
        None
    }
}

fn numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn ipv4_words(text: &str) -> impl Iterator<Item = Ipv4Addr> + '_ {
    text.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .filter_map(|word| word.parse().ok())
}

fn linux() -> Result<Entries, Err> {
    let addresses = json_command("ip", &["-j", "-4", "addr"])?;
    let links = json_command("ip", &["-j", "link"])?;
    let mut rows = linux_links(&addresses, &links)?;
    let root = Path::new("/");
    let mut paths = ifupdown(root)?;
    paths.extend(dir_files(root, "etc/netplan", |_| true)?);
    paths.extend(dir_files(root, "etc/systemd/network", |name| {
        name.ends_with(".network")
    })?);
    paths.extend(dir_files(
        root,
        "etc/NetworkManager/system-connections",
        |_| true,
    )?);
    paths.extend(dir_files(root, "etc/cloud/cloud.cfg.d", |name| {
        name.contains("network")
    })?);
    let mut modes = BTreeMap::new();
    let mut saved = Vec::new();
    for path in paths {
        let profile = profile_text(&path)?;
        let filename = path.display().to_string();
        if filename.contains("/netplan/") || filename.contains("/cloud/") {
            if profile.lines().any(|line| {
                let line = line.trim();
                line.starts_with("addresses:")
                    || line
                        .strip_prefix("dhcp4:")
                        .is_some_and(|value| matches!(value.trim(), "false" | "no"))
            }) {
                saved.push(json!({"reason":"Saved YAML network profile needs structured validation","source":filename}));
            }
            continue;
        }
        let mut interface = None;
        for line in profile
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
        {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if let ["iface", name, "inet", mode, ..] = fields.as_slice() {
                interface = Some(*name);
                modes.insert((*name).to_owned(), (*mode).to_owned());
            }
            if let Some(addresses) = address_line(line) {
                for address in ipv4_words(addresses.split(',').next().unwrap_or_default()) {
                    let matched = rows.iter().find(|row| {
                        if let Some(name) = interface {
                            row["interface"] == name
                        } else {
                            row["address"] == address.to_string()
                        }
                    });
                    let link_mac = links
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|link| interface.is_some_and(|name| link["ifname"] == name))
                        .and_then(|link| link["address"].as_str());
                    let mac = matched
                        .and_then(|row| row["mac"].as_str())
                        .or(link_mac)
                        .unwrap_or_default();
                    saved.push(json!({"address":address,"mac":mac,"interface":interface,"assignment":"static","source":filename}));
                }
            }
        }
    }
    for row in &mut rows {
        if row["assignment"] != "dhcp"
            && let Some(mode) = row["interface"].as_str().and_then(|name| modes.get(name))
            && matches!(mode.as_str(), "dhcp" | "static")
        {
            row["assignment"] = json!(mode);
        }
    }
    rows.extend(saved);
    if rows.is_empty() {
        rows.push(json!({"reason":"No interface evidence collected"}));
    }
    Ok(rows)
}

fn plist_read(path: &Path) -> Result<Value, Err> {
    let settings = plist::Value::from_file(path)?;
    Ok(serde_json::to_value(settings)?)
}

fn plist_query(binary: &str, args: &[&str], timeout: u64) -> Result<Value, Err> {
    let out = child(
        Command::new(binary).args(args),
        None,
        timeout,
        4 * 1024 * 1024,
    )?;
    if !out.status.success() {
        return Err(error("Property list command failed.").into());
    }
    Ok(serde_json::to_value(plist::Value::from_reader(
        Cursor::new(out.stdout),
    )?)?)
}

fn macos() -> Result<Entries, Err> {
    let prefs = plist_read(Path::new(
        "/Library/Preferences/SystemConfiguration/preferences.plist",
    ))?;
    let mut modes = BTreeMap::new();
    let mut rows = Vec::new();
    for service in prefs["NetworkServices"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
    {
        let interface = service["Interface"]["DeviceName"]
            .as_str()
            .unwrap_or_default();
        if interface.is_empty() {
            continue;
        }
        let ipv4 = &service["IPv4"];
        modes.insert(interface, ipv4["ConfigMethod"].as_str().unwrap_or_default());
        for address in ipv4["Addresses"].as_array().into_iter().flatten() {
            rows.push(json!({"address":address,"interface":interface,"assignment":"static","source":"saved macOS network service"}));
        }
    }
    let raw = run("/sbin/ifconfig", &["-a"])?;
    let mut macs = BTreeMap::new();
    let mut current = "";
    let mut packets: BTreeMap<&str, Result<String, io::Error>> = BTreeMap::new();
    for line in raw.lines() {
        if !line.starts_with(char::is_whitespace) {
            current = line.split(':').next().unwrap_or_default();
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        match fields.as_slice() {
            ["ether", mac, ..] => {
                macs.insert(current, *mac);
            }
            ["inet", ip, ..] => {
                let mode = match modes.get(current).copied() {
                    Some("Manual") => "static",
                    Some("DHCP") => match packets
                        .entry(current)
                        .or_insert_with(|| run("/usr/sbin/ipconfig", &["getpacket", current]))
                    {
                        Ok(lease)
                            if lease.lines().any(|line| {
                                line.split_once('=').is_some_and(|(key, value)| {
                                    key.trim() == "yiaddr" && value.trim() == *ip
                                })
                            }) =>
                        {
                            "dhcp"
                        }
                        Ok(_) => "static",
                        Err(_) => "unknown",
                    },
                    _ => "unknown",
                };
                rows.push(json!({"address":ip,"interface":current,"assignment":mode,"source":"live macOS interfaces and DHCP lease"}));
            }
            _ => (),
        }
    }
    for row in &mut rows {
        row["mac"] = json!(
            row["interface"]
                .as_str()
                .and_then(|name| macs.get(name))
                .copied()
                .unwrap_or_default()
        );
    }
    Ok(rows)
}

fn saved_row(rows: &mut Entries, mac: &str, mode: &str, address: Option<&str>, source: &str) {
    let ip = address.and_then(ip_literal);
    if address.is_some() && ip.is_none() {
        return;
    }
    rows.push(
        json!({"assignment":mode,"address":ip,"mac":mac,"scope":"offline",
        "profile_verified":true,"source":source}),
    );
}

pub fn saved_profiles(root: &Path, mac: &str, allow_empty: bool) -> Result<Entries, Err> {
    let mut rows = Vec::new();
    for path in dir_files(root, "etc/netplan", |name| name.ends_with(".yaml"))? {
        let source = format!("/{}", path.strip_prefix(root)?.display());
        let profiles = YamlLoader::load_from_str(&profile_text(&path)?)?;
        for profile in profiles {
            let network = &profile["network"];
            for kind in ["ethernets", "bridges", "vlans", "bonds", "wifis"] {
                let Some(items) = network[kind].as_hash() else {
                    continue;
                };
                for (_, item) in items {
                    if item["dhcp4"].as_bool() == Some(true) {
                        saved_row(&mut rows, mac, "dhcp", None, &source);
                    }
                    for value in item["addresses"].as_vec().into_iter().flatten() {
                        if let Some(address) = value.as_str() {
                            saved_row(&mut rows, mac, "static", Some(address), &source);
                        }
                    }
                }
            }
        }
    }
    for path in ifupdown(root)? {
        let source = format!("/{}", path.strip_prefix(root)?.display());
        for line in profile_text(&path)?
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
        {
            match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["iface", _, "inet", "dhcp", ..] => {
                    saved_row(&mut rows, mac, "dhcp", None, &source)
                }
                ["address", address, ..] => {
                    saved_row(&mut rows, mac, "static", Some(address), &source)
                }
                _ => (),
            }
        }
    }
    let mut paths = dir_files(root, "etc/systemd/network", |name| {
        name.ends_with(".network")
    })?;
    paths.extend(dir_files(
        root,
        "etc/NetworkManager/system-connections",
        |_| true,
    )?);
    for path in paths {
        let source = format!("/{}", path.strip_prefix(root)?.display());
        let text = profile_text(&path)?;
        let mut section = "";
        for line in text.lines().map(str::trim) {
            if let Some(name) = line
                .strip_prefix('[')
                .and_then(|line| line.strip_suffix(']'))
            {
                section = name;
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            if section == "ipv4" && key.trim().eq_ignore_ascii_case("method") && value == "auto"
                || section == "Network"
                    && key.trim().eq_ignore_ascii_case("dhcp")
                    && matches!(value.to_ascii_lowercase().as_str(), "yes" | "true" | "ipv4")
            {
                saved_row(&mut rows, mac, "dhcp", None, &source);
            } else if section == "ipv4"
                && key
                    .trim()
                    .to_ascii_lowercase()
                    .strip_prefix("address")
                    .is_some_and(numeric)
                || section == "Network" && key.trim() == "Address"
            {
                saved_row(&mut rows, mac, "static", value.split(',').next(), &source);
            }
        }
    }
    if rows.is_empty() && !allow_empty {
        return Err(error("No supported saved IPv4 profile found.").into());
    }
    Ok(rows)
}

struct OfflineDisk {
    tool: &'static str,
    guest: String,
    volume_path: String,
    activated: bool,
    loop_device: Option<String>,
    mount: Option<tempfile::TempDir>,
    mounted: bool,
}

impl OfflineDisk {
    fn stopped(&self) -> bool {
        run(self.tool, &["status", &self.guest])
            .is_ok_and(|value| value.split_whitespace().last() == Some("stopped"))
    }
}

impl Drop for OfflineDisk {
    fn drop(&mut self) {
        if self.mounted
            && let Some(mount) = &self.mount
            && run("umount", &[mount.path().to_str().unwrap_or_default()]).is_err()
        {
            eprintln!("Offline audit mount could not be safely detached.");
            if let Some(path) = self.mount.take() {
                let _ = path.keep();
            }
            return;
        }
        if let Some(device) = &self.loop_device
            && run("losetup", &["-d", device]).is_err()
        {
            eprintln!("Offline audit loop device could not be released.");
            return;
        }
        if self.activated {
            if !self.stopped() {
                eprintln!(
                    "Guest state changed while reading; offline audit left the source volume accessible."
                );
            } else if run("lvchange", &["-an", &self.volume_path]).is_err() {
                eprintln!("Offline audit could not restore prior volume activation.");
            }
        }
    }
}

fn local_guest_config(guest: &str, kind: &str) -> Result<Value, Err> {
    let node = run("hostname", &[])?;
    Ok(json_command(
        "pvesh",
        &[
            "get",
            &format!("/nodes/{}/{kind}/{guest}/config", node.trim()),
            "--output-format",
            "json",
        ],
    )?)
}

fn offline(guest: &str, kind: &str) -> Result<Entries, Err> {
    let config = local_guest_config(guest, kind)?;
    let properties = config
        .as_object()
        .ok_or_else(|| error("Guest configuration unavailable."))?;
    let mut macs = Vec::new();
    for (key, value) in properties {
        if !key.strip_prefix("net").is_some_and(numeric) {
            continue;
        }
        for pair in value.as_str().unwrap_or_default().split(',') {
            if let Some((name, mac)) = pair.split_once('=')
                && matches!(
                    name,
                    "virtio" | "e1000" | "e1000e" | "rtl8139" | "vmxnet3" | "hwaddr"
                )
                && mac.len() == 17
                && mac.split(':').count() == 6
                && mac
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() || byte == b':')
            {
                macs.push(mac.to_ascii_lowercase());
            }
        }
    }
    let [mac] = macs.as_slice() else {
        return Err(
            error("Offline interface mapping needs one NIC or explicit MAC mapping.").into(),
        );
    };
    let disk = if kind == "lxc" {
        config["rootfs"].as_str()
    } else {
        ["scsi0", "virtio0", "sata0"]
            .iter()
            .find_map(|key| config[key].as_str())
    }
    .ok_or_else(|| error("No supported system disk."))?;
    let volume = disk
        .split(',')
        .next()
        .filter(|volume| !volume.is_empty() && !volume.starts_with('-'))
        .ok_or_else(|| error("Invalid system volume."))?;
    let path = run("pvesm", &["path", volume])?.trim().to_owned();
    if !Path::new(&path).is_absolute() {
        return Err(error("Unsupported offline volume path.").into());
    }
    let mut source = OfflineDisk {
        tool: if kind == "qemu" { "qm" } else { "pct" },
        guest: guest.to_owned(),
        volume_path: path,
        activated: false,
        loop_device: None,
        mount: None,
        mounted: false,
    };
    if !source.stopped() {
        return Err(error("Cannot verify the guest remains stopped.").into());
    }
    if source.volume_path.starts_with("/dev/")
        && run(
            "lvs",
            &["--noheadings", "-o", "lv_active", &source.volume_path],
        )?
        .trim()
            != "active"
    {
        run("lvchange", &["-ay", "-K", &source.volume_path])?;
        source.activated = true;
    }
    let probe = child(
        Command::new("blkid").args(["-p", "-s", "TYPE", "-o", "value", &source.volume_path]),
        None,
        20,
        4096,
    )?;
    if !matches!(probe.status.code(), Some(0 | 2)) {
        return Err(error("Offline disk probe failed.").into());
    }
    let offset = if String::from_utf8_lossy(&probe.stdout).trim() == "ext4" {
        0
    } else {
        let layout = json_command("sfdisk", &["--json", &source.volume_path])?;
        let table = &layout["partitiontable"];
        let partition = table["partitions"]
            .as_array()
            .and_then(|parts| {
                parts
                    .iter()
                    .max_by_key(|part| part["size"].as_u64().unwrap_or_default())
            })
            .ok_or_else(|| error("Unsupported offline partition table."))?;
        partition["start"]
            .as_u64()
            .and_then(|start| start.checked_mul(table["sectorsize"].as_u64()?))
            .ok_or_else(|| error("Invalid offline partition offset."))?
    };
    let device = run(
        "losetup",
        &[
            "--find",
            "--show",
            "--read-only",
            "--offset",
            &offset.to_string(),
            &source.volume_path,
        ],
    )?
    .trim()
    .to_owned();
    if !device.strip_prefix("/dev/loop").is_some_and(numeric) {
        return Err(error("Invalid readonly loop device.").into());
    }
    source.loop_device = Some(device.clone());
    if run("blkid", &["-p", "-s", "TYPE", "-o", "value", &device])?.trim() != "ext4" {
        return Err(error("Unsupported offline filesystem.").into());
    }
    source.mount = Some(
        tempfile::Builder::new()
            .prefix("lab-address-read-")
            .tempdir()?,
    );
    let target = source
        .mount
        .as_ref()
        .ok_or_else(|| error("Offline mount unavailable."))?
        .path()
        .to_owned();
    run(
        "mount",
        &[
            "-o",
            "ro,noload,nodev,nosuid,noexec",
            &device,
            target
                .to_str()
                .ok_or_else(|| error("Invalid offline mount path."))?,
        ],
    )?;
    source.mounted = true;
    let cloud_init = config["template"].as_u64().is_some_and(|value| value > 0)
        && config["ipconfig0"].is_string()
        && !config["cicustom"]
            .as_str()
            .is_some_and(|value| value.contains("network="));
    let mut rows = saved_profiles(&target, mac, cloud_init)?;
    if !source.stopped() {
        return Err(error("Guest started during read; retry.").into());
    }
    for row in &mut rows {
        row["runtime"] = json!("stopped");
        row["evidence"] = json!("Saved system disk read-only; ext4 journal replay disabled");
    }
    for (key, settings) in properties {
        if key.strip_prefix("ipconfig").is_some_and(numeric)
            && let Some(address) = settings
                .as_str()
                .and_then(|value| value.split(',').find_map(|item| item.strip_prefix("ip=")))
        {
            saved_row(
                &mut rows,
                mac,
                if address == "dhcp" { "dhcp" } else { "static" },
                (address != "dhcp").then_some(address),
                &format!("Proxmox {key}"),
            );
        }
    }
    if rows.is_empty() {
        return Err(error("No verified saved address settings found.").into());
    }
    Ok(rows)
}

fn qga(id: &str, args: &[&str]) -> Result<String, Err> {
    let mut command = vec!["guest", "exec", id, "--"];
    command.extend_from_slice(args);
    let result = json_command("qm", &command)?;
    if result["exitcode"] != 0 || result["out-truncated"] == true {
        return Err(error("Guest read failed or was truncated.").into());
    }
    Ok(result["out-data"].as_str().unwrap_or_default().to_owned())
}

fn haos(id: &str) -> Result<Entries, Err> {
    let links: Value = serde_json::from_str(&qga(id, &["ip", "-j", "addr"])?)?;
    let mut rows = linux_links(&links, &links)?;
    rows.retain(|row| row["interface"] != "lo");
    for row in &mut rows {
        row["source"] = json!("HAOS live interface");
    }
    for uuid in qga(id, &["nmcli", "-g", "UUID", "connection", "show"])?.lines() {
        if uuid::Uuid::parse_str(uuid).is_err() {
            return Err(error("Invalid guest profile identity.").into());
        }
        let read = |field| qga(id, &["nmcli", "-g", field, "connection", "show", uuid]);
        let method = read("ipv4.method")?;
        let iface = read("connection.interface-name")?;
        let iface = iface.trim();
        let mac = links
            .as_array()
            .into_iter()
            .flatten()
            .find(|nic| nic["ifname"] == iface)
            .and_then(|nic| nic["address"].as_str())
            .unwrap_or_default();
        for address in read("ipv4.addresses")?
            .trim()
            .split(',')
            .filter_map(ip_literal)
        {
            rows.push(json!({"address":address,"mac":mac,"assignment":"static","source":"HAOS saved NetworkManager profile"}));
        }
        if method.trim() == "auto" {
            for row in rows.iter_mut().filter(|row| row["interface"] == iface) {
                row["assignment"] = json!("dhcp");
            }
        }
    }
    Ok(rows)
}

fn guests(guest_binary: &str) -> Result<Entries, Err> {
    let guests = json_command(
        "pvesh",
        &[
            "get",
            "/cluster/resources",
            "--type",
            "vm",
            "--output-format",
            "json",
        ],
    )?;
    let status = guests
        .as_array()
        .ok_or_else(|| error("Invalid guest inventory."))?;
    let local = run("hostname", &[])?;
    let mut rows = Vec::new();
    for group in status.chunks(2) {
        std::thread::scope(|scope| -> io::Result<()> {
            let jobs: Vec<_>=group.iter().filter(|guest|guest["node"]==local.trim()).map(|guest|scope.spawn(|| {
                let Some(id)=guest["vmid"].as_u64().filter(|id|*id>0).map(|id|id.to_string()) else { return Vec::new(); };
                let Some(kind)=guest["type"].as_str().filter(|kind|matches!(*kind,"qemu"|"lxc")) else { return Vec::new(); };
                let live = guest["status"]=="running";
                let result = if !live { offline(&id,kind) } else if kind=="qemu" {
                    qga(&id,&[guest_binary,"--guest-only"]).and_then(|body|serde_json::from_str(&body).map_err(Into::into))
                        .or_else(|_|haos(&id))
                } else {
                    json_command("pct",&["exec",&id,"--",guest_binary,"--guest-only"])
                        .and_then(|result|serde_json::from_value(result).map_err(Into::into))
                };
                let mut values=result.unwrap_or_else(|cause|vec![json!({"reason":format!("Guest check failed: {cause}"),"runtime":guest["status"]})]);
                for row in &mut values { row["suffix"]=json!(format!("/{kind}/{id}")); }
                values
            })).collect();
            for job in jobs {
                rows.extend(
                    job.join()
                        .map_err(|_| error("Guest audit did not complete."))?,
                );
            }
            Ok(())
        })?;
    }
    Ok(rows)
}

struct AttachedTart {
    disk: String,
}

impl Drop for AttachedTart {
    fn drop(&mut self) {
        if run("hdiutil", &["detach", &self.disk]).is_err() {
            eprintln!("Could not detach the read-only Tart device.");
        }
    }
}

fn tart_offline(name: &str) -> Result<Entries, Err> {
    if name.contains('/') || matches!(name, "." | ".." | "") {
        return Err(error("Invalid Tart guest name.").into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(OsString::new));
    let root = home.join(".tart/vms").join(name);
    let config: Value = serde_json::from_str(&profile_text(&root.join("config.json"))?)?;
    let image = root.join("disk.img");
    let info = plist_query(
        "hdiutil",
        &[
            "attach",
            "-readonly",
            "-nomount",
            "-plist",
            image.to_str().ok_or_else(|| error("Invalid Tart path."))?,
        ],
        45,
    )?;
    let devices = info["system-entities"]
        .as_array()
        .ok_or_else(|| error("No Tart disk attachment report."))?;
    let disk = devices
        .iter()
        .find(|entity| entity["content-hint"] == "GUID_partition_scheme")
        .and_then(|entity| entity["dev-entry"].as_str())
        .filter(|path| path.starts_with("/dev/disk"))
        .ok_or_else(|| error("Tart disk device unavailable."))?;
    let target = tempfile::Builder::new()
        .prefix("lab-tart-read-")
        .tempdir()?;
    let _attached = AttachedTart {
        disk: disk.to_owned(),
    };
    let apfs = plist_query("diskutil", &["apfs", "list", "-plist"], 20)?;
    let volume = apfs["Containers"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|container| container["Volumes"].as_array().into_iter().flatten())
        .find_map(|volume| {
            let id = volume["DeviceIdentifier"].as_str()?;
            let identity = devices.iter().any(|entity| {
                entity["dev-entry"]
                    .as_str()
                    .and_then(|path| path.strip_prefix("/dev/"))
                    == Some(id)
            });
            let data = volume["Roles"]
                .as_array()
                .is_some_and(|roles| roles.iter().any(|role| role == "Data"));
            (identity && data && id.starts_with("disk")).then_some(id)
        })
        .ok_or_else(|| error("Read-only Tart data volume was not located."))?;
    run(
        "diskutil",
        &[
            "mount",
            "readOnly",
            "-mountPoint",
            target
                .path()
                .to_str()
                .ok_or_else(|| error("Invalid Tart mount path."))?,
            volume,
        ],
    )?;
    let prefs = plist_read(
        &target
            .path()
            .join("Library/Preferences/SystemConfiguration/preferences.plist"),
    )?;
    let mac = string(&config, "macAddress")?;
    let mut rows = Vec::new();
    for service in prefs["NetworkServices"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
    {
        let ipv4 = &service["IPv4"];
        let source = "Read-only Tart SystemConfiguration";
        if ipv4["ConfigMethod"] == "DHCP" {
            saved_row(&mut rows, mac, "dhcp", None, source);
        }
        for address in ipv4["Addresses"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            saved_row(&mut rows, mac, "static", Some(address), source);
        }
    }
    if rows.is_empty() {
        return Err(error("No supported Tart profile.").into());
    }
    Ok(rows)
}

pub fn collect(guest_only: bool, tart_names: &[String], guest_binary: &str) -> Entries {
    let mut results = if cfg!(target_os = "macos") {
        macos()
    } else {
        linux()
    }
    .unwrap_or_else(|_| vec![json!({"reason":"Platform/configuration collector unavailable"})]);
    if guest_only {
        return results;
    }
    if Path::new("/usr/sbin/qm").exists() {
        match guests(guest_binary) {
            Ok(rows) => results.extend(rows),
            Err(_) => results.push(json!({"reason":"Provider guest inventory unavailable"})),
        }
    }
    if cfg!(target_os = "macos") && !tart_names.is_empty() {
        let states = json_command("/opt/homebrew/bin/tart", &["list", "--format", "json"]);
        for name in tart_names {
            let stopped = states.as_ref().is_ok_and(|list| {
                list.as_array()
                    .into_iter()
                    .flatten()
                    .any(|guest| guest["Name"] == *name && guest["Running"] == false)
            });
            let mut rows = if stopped {
                tart_offline(name)
            } else {
                Err(error("Tart guest not stopped; live guest access required.").into())
            }
            .unwrap_or_else(|reason| {
                vec![json!({"reason":format!("Tart check failed: {reason}")})]
            });
            for row in &mut rows {
                row["suffix"] = json!(format!("/tart/{name}"));
            }
            results.extend(rows);
        }
    }
    results
}

pub fn remote_appliance(input: impl io::Read) -> Result<Entries, Err> {
    appliance::remote(input)
}
