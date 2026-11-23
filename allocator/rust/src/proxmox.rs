//! The allocator's pinned HTTPS connection and guest operations for Proxmox.
use std::{collections::BTreeSet, fs, io::Read, time::Duration};

use reqwest::{Certificate, Method, blocking::Client, header, redirect, retry};
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use crate::{Rejected, provider::unavailable, protocol::public_key};

#[derive(Clone, Copy)]
pub enum TaskKind {
    Clone,
    Start,
    Shutdown,
    Rollback,
    Destroy,
    Resize,
    Resume,
}

impl TaskKind {
    fn api(self) -> &'static str {
        match self {
            Self::Clone => "qmclone",
            Self::Start => "qmstart",
            Self::Shutdown => "qmshutdown",
            Self::Rollback => "qmrollback",
            Self::Destroy => "qmdestroy",
            Self::Resize => "resize",
            Self::Resume => "qmresume",
        }
    }

    pub fn phase(phase: &str) -> Option<Self> {
        match phase {
            "cloning" => Some(Self::Clone),
            "starting" => Some(Self::Start),
            "stopping" | "export-stopping" => Some(Self::Shutdown),
            "resetting" => Some(Self::Rollback),
            "deleting" => Some(Self::Destroy),
            "resizing" => Some(Self::Resize),
            _ => None,
        }
    }
}

pub fn task_matches(
    task: &str,
    node: &str,
    guest_id: &str,
    token_id: &str,
    kind: TaskKind,
) -> bool {
    let parts: Vec<_> = task.split(':').collect();
    matches!(parts.as_slice(), ["UPID", n, _, _, _, k, guest, actor, _]
        if *n == node && *k == kind.api() && *guest == guest_id && *actor == token_id)
}

#[derive(Clone, Debug)]
pub struct GuestPin {
    pub name: String,
    pub uuid: String,
    pub volume: String,
}

impl GuestPin {
    pub fn new(
        name: impl Into<String>,
        uuid: impl Into<String>,
        volume: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            uuid: uuid.into(),
            volume: volume.into(),
        }
    }

    pub fn verify(&self, config: &Value) -> Result<(), Rejected> {
        let expected = format!("uuid={}", self.uuid);
        if config["name"] != self.name
            || truthy(&config["template"])
            || !config["smbios1"]
                .as_str()
                .is_some_and(|value| value.split(',').any(|item| item == expected))
            || first_property(&config["scsi0"]) != Some(&self.volume)
        {
            return Err(identity_changed(
                "The slot name, hardware UUID or boot volume changed.",
            ));
        }
        if truthy(&config["lock"]) {
            return Err(Rejected::new(
                "slot_locked",
                "Proxmox reports an in-flight task on the pilot slot.",
                409,
            ));
        }
        Ok(())
    }

    pub fn generation<'a>(&self, config: &'a Value) -> Result<&'a str, Rejected> {
        self.verify(config)?;
        uuid_field(&config["vmgenid"])
            .ok_or_else(|| identity_changed("A provider VM generation ID is required."))
    }
}

pub(crate) fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

pub(crate) fn uuid_field(value: &Value) -> Option<&str> {
    let raw = value.as_str()?;
    let parsed = Uuid::parse_str(raw).ok()?;
    (!parsed.is_nil() && parsed.hyphenated().to_string() == raw).then_some(raw)
}

pub(crate) fn first_property(value: &Value) -> Option<&str> {
    value.as_str()?.split(',').next()
}

pub(crate) fn identity_changed(reason: &str) -> Rejected {
    Rejected::new("slot_identity_changed", reason, 409)
}

fn invalid_config() -> Rejected {
    Rejected::new(
        "provider_configuration_invalid",
        "Verify the pinned Proxmox configuration and token files.",
        503,
    )
}

pub(crate) fn config_text<'a>(config: &'a Value, key: &str) -> Result<&'a str, Rejected> {
    config[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_config)
}

pub(crate) fn api_id(value: &Value) -> Option<String> {
    let value = value.as_u64().or_else(|| value.as_str()?.parse().ok())?;
    (value > 0 && value <= u32::MAX as u64).then(|| value.to_string())
}

pub(crate) fn identifier(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

pub(crate) fn escape(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

pub(crate) fn query(fields: &[(&str, &str)]) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields.iter().copied())
        .finish()
}

fn credential(path: &str) -> Result<(String, header::HeaderValue), Rejected> {
    let secret = fs::read_to_string(path).map_err(|_| invalid_config())?;
    let token = secret.trim();
    let (identity, data) = token.split_once('=').ok_or_else(invalid_config)?;
    if !identity.contains('!')
        || data.is_empty()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return Err(invalid_config());
    }
    let mut header = header::HeaderValue::from_str(&format!("PVEAPIToken={token}"))
        .map_err(|_| invalid_config())?;
    header.set_sensitive(true);
    Ok((identity.to_owned(), header))
}

#[derive(Clone)]
pub(crate) struct Proxmox {
    client: Client,
    pub base: String,
    pub node: String,
    pub guest_id: String,
    pub identity: String,
    write_token: header::HeaderValue,
    audit_token: header::HeaderValue,
}

impl Proxmox {
    pub fn open(config: &Value, capacity: &Value) -> Result<Self, Rejected> {
        let url = Url::parse(config_text(config, "url")?).map_err(|_| invalid_config())?;
        if url.scheme() != "https"
            || url.host().is_none()
            || url.path() != "/"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid_config());
        }
        let node = config_text(config, "node")?;
        if !identifier(node) {
            return Err(invalid_config());
        }
        let guest_id = api_id(&config["guest_id"]).ok_or_else(invalid_config)?;
        let (identity, write_token) = credential(config_text(config, "token_file")?)?;
        let (audit_identity, audit_token) = credential(config_text(capacity, "token_file")?)?;
        if identity == audit_identity {
            return Err(invalid_config());
        }
        let pem = fs::read(config_text(config, "ca_file")?).map_err(|_| invalid_config())?;
        let anchors = Certificate::from_pem_bundle(&pem).map_err(|_| invalid_config())?;
        if anchors.is_empty() {
            return Err(invalid_config());
        }
        let client = Client::builder()
            .tls_backend_native()
            .tls_certs_only(anchors)
            .https_only(true)
            .no_proxy()
            .redirect(redirect::Policy::none())
            .retry(retry::never())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| unavailable())?;
        Ok(Self {
            client,
            base: format!("{}api2/json", url.as_str()),
            node: node.to_owned(),
            guest_id,
            identity,
            write_token,
            audit_token,
        })
    }

    fn request(
        &self,
        method: Method,
        route: &str,
        form: Option<&[(&str, &str)]>,
        audit: bool,
    ) -> Result<Result<Value, u16>, Rejected> {
        let token = if audit {
            &self.audit_token
        } else {
            &self.write_token
        };
        let mut pending = self
            .client
            .request(method, format!("{}{route}", self.base))
            .header(header::AUTHORIZATION, token);
        if let Some(fields) = form {
            pending = pending
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(query(fields));
        }
        let response = pending.send().map_err(|error| {
            tracing::warn!(
                timeout = error.is_timeout(),
                "Proxmox transport did not complete"
            );
            unavailable()
        })?;
        if !response.status().is_success() {
            return Ok(Err(response.status().as_u16()));
        }
        let max = 2 * 1024 * 1024;
        let mut bytes = Vec::new();
        response
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if bytes.len() as u64 > max {
            return Err(unavailable());
        }
        let result: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        Ok(Ok(result.get("data").cloned().ok_or_else(unavailable)?))
    }

    fn accepted(response: Result<Value, u16>) -> Result<Value, Rejected> {
        response.map_err(|status| {
            tracing::warn!(status, "Proxmox rejected a scoped provider request");
            unavailable()
        })
    }

    pub fn get(&self, route: &str) -> Result<Value, Rejected> {
        Self::accepted(self.request(Method::GET, route, None, false)?)
    }

    pub fn audit(&self, route: &str) -> Result<Value, Rejected> {
        Self::accepted(self.request(Method::GET, route, None, true)?)
    }

    pub fn modify(
        &self,
        method: Method,
        route: &str,
        form: Option<&[(&str, &str)]>,
    ) -> Result<Value, Rejected> {
        Self::accepted(self.request(method, route, form, false)?)
    }

    pub fn guest_route(&self) -> String {
        format!("/nodes/{}/qemu/{}", self.node, self.guest_id)
    }

    pub fn audit_tasks(
        &self,
        guest: &str,
        kind: TaskKind,
        limit: &str,
    ) -> Result<Vec<Value>, Rejected> {
        let node = format!("/nodes/{}", self.node);
        let permissions = self.audit(&format!(
            "/access/permissions?{}",
            query(&[("path", &node)])
        ))?;
        if permissions[&node].get("Sys.Audit").is_none() {
            return Err(Rejected::new(
                "incomplete_task_history",
                "Read-only node Sys.Audit is required to see every task owner.",
                409,
            ));
        }
        as_list(self.audit(&self.task_query(guest, kind, limit))?)
    }

    fn task_query(&self, guest: &str, kind: TaskKind, limit: &str) -> String {
        format!(
            "/nodes/{}/tasks?{}",
            self.node,
            query(&[
                ("source", "all"),
                ("vmid", guest),
                ("typefilter", kind.api()),
                ("limit", limit)
            ])
        )
    }

    pub fn recent_tasks(&self, guest: &str, kind: TaskKind) -> Result<BTreeSet<String>, Rejected> {
        let rows = as_list(self.get(&self.task_query(guest, kind, "50"))?)?;
        Ok(rows
            .iter()
            .filter_map(|task| task["upid"].as_str())
            .filter(|task| task_matches(task, &self.node, guest, &self.identity, kind))
            .map(str::to_owned)
            .collect())
    }

    pub fn task(&self, id: &str) -> Result<bool, Rejected> {
        let route = format!("/nodes/{}/tasks/{}/status", self.node, escape(id));
        let response = self.get(&route)?;
        if response["status"] == "running" {
            return Ok(false);
        }
        if response["exitstatus"] == "OK" {
            return Ok(true);
        }
        Err(Rejected::new(
            "provider_task_failed",
            "The recorded provider task failed and needs operator review.",
            409,
        ))
    }

    pub fn clean_rollback(&self, rollback: &str) -> Result<(), Rejected> {
        let invalid = || {
            Rejected::new(
                "slot_reconciliation_required",
                "Attest the clean rollback of the stopped slot.",
                409,
            )
        };
        if !task_matches(
            rollback,
            &self.node,
            &self.guest_id,
            &self.identity,
            TaskKind::Rollback,
        ) {
            return Err(invalid());
        }
        let stamp = rollback
            .split(':')
            .nth(4)
            .and_then(|value| i64::from_str_radix(value, 16).ok())
            .ok_or_else(invalid)?;
        let rolls = self.audit_tasks(&self.guest_id, TaskKind::Rollback, "1")?;
        if !rolls
            .first()
            .is_some_and(|entry| entry["upid"] == rollback && entry["status"] == "OK")
        {
            return Err(Rejected::new(
                "slot_reconciliation_required",
                "Clean rollback does not match provider history.",
                409,
            ));
        }
        for kind in [TaskKind::Start, TaskKind::Resume] {
            let recent = self.audit_tasks(&self.guest_id, kind, "1")?;
            if let Some(entry) = recent.first() {
                if !entry["starttime"]
                    .as_i64()
                    .is_some_and(|start| start < stamp)
                {
                    return Err(Rejected::new(
                        "slot_reconciliation_required",
                        "Guest ran since its attested rollback.",
                        409,
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn verify_config(&self, pin: &GuestPin) -> Result<Value, Rejected> {
        let config = self.get(&format!("{}/config", self.guest_route()))?;
        pin.verify(&config)?;
        Ok(config)
    }

    pub fn status(&self, pin: &GuestPin) -> Result<String, Rejected> {
        self.verify_config(pin)?;
        let current = self.get(&format!("{}/status/current", self.guest_route()))?;
        current["status"]
            .as_str()
            .filter(|status| matches!(*status, "running" | "stopped"))
            .map(str::to_owned)
            .ok_or_else(unavailable)
    }

    pub fn ipv4(&self) -> Result<Option<String>, Rejected> {
        let route = format!("{}/agent/network-get-interfaces", self.guest_route());
        let response = match self.request(Method::GET, &route, None, false)? {
            Err(500) => return Ok(None),
            value => Self::accepted(value)?,
        };
        let interfaces = response["result"].as_array().ok_or_else(unavailable)?;
        let ipv4 = interfaces
            .iter()
            .filter(|iface| iface["name"] != "lo")
            .filter_map(|iface| iface["ip-addresses"].as_array())
            .flatten()
            .find(|item| item["ip-address-type"] == "ipv4")
            .and_then(|item| item["ip-address"].as_str())
            .map(str::to_owned);
        Ok(ipv4)
    }

    pub fn host_key(&self) -> Result<Option<String>, Rejected> {
        let agent = format!("{}/agent/file-read?file=", self.guest_route());
        for path in [
            "/var/lib/cloud/instance/boot-finished",
            "/etc/ssh/ssh_host_ed25519_key.pub",
        ] {
            let result = match self.request(
                Method::GET,
                &format!("{agent}{}", escape(path)),
                None,
                false,
            )? {
                Err(500) => return Ok(None),
                value => Self::accepted(value)?,
            };
            if path.ends_with(".pub") {
                let key = result["content"]
                    .as_str()
                    .ok_or_else(unavailable)?
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                return public_key(&json!(key)).map(Some);
            }
        }
        unreachable!("host public key is the final path")
    }

    pub fn install_key(&self, pin: &GuestPin, key: &Value, lease: &str) -> Result<(), Rejected> {
        self.verify_config(pin)?;
        let key = public_key(key)?;
        let route = format!("{}/agent/file-write", self.guest_route());
        self.modify(
            Method::POST,
            &route,
            Some(&[
                ("file", "/etc/ssh/lease_authorized_keys"),
                ("content", &format!("{key} lease-{lease}\n")),
            ]),
        )?;
        Ok(())
    }
}

pub(crate) fn as_list(value: Value) -> Result<Vec<Value>, Rejected> {
    match value {
        Value::Array(rows) if rows.iter().all(Value::is_object) => Ok(rows),
        _ => Err(unavailable()),
    }
}
