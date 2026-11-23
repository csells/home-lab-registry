use std::{
    collections::BTreeSet,
    io::Read,
    process::{Command, Stdio},
    time::Duration,
};

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wait_timeout::ChildExt;

use crate::{
    Rejected,
    provider::unavailable,
    proxmox::{
        Proxmox, TaskKind, api_id, as_list, config_text, escape, first_property, identifier,
        truthy, uuid_field,
    },
    tokens::{hash, hex},
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Shape {
    pub vcpus: i64,
    pub memory_mib: i64,
    pub disk_mib: i64,
}

impl Shape {
    fn fits(self, maximum: Self) -> bool {
        (1..=maximum.vcpus).contains(&self.vcpus)
            && (1..=maximum.memory_mib).contains(&self.memory_mib)
            && (1..=maximum.disk_mib).contains(&self.disk_mib)
    }

    pub fn for_request(self, request: &Value) -> Result<Self, Rejected> {
        let disk = self
            .disk_mib
            .max(request["disk_mib"].as_i64().unwrap_or_default());
        let disk_mib = disk
            .checked_add(1023)
            .map(|value| value / 1024 * 1024)
            .ok_or_else(shape_unverified)?;
        Ok(Self {
            vcpus: self
                .vcpus
                .max(request["vcpus"].as_i64().unwrap_or_default()),
            memory_mib: self
                .memory_mib
                .max(request["memory_mib"].as_i64().unwrap_or_default()),
            disk_mib,
        })
    }
}

#[derive(Clone)]
pub(crate) struct DynamicScope {
    pub storage: String,
    pub pool: Option<String>,
    pub template_id: String,
    template_name: String,
    template_uuid: String,
    template_volume: String,
    pub default: Shape,
    pub maximum: Shape,
    inventory_command: Option<Vec<String>>,
    qualification: Option<Value>,
}

fn invalid() -> Rejected {
    Rejected::new(
        "provider_configuration_invalid",
        "Verify the enrolled Proxmox image, storage and capacity.",
        503,
    )
}

fn template_changed(reason: &str) -> Rejected {
    Rejected::new("template_identity_changed", reason, 409)
}

fn identity_changed(reason: &str) -> Rejected {
    Rejected::new("dynamic_identity_changed", reason, 409)
}

fn shape_unverified() -> Rejected {
    Rejected::new(
        "dynamic_shape_unverified",
        "The provider did not confirm the requested clone resources.",
        409,
    )
}

fn volume_unowned(reason: &str) -> Rejected {
    Rejected::new("unowned_volume", reason, 409)
}

fn storage_unverified() -> Rejected {
    Rejected::new(
        "dynamic_storage_unverified",
        "Owned physical volume inventory could not be verified.",
        409,
    )
}

fn lease_name(lease: &str) -> Result<String, Rejected> {
    hex(lease, 32)
        .then(|| format!("lease-{lease}"))
        .ok_or_else(|| Rejected::new("unknown_lease", "No lease has this identifier.", 404))
}

pub(crate) fn legacy_json(value: &Value, spaces: bool) -> String {
    let sep = if spaces { ", " } else { "," };
    let entry = if spaces { ": " } else { ":" };
    match value {
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|item| legacy_json(item, spaces))
                .collect::<Vec<_>>()
                .join(sep)
        ),
        Value::Object(items) => {
            let mut items: Vec<_> = items.iter().collect();
            items.sort_unstable_by_key(|(key, _)| *key);
            format!(
                "{{{}}}",
                items
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}{entry}{}",
                        legacy_json(&json!(key), spaces),
                        legacy_json(value, spaces)
                    ))
                    .collect::<Vec<_>>()
                    .join(sep)
            )
        }
        Value::String(_) => {
            use std::fmt::Write as _;
            let mut text = String::new();
            for character in value.to_string().chars() {
                if character < '\u{7f}' {
                    text.push(character);
                } else {
                    for unit in character.encode_utf16(&mut [0; 2]) {
                        write!(&mut text, "\\u{unit:04x}")
                            .expect("writing to a String cannot fail");
                    }
                }
            }
            text
        }
        _ => value.to_string(),
    }
}

pub(crate) fn disk_mib(config: &Value) -> Result<i64, Rejected> {
    let mut sizes = config["scsi0"]
        .as_str()
        .ok_or_else(shape_unverified)?
        .split(',')
        .filter_map(|part| part.strip_prefix("size="));
    let size = sizes
        .next()
        .and_then(|value| value.strip_suffix('G'))
        .filter(|value| !value.starts_with('0') && value.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|value| value.checked_mul(1024))
        .ok_or_else(shape_unverified)?;
    if sizes.next().is_some() {
        return Err(shape_unverified());
    }
    Ok(size)
}

fn numbered(key: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| {
        key.strip_prefix(prefix)
            .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

fn network(config: &Value) -> Result<Vec<&str>, Rejected> {
    let failed = || {
        Rejected::new(
            "recipe_network_unverified",
            "The recipe needs a single active DHCP interface.",
            409,
        )
    };
    let values = config.as_object().ok_or_else(failed)?;
    let net = values
        .keys()
        .filter(|name| numbered(name, &["net"]))
        .collect::<Vec<_>>();
    let ip = values
        .keys()
        .filter(|name| numbered(name, &["ipconfig"]))
        .collect::<Vec<_>>();
    if net != ["net0"] || ip != ["ipconfig0"] || config["ipconfig0"] != "ip=dhcp" {
        return Err(failed());
    }
    let mut parts = config["net0"].as_str().ok_or_else(failed)?.split(',');
    let mac = parts
        .next()
        .and_then(|value| value.strip_prefix("virtio="))
        .ok_or_else(failed)?;
    let groups: Vec<_> = mac.split(':').collect();
    if groups.len() != 6
        || !groups
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(failed());
    }
    let mut options: Vec<_> = parts.collect();
    if options
        .iter()
        .filter(|item| item.starts_with("bridge="))
        .count()
        != 1
        || options.contains(&"link_down=1")
    {
        return Err(failed());
    }
    options.sort_unstable();
    Ok(options)
}

fn command_files(command: &[String]) -> Result<Vec<Value>, Rejected> {
    let (program, args) = command.split_first().ok_or_else(storage_unverified)?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| storage_unverified())?;
    let stdout = child.stdout.take().expect("piped stdout");
    let read = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let result = child.wait_timeout(Duration::from_secs(8));
    if !matches!(&result, Ok(Some(_))) {
        child.kill().map_err(|_| storage_unverified())?;
        child.wait().map_err(|_| storage_unverified())?;
        return Err(storage_unverified());
    }
    let status = result
        .map_err(|_| storage_unverified())?
        .expect("completed command");
    let bytes = read
        .join()
        .map_err(|_| storage_unverified())?
        .map_err(|_| storage_unverified())?;
    if !status.success() || bytes.len() > 64 * 1024 {
        return Err(storage_unverified());
    }
    as_list(serde_json::from_slice(&bytes).map_err(|_| storage_unverified())?)
        .map_err(|_| storage_unverified())
}

impl DynamicScope {
    pub fn open(settings: &Value, api: &Proxmox, storage: &str) -> Result<Self, Rejected> {
        let optional_name = |key: &str| match &settings[key] {
            Value::Null => Ok(None),
            Value::String(name) if identifier(name) => Ok(Some(name.clone())),
            _ => Err(invalid()),
        };
        let template_id = api_id(&settings["template_id"])
            .filter(|value| *value != api.guest_id)
            .ok_or_else(invalid)?;
        let template_name = config_text(settings, "template_name")?.to_owned();
        let template_uuid = uuid_field(&settings["template_uuid"])
            .ok_or_else(invalid)?
            .to_owned();
        let template_volume = config_text(settings, "template_volume")?.to_owned();
        let default: Shape =
            serde_json::from_value(settings["default"].clone()).map_err(|_| invalid())?;
        let maximum: Shape =
            serde_json::from_value(settings["maximum"].clone()).map_err(|_| invalid())?;
        if !identifier(storage)
            || !identifier(&template_name)
            || !template_volume.starts_with(&format!("{storage}:"))
            || !default.fits(maximum)
            || default.disk_mib % 1024 != 0
            || maximum.disk_mib % 1024 != 0
        {
            return Err(invalid());
        }
        let inventory_command = if settings["inventory_command"].is_null() {
            None
        } else {
            let args: Vec<String> = serde_json::from_value(settings["inventory_command"].clone())
                .map_err(|_| invalid())?;
            if args.is_empty() || args.iter().any(String::is_empty) {
                return Err(invalid());
            }
            Some(args)
        };
        let qualification = if settings["qualification"].is_null() {
            None
        } else {
            let record = &settings["qualification"];
            let valid = record.as_object().is_some_and(|map| map.len() == 3)
                && ["image_sha256", "template_config_sha256"]
                    .into_iter()
                    .all(|key| record[key].as_str().is_some_and(|value| hex(value, 64)))
                && record["bootstrap_revision"]
                    .as_str()
                    .is_some_and(identifier);
            if !valid {
                return Err(invalid());
            }
            Some(record.clone())
        };
        Ok(Self {
            storage: storage.to_owned(),
            pool: optional_name("pool")?,
            template_id,
            template_name,
            template_uuid,
            template_volume,
            default,
            maximum,
            inventory_command,
            qualification,
        })
    }

    pub fn identity(&self, api: &Proxmox) -> Vec<Value> {
        let mut binding = vec![
            json!(api.base()),
            json!(api.node),
            json!(api.guest_id),
            json!(api.identity),
            json!(self.storage),
            json!(self.template_id),
            json!(self.template_name),
            json!(self.template_uuid),
            json!(self.template_volume),
            json!(self.default),
            json!(self.maximum),
        ];
        if let Some(pool) = &self.pool {
            binding.push(json!(pool));
        }
        if let Some(qualification) = &self.qualification {
            binding.push(qualification.clone());
        }
        binding
    }

    fn template_route(&self, api: &Proxmox) -> String {
        format!("/nodes/{}/qemu/{}", api.node, self.template_id)
    }

    pub fn template_config(&self, api: &Proxmox) -> Result<Value, Rejected> {
        let config = api.get(&format!("{}/config", self.template_route(api)))?;
        let uuid = format!("uuid={}", self.template_uuid);
        if config["name"] != self.template_name
            || config["template"] != 1
            || config["ipconfig0"] != "ip=dhcp"
            || truthy(&config["lock"])
            || first_property(&config["scsi0"]) != Some(&self.template_volume)
            || !config["smbios1"]
                .as_str()
                .is_some_and(|meta| meta.split(',').any(|item| item == uuid))
            || disk_mib(&config).ok() != Some(self.default.disk_mib)
        {
            return Err(template_changed(
                "The enrolled clean DHCP template could not be verified.",
            ));
        }
        if let Some(qualification) = &self.qualification {
            let mut clean = config.clone();
            let object = clean.as_object_mut().ok_or_else(unavailable)?;
            object.remove("digest");
            object.remove("lock");
            if qualification["template_config_sha256"] != hash(&legacy_json(&clean, false)) {
                return Err(template_changed(
                    "The versioned template configuration changed.",
                ));
            }
            network(&config)?;
        }
        Ok(config)
    }

    pub fn preflight_storage(&self, status: &Value) -> Result<(), Rejected> {
        if self.qualification.is_some()
            && (status["type"] != "dir"
                || !status["content"]
                    .as_str()
                    .is_some_and(|value| value.split(',').any(|item| item == "images")))
        {
            return Err(Rejected::new(
                "recipe_storage_unverified",
                "The qualified raw full-clone recipe requires directory storage with VM images enabled.",
                409,
            ));
        }
        Ok(())
    }

    fn storage_files(&self) -> Result<BTreeSet<String>, Rejected> {
        let files = command_files(
            self.inventory_command
                .as_deref()
                .ok_or_else(storage_unverified)?,
        )?;
        if files.len() > 8 {
            return Err(storage_unverified());
        }
        files
            .iter()
            .map(|entry| {
                if !entry.as_object().is_some_and(|object| object.len() == 1) {
                    return Err(storage_unverified());
                }
                entry["volid"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(storage_unverified)
            })
            .collect()
    }

    pub fn verify_absent(&self, api: &Proxmox) -> Result<(), Rejected> {
        let held = || {
            Rejected::new(
                "dynamic_scope_occupied",
                "The enrolled guest ID or its scoped storage is occupied.",
                409,
            )
        };
        let inventory = as_list(api.audit("/cluster/resources?type=vm")?)?;
        if inventory
            .iter()
            .any(|row| api_id(&row["vmid"]).is_some_and(|id| id == api.guest_id))
            || !self.storage_files()?.is_empty()
        {
            return Err(held());
        }
        if let Some(pool) = &self.pool {
            if api.get(&format!("/pools/{}", escape(pool)))?["members"] != json!([]) {
                return Err(held());
            }
        }
        Ok(())
    }

    pub fn clone(&self, api: &Proxmox, lease: &str) -> Result<Value, Rejected> {
        let pool = self.pool.as_deref().ok_or_else(|| {
            Rejected::new(
                "dynamic_scope_unverified",
                "Enroll a dedicated pool before allocating new clones.",
                409,
            )
        })?;
        self.template_config(api)?;
        self.verify_absent(api)?;
        api.modify(
            Method::POST,
            &format!("{}/clone", self.template_route(api)),
            Some(&[
                ("newid", &api.guest_id),
                ("name", &lease_name(lease)?),
                ("full", "1"),
                ("format", "raw"),
                ("storage", &self.storage),
                ("pool", pool),
            ]),
        )
    }

    fn attached_volume(&self, api: &Proxmox, config: &Value) -> Result<String, Rejected> {
        let mut boot = Vec::new();
        let mut cloud_init = None;
        let owned_prefix = format!(
            "{}:{}/vm-{}-disk-",
            self.storage, api.guest_id, api.guest_id
        );
        let expected_cloud = format!(
            "{}:{}/vm-{}-cloudinit.raw",
            self.storage, api.guest_id, api.guest_id
        );
        for (key, value) in config.as_object().ok_or_else(unavailable)? {
            if !numbered(
                key,
                &[
                    "scsi", "sata", "ide", "virtio", "unused", "efidisk", "tpmstate",
                ],
            ) {
                continue;
            }
            let volume = first_property(value).ok_or_else(unavailable)?;
            if volume == "none" {
                continue;
            }
            if key == "ide2" && volume == expected_cloud {
                cloud_init = Some(volume);
                continue;
            }
            let valid = volume
                .strip_prefix(&owned_prefix)
                .and_then(|tail| tail.strip_suffix(".raw"))
                .is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
                });
            if !valid {
                return Err(volume_unowned(
                    "The guest contains a volume outside its enrolled staging scope.",
                ));
            }
            boot.push((key.as_str(), volume));
        }
        let [("scsi0", volume)] = boot.as_slice() else {
            return Err(volume_unowned(
                "The dynamic recipe requires one enrolled boot volume.",
            ));
        };
        let mut owned = self.storage_files()?;
        if let Some(cloud_init) = cloud_init {
            owned.remove(cloud_init);
        }
        if owned != BTreeSet::from([(*volume).to_owned()]) {
            return Err(volume_unowned(
                "Scoped storage does not match the guest’s attached boot volume.",
            ));
        }
        Ok((*volume).to_owned())
    }

    pub fn describe(&self, api: &Proxmox, lease: &str) -> Result<Value, Rejected> {
        if let Some(pool) = &self.pool {
            let reply = api.get(&format!("/pools/{}", escape(pool)))?;
            let exact = reply["members"]
                .as_array()
                .filter(|members| members.len() == 1)
                .is_some_and(|members| {
                    members[0]["type"] == "qemu"
                        && members[0]["node"] == api.node
                        && api_id(&members[0]["vmid"]).is_some_and(|id| id == api.guest_id)
                });
            if !exact {
                return Err(identity_changed(
                    "The allocation pool does not contain exactly the owned clone.",
                ));
            }
        }
        let config = api.get(&format!("{}/config", api.guest_route()))?;
        if config["name"] != lease_name(lease)?
            || truthy(&config["template"])
            || truthy(&config["lock"])
            || config["ipconfig0"] != "ip=dhcp"
        {
            return Err(identity_changed(
                "The allocated clone failed exact identity or DHCP verification.",
            ));
        }
        if self.qualification.is_some()
            && network(&config)? != network(&self.template_config(api)?)?
        {
            return Err(Rejected::new(
                "recipe_network_unverified",
                "The clone network differs from the enrolled template.",
                409,
            ));
        }
        let mut found = config["smbios1"]
            .as_str()
            .ok_or_else(unavailable)?
            .split(',')
            .filter_map(|value| value.strip_prefix("uuid="));
        let hardware = found
            .next()
            .ok_or_else(|| identity_changed("The clone lacks an independent hardware identity."))?;
        if found.next().is_some()
            || hardware == self.template_uuid
            || uuid_field(&json!(hardware)).is_none()
        {
            return Err(identity_changed(
                "The clone lacks an independent hardware identity.",
            ));
        }
        let volume = self.attached_volume(api, &config)?;
        let generation = uuid_field(&config["vmgenid"])
            .ok_or_else(|| identity_changed("The clone lacks an independent VM generation."))?;
        Ok(json!({"uuid":hardware,"volume":volume,"generation":generation}))
    }

    fn config_matches(
        &self,
        api: &Proxmox,
        lease: &str,
        pins: &Value,
        shape: Shape,
        check_disk: bool,
    ) -> Result<i64, Rejected> {
        let current = api.get(&format!("{}/config", api.guest_route()))?;
        let disk = disk_mib(&current)?;
        let number = |key: &str| {
            current[key]
                .as_i64()
                .or_else(|| current[key].as_str()?.parse().ok())
        };
        if self.describe(api, lease)? != *pins
            || number("memory") != Some(shape.memory_mib)
            || number("cores") != Some(shape.vcpus)
            || truthy(&current["lock"])
            || (check_disk && disk != shape.disk_mib)
        {
            return Err(shape_unverified());
        }
        Ok(disk)
    }

    pub fn apply_shape(
        &self,
        api: &Proxmox,
        lease: &str,
        pins: &Value,
        shape: Shape,
    ) -> Result<i64, Rejected> {
        if self.describe(api, lease)? != *pins {
            return Err(identity_changed(
                "The clone identity changed before sizing.",
            ));
        }
        api.modify(
            Method::PUT,
            &format!("{}/config", api.guest_route()),
            Some(&[
                ("memory", &shape.memory_mib.to_string()),
                ("cores", &shape.vcpus.to_string()),
                ("sockets", "1"),
                ("balloon", "0"),
            ]),
        )?;
        self.config_matches(api, lease, pins, shape, false)
    }

    pub fn verify_shape(
        &self,
        api: &Proxmox,
        lease: &str,
        pins: &Value,
        shape: Shape,
    ) -> Result<(), Rejected> {
        self.config_matches(api, lease, pins, shape, true)
            .map(|_| ())
    }

    pub fn resize(
        &self,
        api: &Proxmox,
        lease: &str,
        pins: &Value,
        shape: Shape,
    ) -> Result<Value, Rejected> {
        let old = self.config_matches(api, lease, pins, shape, false)?;
        if old >= shape.disk_mib {
            return Err(shape_unverified());
        }
        api.modify(
            Method::PUT,
            &format!("{}/resize", api.guest_route()),
            Some(&[
                ("disk", "scsi0"),
                ("size", &format!("{}G", shape.disk_mib / 1024)),
            ]),
        )
    }

    pub fn recent_clone_tasks(&self, api: &Proxmox) -> Result<BTreeSet<String>, Rejected> {
        api.recent_tasks(&self.template_id, TaskKind::Clone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn admitted_disk_size_rounds_up_without_changing_other_requirements() {
        let default = Shape {
            vcpus: 1,
            memory_mib: 512,
            disk_mib: 8192,
        };
        assert_eq!(
            default
                .for_request(&json!({"vcpus":2,"disk_mib":8200}))
                .unwrap(),
            Shape {
                vcpus: 2,
                memory_mib: 512,
                disk_mib: 9216
            }
        );
        assert_eq!(
            disk_mib(&json!({"scsi0":"scope:vm-901.raw,size=9G"})).unwrap(),
            9216
        );
        assert!(disk_mib(&json!({"scsi0":"scope:vm-901.raw,size=9G,size=10G"})).is_err());
    }

    #[test]
    fn the_legacy_identity_and_configuration_digest_use_their_original_encoding() {
        let value = json!(["configuration", {"z":"x", "a":"λ😄"}]);
        assert_eq!(
            legacy_json(&value, true),
            "[\"configuration\", {\"a\": \"\\u03bb\\ud83d\\ude04\", \"z\": \"x\"}]"
        );
        assert_eq!(
            legacy_json(&value, false),
            "[\"configuration\",{\"a\":\"\\u03bb\\ud83d\\ude04\",\"z\":\"x\"}]"
        );
    }
}
