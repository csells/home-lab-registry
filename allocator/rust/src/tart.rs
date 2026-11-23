use std::{collections::BTreeSet, fs, io::Read, time::Duration};

use reqwest::{blocking::Client, header, redirect, retry};
use serde_json::{Value, json};
use url::Url;

use crate::{
    Rejected,
    capacity::{MIB, Observation},
    proxmox::config_text,
    proxmox_dynamic::Shape,
    request::public_key,
    tokens::{hex, valid_token},
};

fn unreachable_worker() -> Rejected {
    Rejected::new(
        "apple_worker_unreachable",
        "The enrolled Tart worker did not respond.",
        503,
    )
}

fn invalid() -> Rejected {
    Rejected::new(
        "provider_configuration_invalid",
        "Configure the exact loopback Tart worker identity, token and recipe.",
        503,
    )
}

pub(crate) fn tunnel(raw: &str) -> Result<Url, Rejected> {
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none_or(|port| port == 0)
        || url.path() != "/"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    Ok(url)
}

fn worker_reply(identity: &str, mut response: Value) -> Result<Value, Rejected> {
    if response["worker"] != identity {
        return Err(Rejected::new(
            "apple_worker_identity",
            "The response did not match the enrolled Tart worker.",
            409,
        ));
    }
    response
        .as_object_mut()
        .and_then(|body| body.remove("value"))
        .ok_or_else(unreachable_worker)
}

fn invalid_task() -> Rejected {
    Rejected::new(
        "apple_operation_identity",
        "Invalid Tart operation reference.",
        409,
    )
}

fn task_identity(task: &str) -> Option<(&str, &str)> {
    let (operation, lease) = task.split_once(':')?;
    (matches!(operation, "clone" | "start" | "stop" | "delete") && hex(lease, 32))
        .then_some((operation, lease))
}

pub(crate) fn operation(phase: &str) -> Result<&'static str, Rejected> {
    match phase {
        "cloning" => Ok("clone"),
        "starting" => Ok("start"),
        "stopping" | "export-stopping" => Ok("stop"),
        "deleting" => Ok("delete"),
        _ => Err(invalid_task()),
    }
}

pub(crate) struct Tart {
    url: Url,
    pub node: String,
    auth: header::HeaderValue,
    client: Client,
    shape: Shape,
}

impl Tart {
    pub fn open(config: &Value) -> Result<Self, Rejected> {
        let url = tunnel(config_text(config, "url")?)?;
        let node = config_text(config, "identity")?;
        let secret =
            fs::read_to_string(config_text(config, "token_file")?).map_err(|_| invalid())?;
        let secret = secret.trim();
        let shape: Shape =
            serde_json::from_value(config["shape"].clone()).map_err(|_| invalid())?;
        if !hex(node, 32)
            || !valid_token(secret, 43, 128)
            || shape.memory_mib < 1
            || shape.disk_mib < 1
            || shape.vcpus < 1
        {
            return Err(invalid());
        }
        let mut auth =
            header::HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|_| invalid())?;
        auth.set_sensitive(true);
        let client = Client::builder()
            .no_proxy()
            .redirect(redirect::Policy::none())
            .retry(retry::never())
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| unreachable_worker())?;
        Ok(Self {
            url,
            node: node.to_owned(),
            auth,
            client,
            shape,
        })
    }

    pub fn identity(&self) -> Vec<Value> {
        vec![json!("tart"), json!(self.node), json!(self.shape)]
    }

    pub fn call(&self, action: &str, mut fields: Value) -> Result<Value, Rejected> {
        fields["action"] = json!(action);
        let response = self
            .client
            .post(self.url.clone())
            .header(header::AUTHORIZATION, &self.auth)
            .header(header::CONTENT_TYPE, "application/json")
            .body(fields.to_string())
            .send()
            .map_err(|_| unreachable_worker())?;
        let status = response.status();
        let max = if status.is_success() { 65536 } else { 4096 };
        let mut bytes = Vec::new();
        response
            .take(max + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unreachable_worker())?;
        if bytes.len() as u64 > max {
            return Err(unreachable_worker());
        }
        let reply: Value = serde_json::from_slice(&bytes).map_err(|_| unreachable_worker())?;
        if !status.is_success() {
            let code = reply["code"].as_str().ok_or_else(unreachable_worker)?;
            let reason = reply["reason"].as_str().ok_or_else(unreachable_worker)?;
            return Err(Rejected::new(code, reason, status.as_u16()));
        }
        worker_reply(&self.node, reply)
    }

    pub fn idle(&self) -> Result<(), Rejected> {
        if self.call("idle", json!({}))? != true {
            return Err(unreachable_worker());
        }
        Ok(())
    }

    pub fn capacity(
        &self,
        allow_running: bool,
        reservations: [i64; 2],
    ) -> Result<Observation, Rejected> {
        let reply = self.call("capacity", json!({"allow_running":allow_running}))?;
        if reply["shape"] != json!(self.shape) {
            return Err(Rejected::new(
                "apple_recipe_changed",
                "The worker recipe differs from the enrolled shape.",
                409,
            ));
        }
        let available = |field: &str, amount: i64, code: &str| -> Result<i64, Rejected> {
            let floor = reply[format!("floor_{field}")]
                .as_i64()
                .ok_or_else(unreachable_worker)?;
            let value = reply[field]
                .as_i64()
                .and_then(|value| value.checked_sub(amount.checked_mul(MIB)?))
                .ok_or_else(unreachable_worker)?;
            if value < floor {
                return Err(Rejected::new(
                    code,
                    "Apple host cannot preserve its configured headroom.",
                    409,
                ));
            }
            Ok(value)
        };
        Ok(Observation {
            shape: self.shape,
            memory: available(
                "memory_after_start",
                reservations[0],
                "host_memory_headroom",
            )?,
            storage: available(
                "storage_after_writes",
                reservations[1],
                "host_storage_headroom",
            )?,
        })
    }

    pub fn task_matches(&self, task: &str, phase: &str, lease: &str) -> bool {
        task_identity(task).is_some_and(|(actual, id)| {
            id == lease && operation(phase).is_ok_and(|phase| phase == actual)
        })
    }

    pub fn tasks(&self, phase: &str, lease: Option<&str>) -> Result<BTreeSet<String>, Rejected> {
        let operation = operation(phase)?;
        let result = self.call("tasks", json!({"operation":operation,"lease":lease}))?;
        result
            .as_array()
            .ok_or_else(unreachable_worker)?
            .iter()
            .map(|task| {
                let task = task.as_str().ok_or_else(invalid_task)?;
                let (kind, id) = task_identity(task).ok_or_else(invalid_task)?;
                if kind != operation || lease.is_some_and(|lease| lease != id) {
                    return Err(invalid_task());
                }
                Ok(task.to_owned())
            })
            .collect()
    }

    pub fn task(&self, id: &str) -> Result<bool, Rejected> {
        task_identity(id).ok_or_else(invalid_task)?;
        self.call("task", json!({"ref":id}))?
            .as_bool()
            .ok_or_else(unreachable_worker)
    }

    pub fn describe(&self, lease: &str) -> Result<Value, Rejected> {
        self.call("describe", json!({"lease":lease}))
    }

    pub fn guest(
        &self,
        action: &str,
        lease: &str,
        generation: &str,
        mut fields: Value,
    ) -> Result<Value, Rejected> {
        if !hex(lease, 32) || !hex(generation, 64) {
            return Err(Rejected::new(
                "apple_identity_changed",
                "The Tart guest or workspace identity differs from the enrolled lease.",
                409,
            ));
        }
        fields["generation"] = json!(generation);
        fields["lease"] = json!(lease);
        self.call(action, fields)
    }

    pub fn host_key(&self, lease: &str, generation: &str) -> Result<Option<String>, Rejected> {
        let key = self.guest("host-key", lease, generation, json!({}))?;
        if key.is_null() {
            Ok(None)
        } else {
            public_key(&key).map(Some)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tart_control_requires_the_exact_enrolled_loopback_tunnel() {
        assert!(tunnel("http://127.0.0.1:19874").is_ok());
        for denied in [
            "http://localhost:19874",
            "http://127.0.0.1:19874/?query",
            "http://172.16.2.3:19874",
            "http://operator@127.0.0.1:19874",
            "https://127.0.0.1:19874",
        ] {
            assert!(tunnel(denied).is_err(), "{denied}");
        }
    }

    #[test]
    fn independent_worker_identity_and_generation_are_required() {
        let worker = "1".repeat(32);
        assert_eq!(
            worker_reply(&worker, json!({"worker":worker,"value":true})).unwrap(),
            true
        );
        assert_eq!(
            worker_reply(&worker, json!({"worker":"different","value":true}))
                .unwrap_err()
                .code(),
            "apple_worker_identity"
        );
        assert!(task_identity("start:bad").is_none());
        let task = format!("clone:{worker}");
        assert_eq!(task_identity(&task), Some(("clone", worker.as_str())));
    }
}
