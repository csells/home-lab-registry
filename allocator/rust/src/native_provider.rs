use std::{
    collections::{BTreeMap, BTreeSet},
    net::Ipv4Addr,
};

use reqwest::Method;
use serde_json::{Value, json};

use crate::{
    ProviderKind, Rejected, Slot,
    capacity::{Capacity, Observation, Pool},
    provider::unavailable,
    proxmox::{GuestPin, Proxmox, TaskKind, api_id, config_text, escape, task_matches, uuid_field},
    proxmox_dynamic::{DynamicScope, Shape, legacy_json},
    protocol::{APPLE, LINUX, public_key},
    request::canonical_requirements,
    tart::{Tart, operation as tart_operation},
    tokens::hex,
};

pub(crate) struct NativeProvider {
    slots: BTreeMap<String, ProviderSlot>,
    controller_guest: String,
    dependencies: Value,
}

struct ProviderSlot {
    descriptor: Slot,
    worker: Worker,
}

enum Worker {
    Linux(Box<Linux>),
    Tart(Tart),
}

struct Linux {
    api: Proxmox,
    capacity: Capacity,
    mode: LinuxMode,
}

enum LinuxMode {
    Fixed {
        pin: GuestPin,
        snapshot: String,
        initial_rollback: Option<String>,
    },
    Dynamic(DynamicScope),
}

fn invalid() -> Rejected {
    Rejected::new(
        "provider_configuration_invalid",
        "Check the exact enrolled provider, controller and slot bindings.",
        503,
    )
}

fn unknown_action() -> Rejected {
    Rejected::new(
        "provider_operation_unknown",
        "Unknown provider operation.",
        400,
    )
}

fn unknown_outcome() -> Rejected {
    Rejected::new(
        "operation_outcome_unknown",
        "The recorded provider task cannot be attributed. The slot remains held.",
        409,
    )
}

fn missing_identity() -> Rejected {
    Rejected::new(
        "dynamic_outcome_unknown",
        "The clone has not established a verifiable identity.",
        409,
    )
}

fn different_generation() -> Rejected {
    Rejected::new(
        "slot_reconciliation_required",
        "Provider generation differs from the recorded lease. Hold for reconciliation.",
        409,
    )
}

fn guest_name(id: &str) -> String {
    format!("lease-{id}")
}

fn lease(request: &Value) -> Result<&str, Rejected> {
    request["lease"]
        .as_str()
        .filter(|id| hex(id, 32))
        .ok_or_else(|| Rejected::new("unknown_lease", "No lease has this identifier.", 404))
}

fn reservations(request: &Value) -> Result<[i64; 2], Rejected> {
    let values: [i64; 2] =
        serde_json::from_value(request["reservations"].clone()).map_err(|_| unavailable())?;
    if values.iter().any(|value| *value < 0) {
        return Err(unavailable());
    }
    Ok(values)
}

pub(crate) fn verify_requirements(
    requirements: &Value,
    offers: &Value,
    dependencies: &Value,
) -> Result<(), Rejected> {
    canonical_requirements(requirements)?;
    for (key, wanted) in requirements.as_object().expect("validated requirements") {
        if key == "dependencies" {
            let absent: Vec<_> = wanted
                .as_array()
                .expect("validated dependency list")
                .iter()
                .filter_map(Value::as_str)
                .filter(|name| dependencies.get(*name).is_none())
                .collect();
            if !absent.is_empty() {
                return Err(Rejected::new(
                    "dependency_unavailable",
                    format!("Unavailable shared dependencies: {}", absent.join(", ")),
                    422,
                ));
            }
            continue;
        }
        let actual = &offers[key];
        let supported = if let Some(wanted) = wanted.as_u64() {
            actual.as_u64().is_some_and(|actual| actual >= wanted)
        } else {
            actual == wanted
        };
        if !supported {
            let wanted = wanted
                .as_str()
                .map_or_else(|| wanted.to_string(), str::to_owned);
            let actual = actual
                .as_str()
                .map_or_else(|| actual.to_string(), str::to_owned);
            return Err(Rejected::new(
                "unsupported_capability",
                format!("Requested {key}={wanted}; the qualified slot provides {actual}."),
                422,
            ));
        }
    }
    Ok(())
}

impl NativeProvider {
    pub fn open(config: &Value) -> Result<Self, Rejected> {
        let mut definitions = BTreeMap::from([("primary".to_owned(), config)]);
        if let Some(extra) = config.get("additional_slots") {
            let extra = extra.as_object().ok_or_else(invalid)?;
            if extra.len() > 7 {
                return Err(invalid());
            }
            for (id, value) in extra {
                if id == "primary"
                    || id.len() > 32
                    || !id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    return Err(invalid());
                }
                definitions.insert(id.clone(), value);
            }
        }
        let mut slots = BTreeMap::new();
        let mut nodes = BTreeSet::new();
        for (id, definition) in definitions {
            let slot = ProviderSlot::open(definition)?;
            if !nodes.insert(slot.descriptor.node.clone()) {
                return Err(invalid());
            }
            slots.insert(id, slot);
        }
        if !matches!(slots["primary"].worker, Worker::Linux(_)) {
            return Err(invalid());
        }
        Ok(Self {
            slots,
            controller_guest: api_id(&config["controller_guest_id"]).ok_or_else(invalid)?,
            dependencies: config
                .get("dependency_bindings")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
    }

    pub fn call(&self, request: Value) -> Result<Value, Rejected> {
        let action = request["action"].as_str().ok_or_else(unknown_action)?;
        match action {
            "controller-generation" => {
                let Worker::Linux(host) = &self.slots["primary"].worker else {
                    return Err(invalid());
                };
                let config = host.api.audit(&format!(
                    "/nodes/{}/qemu/{}/config",
                    host.api.node, self.controller_guest
                ))?;
                Ok(json!(
                    uuid_field(&config["vmgenid"]).ok_or_else(different_generation)?
                ))
            }
            "slots" => Ok(json!(
                self.slots
                    .iter()
                    .map(|(name, slot)| (name, &slot.descriptor))
                    .collect::<BTreeMap<_, _>>()
            )),
            _ => {
                let slot = request["slot_id"]
                    .as_str()
                    .and_then(|id| self.slots.get(id))
                    .ok_or_else(|| {
                        Rejected::new(
                            "slot_unavailable",
                            "This lease has no configured slot binding.",
                            409,
                        )
                    })?;
                slot.call(action, &request, &self.dependencies)
            }
        }
    }
}

impl ProviderSlot {
    fn open(definition: &Value) -> Result<Self, Rejected> {
        let user = config_text(definition, "guest_user")?.to_owned();
        let (mut identity, descriptor, worker) = if let Some(config) = definition.get("tart") {
            let worker = Tart::open(config)?;
            (
                worker.identity(),
                Slot {
                    identity: String::new(),
                    recipe: APPLE.to_owned(),
                    user: user.clone(),
                    host_key: None,
                    name: None,
                    node: worker.node.clone(),
                    guest_id: "apple-workspace".to_owned(),
                    kind: ProviderKind::Tart,
                },
                Worker::Tart(worker),
            )
        } else {
            let config = &definition["proxmox"];
            let api = Proxmox::open(config, &definition["capacity"])?;
            let capacity = Capacity::open(&definition["capacity"], &api.guest_id)?;
            let (identity, mode, kind, host_key, name) =
                if let Some(settings) = definition.get("dynamic") {
                    let scope = DynamicScope::open(settings, &api, &capacity.storage)?;
                    (
                        scope.identity(&api),
                        LinuxMode::Dynamic(scope),
                        ProviderKind::Dynamic,
                        None,
                        None,
                    )
                } else {
                    let pin = GuestPin::new(
                        config_text(config, "guest_name")?,
                        config_text(config, "guest_uuid")?,
                        config_text(config, "guest_volume")?,
                    );
                    let snapshot = config_text(config, "snapshot")?.to_owned();
                    let initial_rollback = config["initial_rollback"].as_str().map(str::to_owned);
                    let key = public_key(&definition["guest_host_key"])?;
                    let identity = vec![
                        json!(api.base),
                        json!(api.node),
                        json!(api.guest_id),
                        json!(pin.uuid),
                        json!(pin.volume),
                        json!(pin.name),
                        json!(snapshot),
                        json!(api.identity),
                        json!(capacity.storage),
                    ];
                    let name = Some(pin.name.clone());
                    (
                        identity,
                        LinuxMode::Fixed {
                            pin,
                            snapshot,
                            initial_rollback,
                        },
                        ProviderKind::Fixed,
                        Some(key),
                        name,
                    )
                };
            let descriptor = Slot {
                identity: String::new(),
                recipe: LINUX.to_owned(),
                user: user.clone(),
                host_key,
                name,
                node: api.node.clone(),
                guest_id: api.guest_id.clone(),
                kind,
            };
            (
                identity,
                descriptor,
                Worker::Linux(Box::new(Linux {
                    api,
                    capacity,
                    mode,
                })),
            )
        };
        identity.push(json!(user));
        if descriptor.kind == ProviderKind::Fixed {
            identity.push(json!(descriptor.host_key));
        }
        Ok(Self {
            descriptor: Slot {
                identity: legacy_json(&json!(identity), true),
                ..descriptor
            },
            worker,
        })
    }

    fn offers(&self, shape: Shape) -> Value {
        let mut value = json!(shape);
        if self.descriptor.kind == ProviderKind::Tart {
            value["os"] = json!("macos");
            value["architecture"] = json!("arm64");
            value["distribution"] = json!("macos");
        } else {
            value["os"] = json!("linux");
            value["architecture"] = json!("amd64");
            value["distribution"] = json!("debian13");
        }
        value["gpu"] = json!("none");
        value
    }

    fn observe(
        &self,
        require: bool,
        allow_running: bool,
        shape: Option<Shape>,
        reserved: Option<[i64; 2]>,
    ) -> Result<Observation, Rejected> {
        match &self.worker {
            Worker::Tart(tart) => tart.capacity(allow_running, reserved.unwrap_or_default()),
            Worker::Linux(host) => {
                let mut result =
                    host.capacity
                        .observe(&host.api, host.pool(), require, allow_running, shape)?;
                if let Some(reservations) = reserved {
                    (result.memory, result.storage) = host
                        .capacity
                        .admit((result.memory, result.storage), reservations)?;
                }
                Ok(result)
            }
        }
    }

    fn call(&self, action: &str, request: &Value, dependencies: &Value) -> Result<Value, Rejected> {
        match action {
            "capabilities" => {
                if let Worker::Linux(host) = &self.worker {
                    host.verify_recipe()?
                }
                Ok(self.offers(self.observe(true, true, None, None)?.shape))
            }
            "enrollment-check" => {
                match &self.worker {
                    Worker::Linux(host) => host.idle(None)?,
                    Worker::Tart(tart) => tart.idle()?,
                }
                Ok(json!(true))
            }
            "reservation-capacity" => {
                let active = request["active"] == true;
                self.observe(active, active, None, Some(reservations(request)?))?;
                Ok(json!(true))
            }
            "prepare" => {
                let shape = match &self.worker {
                    Worker::Linux(host) => host.request_shape(&request["requirements"])?,
                    Worker::Tart(_) => None,
                };
                let measured = self.observe(true, false, shape, Some(reservations(request)?))?;
                verify_requirements(
                    &request["requirements"],
                    &self.offers(measured.shape),
                    dependencies,
                )?;
                let (phase, generation, tasks) = match &self.worker {
                    Worker::Linux(host) => host.prepare(request["clean_rollback"].as_str())?,
                    Worker::Tart(tart) => {
                        tart.idle()?;
                        ("cloning", None, tart.tasks("cloning", None)?)
                    }
                };
                Ok(
                    json!({"phase":phase,"generation":generation,"prior_tasks":tasks,
                    "memory":measured.memory,"storage":measured.storage}),
                )
            }
            "task-history"
            | "task-state"
            | "clone"
            | "describe-clone"
            | "verify-after-cleanup"
            | "dispatch"
            | "guest-observation"
            | "install-key"
            | "shape"
            | "verify-shape"
            | "export-source" => {
                let lease = lease(request)?;
                match &self.worker {
                    Worker::Linux(host) => {
                        let result = host.operation(action, lease, request)?;
                        if action == "install-key" && self.descriptor.kind == ProviderKind::Fixed {
                            return self
                                .descriptor
                                .host_key
                                .as_ref()
                                .map(|key| json!(key))
                                .ok_or_else(unavailable);
                        }
                        Ok(result)
                    }
                    Worker::Tart(tart) => apple(tart, action, lease, request),
                }
            }
            _ => Err(unknown_action()),
        }
    }
}

impl Linux {
    fn pool(&self) -> Pool<'_> {
        match &self.mode {
            LinuxMode::Fixed { pin, .. } => Pool::Fixed(pin),
            LinuxMode::Dynamic(scope) => Pool::Dynamic(scope),
        }
    }

    fn verify_recipe(&self) -> Result<(), Rejected> {
        match &self.mode {
            LinuxMode::Fixed { pin, .. } => self.api.verify_config(pin).map(|_| ()),
            LinuxMode::Dynamic(scope) => scope.template_config(&self.api).map(|_| ()),
        }
    }

    fn idle(&self, prior: Option<&str>) -> Result<(), Rejected> {
        match &self.mode {
            LinuxMode::Fixed {
                pin,
                initial_rollback,
                ..
            } => {
                if self.api.status(pin)? != "stopped" {
                    return Err(Rejected::new(
                        "slot_unavailable",
                        "The slot is running outside the controller.",
                        409,
                    ));
                }
                self.api
                    .clean_rollback(prior.or(initial_rollback.as_deref()).unwrap_or_default())
            }
            LinuxMode::Dynamic(scope) => {
                scope.template_config(&self.api)?;
                scope.verify_absent(&self.api)
            }
        }
    }

    fn request_shape(&self, requirements: &Value) -> Result<Option<Shape>, Rejected> {
        match &self.mode {
            LinuxMode::Fixed { .. } => Ok(None),
            LinuxMode::Dynamic(scope) => {
                if scope.pool.is_none() {
                    return Err(Rejected::new(
                        "dynamic_scope_unverified",
                        "The slot needs its dedicated allocation pool.",
                        409,
                    ));
                }
                Ok(Some(scope.default.for_request(requirements)?))
            }
        }
    }

    fn prepare(
        &self,
        prior: Option<&str>,
    ) -> Result<(&'static str, Option<String>, BTreeSet<String>), Rejected> {
        self.idle(prior)?;
        match &self.mode {
            LinuxMode::Fixed { pin, .. } => {
                let config = self.api.verify_config(pin)?;
                let generation = pin.generation(&config)?.to_owned();
                Ok((
                    "starting",
                    Some(generation),
                    self.api.recent_tasks(&self.api.guest_id, TaskKind::Start)?,
                ))
            }
            LinuxMode::Dynamic(scope) => {
                Ok(("cloning", None, scope.recent_clone_tasks(&self.api)?))
            }
        }
    }

    fn dynamic(&self) -> Result<&DynamicScope, Rejected> {
        match &self.mode {
            LinuxMode::Dynamic(scope) => Ok(scope),
            _ => Err(unknown_action()),
        }
    }

    fn pin(&self, lease: &str, request: &Value) -> Result<GuestPin, Rejected> {
        if let LinuxMode::Fixed { pin, .. } = &self.mode {
            return Ok(pin.clone());
        }
        let pins = &request["pins"];
        let uuid = uuid_field(&pins["uuid"]).ok_or_else(missing_identity)?;
        let volume = pins["volume"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(missing_identity)?;
        Ok(GuestPin::new(guest_name(lease), uuid, volume))
    }

    fn operation(&self, action: &str, id: &str, request: &Value) -> Result<Value, Rejected> {
        let api = &self.api;
        let phase = request["phase"].as_str().ok_or_else(unknown_action)?;
        let kind = TaskKind::phase(phase);
        let task = request["task"].as_str().unwrap_or_default();
        let pin = || self.pin(id, request);
        match action {
            "clone" => {
                let scope = self.dynamic()?;
                let task = scope.clone(api, id)?;
                if !task.as_str().is_some_and(|task| {
                    task_matches(
                        task,
                        &api.node,
                        &scope.template_id,
                        &api.identity,
                        TaskKind::Clone,
                    )
                }) {
                    return Err(unknown_outcome());
                }
                return Ok(task);
            }
            "describe-clone" => return self.dynamic()?.describe(api, id),
            "task-history" => {
                let kind = kind.ok_or_else(unknown_action)?;
                let tasks = if phase == "cloning" {
                    self.dynamic()?.recent_clone_tasks(api)?
                } else {
                    api.recent_tasks(&api.guest_id, kind)?
                };
                return Ok(json!(tasks));
            }
            "task-state" => {
                let kind = kind.ok_or_else(unknown_action)?;
                let guest = if phase == "cloning" {
                    &self.dynamic()?.template_id
                } else {
                    &api.guest_id
                };
                if !task_matches(task, &api.node, guest, &api.identity, kind) {
                    return Err(unknown_outcome());
                }
                return Ok(json!(api.task(task)?));
            }
            "verify-after-cleanup" if phase == "deleting" => {
                self.dynamic()?.verify_absent(api)?;
                return Ok(json!(true));
            }
            "verify-after-cleanup" if phase == "resetting" => {
                let pin = pin()?;
                if api.status(&pin)? != "stopped" {
                    return Err(unexpected_stop());
                }
                api.clean_rollback(task)?;
                let current = api.verify_config(&pin)?;
                if request["generation"] == pin.generation(&current)? {
                    return Err(Rejected::new(
                        "slot_reconciliation_required",
                        "Rollback has not advanced the slot generation.",
                        409,
                    ));
                }
                return Ok(json!(true));
            }
            _ => {}
        }
        let pin = pin()?;
        let current = api.verify_config(&pin)?;
        if request["generation"] != pin.generation(&current)? {
            return Err(different_generation());
        }
        let route = api.guest_route();
        match action {
            "dispatch" => {
                let task = match phase {
                    "starting" => {
                        api.modify(Method::POST, &format!("{route}/status/start"), Some(&[]))?
                    }
                    "stopping" | "export-stopping" => api.modify(
                        Method::POST,
                        &format!("{route}/status/shutdown"),
                        Some(&[("timeout", "20")]),
                    )?,
                    "resetting" => {
                        let LinuxMode::Fixed { snapshot, .. } = &self.mode else {
                            return Err(unknown_action());
                        };
                        api.modify(
                            Method::POST,
                            &format!("{route}/snapshot/{}/rollback", escape(snapshot)),
                            Some(&[("start", "0")]),
                        )?
                    }
                    "deleting" => {
                        if self.dynamic()?.describe(api, id)? != request["pins"] {
                            return Err(Rejected::new(
                                "dynamic_identity_changed",
                                "Guest or disk identity changed before deletion.",
                                409,
                            ));
                        }
                        if api.status(&pin)? != "stopped" {
                            return Err(unexpected_stop());
                        }
                        api.modify(Method::DELETE, &route, None)?
                    }
                    "resizing" => {
                        let scope = self.dynamic()?;
                        scope.resize(
                            api,
                            id,
                            &request["pins"],
                            scope.default.for_request(&request["requirements"])?,
                        )?
                    }
                    _ => return Err(unknown_action()),
                };
                let kind = kind.ok_or_else(unknown_action)?;
                if !task.as_str().is_some_and(|task| {
                    task_matches(task, &api.node, &api.guest_id, &api.identity, kind)
                }) {
                    return Err(unknown_outcome());
                }
                Ok(task)
            }
            "guest-observation" => {
                let status = api.status(&pin)?;
                let address = if status == "running" && request["include_address"] != false {
                    api.ipv4()?
                } else {
                    None
                };
                Ok(json!({"status":status,"address":address,"name":pin.name}))
            }
            "install-key" => {
                let key = match self.mode {
                    LinuxMode::Fixed { .. } => None,
                    LinuxMode::Dynamic(_) => {
                        let Some(key) = api.host_key()? else {
                            return Ok(Value::Null);
                        };
                        Some(key)
                    }
                };
                api.install_key(&pin, &request["ssh_key"], id)?;
                Ok(key.map_or_else(|| json!(true), |key| json!(key)))
            }
            "shape" | "verify-shape" => {
                let scope = self.dynamic()?;
                let shape = scope.default.for_request(&request["requirements"])?;
                if action == "verify-shape" {
                    scope.verify_shape(api, id, &request["pins"], shape)?;
                    Ok(json!(true))
                } else {
                    let actual = scope.apply_shape(api, id, &request["pins"], shape)?;
                    Ok(json!({"current_disk_mib":actual,"disk_mib":shape.disk_mib}))
                }
            }
            "export-source" => {
                if api.status(&pin)? != "stopped" {
                    return Err(export_changed(
                        "The guest is not stopped. Preserve its disk for review.",
                    ));
                }
                let tasks = api.audit_tasks(&api.guest_id, TaskKind::Start, "50")?;
                let mut starts = tasks
                    .iter()
                    .map(|row| row["upid"].as_str().ok_or_else(unavailable))
                    .collect::<Result<Vec<_>, _>>()?;
                starts.sort_unstable();
                let mut result = export_source(request, json!(starts))?;
                if let LinuxMode::Dynamic(scope) = &self.mode {
                    let shape = scope.default.for_request(&request["requirements"])?;
                    result["dynamic_source"] = json!([
                        pin.uuid,
                        request["generation"],
                        pin.volume,
                        shape.disk_mib.to_string()
                    ]);
                }
                Ok(result)
            }
            _ => Err(unknown_action()),
        }
    }
}

fn unexpected_stop() -> Rejected {
    Rejected::new(
        "unexpected_guest_state",
        "The stopped state required for cleanup is unverified.",
        409,
    )
}

fn export_changed(reason: &str) -> Rejected {
    Rejected::new("export_source_changed", reason, 409)
}

fn export_source(request: &Value, tasks: Value) -> Result<Value, Rejected> {
    if request
        .get("start_tasks")
        .is_some_and(|previous| previous != &tasks)
    {
        return Err(export_changed(
            "The guest start history changed. Preserve its disk for review.",
        ));
    }
    Ok(json!({"start_tasks":tasks}))
}

fn apple(tart: &Tart, action: &str, id: &str, request: &Value) -> Result<Value, Rejected> {
    let phase = request["phase"].as_str().ok_or_else(unknown_action)?;
    let task = request["task"].as_str().unwrap_or_default();
    match action {
        "clone" => {
            tart.idle()?;
            let task = tart.call("dispatch", json!({"lease":id,"operation":"clone"}))?;
            if !task
                .as_str()
                .is_some_and(|task| tart.task_matches(task, "cloning", id))
            {
                return Err(unknown_outcome());
            }
            return Ok(task);
        }
        "describe-clone" => return tart.describe(id),
        "task-history" => return Ok(json!(tart.tasks(phase, Some(id))?)),
        "task-state" => {
            if !tart.task_matches(task, phase, id) {
                return Err(unknown_outcome());
            }
            return Ok(json!(tart.task(task)?));
        }
        "verify-after-cleanup" if phase == "deleting" => {
            tart.idle()?;
            return Ok(json!(true));
        }
        _ => {}
    }
    let generation = request["generation"]
        .as_str()
        .ok_or_else(missing_identity)?;
    let pinned = request["pins"]["generation"]
        .as_str()
        .ok_or_else(missing_identity)?;
    if generation != pinned
        || tart.guest("describe", id, generation, json!({}))?["generation"] != generation
    {
        return Err(different_generation());
    }
    match action {
        "dispatch" => {
            let operation = tart_operation(phase)?;
            let mut payload = json!({"operation":operation});
            if operation == "delete" {
                if tart.describe(id)? != request["pins"] {
                    return Err(Rejected::new(
                        "dynamic_identity_changed",
                        "Guest or workspace identity changed before deletion.",
                        409,
                    ));
                }
                payload["discard"] = json!(request["discard"] == true);
            }
            let task = tart.guest("dispatch", id, generation, payload)?;
            if !task
                .as_str()
                .is_some_and(|task| tart.task_matches(task, phase, id))
            {
                return Err(unknown_outcome());
            }
            Ok(task)
        }
        "guest-observation" => {
            let status = tart.guest("status", id, generation, json!({}))?;
            let address = if status == "running" && request["include_address"] != false {
                let value = tart.guest("address", id, generation, json!({}))?;
                if value.is_null() {
                    None
                } else {
                    Some(
                        value
                            .as_str()
                            .and_then(|address| address.parse::<Ipv4Addr>().ok())
                            .ok_or_else(unavailable)?
                            .to_string(),
                    )
                }
            } else {
                None
            };
            Ok(json!({"status":status,"address":address,"name":guest_name(id)}))
        }
        "install-key" => {
            let Some(host_key) = tart.host_key(id, generation)? else {
                return Ok(Value::Null);
            };
            let key = public_key(&request["ssh_key"])?;
            tart.guest("guest-key", id, generation, json!({"key":key}))?;
            Ok(json!(host_key))
        }
        "export-source" => {
            if tart.guest("status", id, generation, json!({}))? != "stopped" {
                return Err(export_changed(
                    "The guest is not stopped. Preserve its workspace for review.",
                ));
            }
            let mut result = export_source(request, json!(tart.tasks("starting", Some(id))?))?;
            result["workspace_source"] = json!(generation);
            Ok(result)
        }
        _ => Err(unknown_action()),
    }
}
