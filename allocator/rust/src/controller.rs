use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    AcquireRequest, CheckpointStore, GuestScope, LeasePhase, Ledger, Lifecycle, Provider,
    ProviderKind, ReclaimAction, Rejected, Slot, TaskCredentials, lease_inventory,
    provider::unavailable,
    protocol::LINUX,
    rows::{
        available_receipt, invalid_data, lease, no_checkpoint, phase, sqlite_json, stored, text,
    },
};

pub(crate) const RETENTION: f64 = 30.0 * 86400.0;
pub(crate) const EXPORT_REVIEW: f64 = 86400.0;

type Generation = Box<dyn FnMut() -> Result<String, Rejected> + Send>;

pub struct Controller {
    ledger: Ledger<Generation>,
    provider: Provider,
    slots: BTreeMap<String, Slot>,
    dependencies: Value,
    pub(crate) reclaim: bool,
    pub(crate) grace: f64,
    pub(crate) upload: Option<String>,
    pub(crate) recovery: Option<String>,
    pub(crate) alerts: bool,
    pub(crate) purge: bool,
    pub(crate) task_ttl: u64,
    pub(crate) principals: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Prepared {
    phase: String,
    generation: Option<String>,
    prior_tasks: Vec<String>,
    memory: i64,
    storage: i64,
}

pub(crate) fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock since epoch")
        .as_secs_f64()
}

fn conflict() -> Rejected {
    Rejected::new(
        "request_key_conflict",
        "This request key is already bound to different inputs.",
        409,
    )
}

fn replay_matches(request: &AcquireRequest, row: &Value) -> Result<(), Rejected> {
    let point = stored(&row["resume_point"])?;
    if point["lease"] != json!(request.resume_from)
        || point["requested_checkpoint"] != json!(request.checkpoint_id)
    {
        return Err(Rejected::new(
            "request_key_conflict",
            "This request key identifies another recovery source.",
            409,
        ));
    }
    let requirements = if request.resume_from.is_some() && !request.requirements_explicit {
        &point["requirements"]
    } else {
        &request.requirements
    };
    if request
        .recipe
        .as_deref()
        .is_some_and(|recipe| row["recipe"] != recipe)
        || &stored(&row["requirements"])? != requirements
        || row["task"] != request.task
        || row["initial_seconds"] != request.seconds
        || row["ssh_key"] != request.ssh_key
        || (request.reclaim_explicit && enrolled(row) != request.reclaim)
    {
        return Err(conflict());
    }
    Ok(())
}

fn enrolled(row: &Value) -> bool {
    row["reclaim_on_expiry"]
        .as_i64()
        .is_some_and(|value| value != 0)
}

fn rollback_needed() -> Rejected {
    Rejected::new(
        "slot_reconciliation_required",
        "The enrolled allocation slot must match the ledger.",
        409,
    )
}

pub(crate) fn provider_shape(row: &Value, action: &str) -> Result<Value, Rejected> {
    Ok(
        json!({"action":action,"slot_id":text(row,"slot_id")?,"lease":text(row,"id")?,
        "phase":text(row,"phase")?,"pins":stored(&row["dynamic_guest"])?,"generation":row["generation"],
        "task":row["upstream"],"requirements":stored(&row["requirements"])?}),
    )
}

impl Controller {
    pub fn open(private: &Value, provider: Provider) -> Result<Self, Rejected> {
        let path = |key: &str| {
            private[key]
                .as_str()
                .filter(|path| !path.is_empty())
                .map(Path::new)
                .ok_or_else(|| {
                    Rejected::new(
                        "configuration_invalid",
                        "Missing allocator ledger configuration.",
                        503,
                    )
                })
        };
        let database = path("database")?;
        let witness = path("ledger_witness")?;
        let policy = &private["reclamation"];
        let recovery = private["recovery"]["endpoint"].as_str().map(str::to_owned);
        let reclaim = policy["verifier_token_file"].is_string();
        let purge = policy["purge_expired_checkpoints"] == true;
        let alerts = policy["alert_token_file"].is_string();
        let grace = policy
            .get("grace_seconds")
            .map_or(Some(7200), Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| {
                Rejected::new(
                    "configuration_invalid",
                    "Grace must be a positive duration.",
                    503,
                )
            })?;
        if (purge || alerts) && !reclaim
            || recovery.is_some() != private["recovery"]["worker_token_file"].is_string()
        {
            return Err(Rejected::new(
                "configuration_invalid",
                "Allocator recovery credentials are incomplete.",
                503,
            ));
        }
        let task_ttl = private
            .get("task_credential_max_seconds")
            .map_or(Some(86400), Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| {
                Rejected::new(
                    "configuration_invalid",
                    "Task credential lifetime is invalid.",
                    503,
                )
            })?;
        let principals = private["client_tokens"]
            .as_object()
            .map_or_else(BTreeSet::new, |map| map.keys().cloned().collect());
        let upload = recovery
            .clone()
            .filter(|_| private["recovery"]["accept_uploads"] == true);
        let dependencies = private
            .get("dependency_bindings")
            .cloned()
            .unwrap_or_else(|| json!({}));
        validate_dependencies(&dependencies)?;
        let verify = provider.clone();
        let generation: Generation = Box::new(move || verify.generation());
        let mut ledger = Ledger::open(database, witness, generation)?;
        ledger.migrate()?;
        let slots = provider.slots()?;
        let unbound = collect_unbound(ledger.connection(), &slots)?;
        for (id, slot) in unbound {
            if id != "primary" || slot.kind != ProviderKind::Fixed {
                provider.call(json!({"action":"enrollment-check","slot_id":id}))?;
            }
            ledger.mutate(|tx| {
                tx.execute(
                    "INSERT INTO slot_binding VALUES (?,?)",
                    params![id, slot.identity],
                )?;
                Ok(())
            })?;
        }
        Ok(Self {
            ledger,
            provider,
            slots,
            reclaim,
            grace: grace as f64,
            upload,
            recovery,
            alerts,
            purge,
            task_ttl,
            principals,
            dependencies,
        })
    }

    pub(crate) fn database(&self) -> &Connection {
        self.ledger.connection()
    }

    pub fn review(&self, before: Option<&str>, scope: &str) -> Result<Value, Rejected> {
        if scope == "inventory" {
            if before.is_some() {
                return Err(Rejected::new(
                    "invalid_cursor",
                    "Inventory cannot be paginated.",
                    400,
                ));
            }
            let dynamic = self
                .slots
                .iter()
                .filter(|(_, slot)| slot.kind == ProviderKind::Dynamic)
                .map(|(id, slot)| {
                    (
                        id.clone(),
                        GuestScope {
                            node: slot.node.clone(),
                            guest_id: slot.guest_id.clone(),
                        },
                    )
                })
                .collect::<HashMap<_, _>>();
            return lease_inventory(self.database(), &dynamic);
        }
        // A read transaction keeps paging, counts and receipt metadata consistent.
        let read = self.database().unchecked_transaction()?;
        let result = crate::review::review_snapshot(&read, before, scope, now())?;
        read.commit()?;
        Ok(result)
    }

    pub(crate) fn slot(&self, row: &Value) -> Result<&Slot, Rejected> {
        self.slots.get(text(row, "slot_id")?).ok_or_else(|| {
            Rejected::new(
                "slot_unavailable",
                "This lease has no configured slot binding.",
                409,
            )
        })
    }

    pub fn task_token(&self, secret: &str) -> Result<Option<String>, Rejected> {
        crate::credentials::task_owner(self.database(), secret, now())
    }

    pub fn issue(&mut self, issuer: &str, data: &Value) -> Result<Value, Rejected> {
        TaskCredentials::new(&mut self.ledger, self.task_ttl)?.issue(issuer, data, now())
    }

    pub fn capabilities(&self) -> Result<Value, Rejected> {
        let names = self
            .dependencies
            .as_object()
            .expect("validated dependency bindings")
            .keys()
            .collect::<Vec<_>>();
        let busy = self.busy()?;
        let mut slots = Vec::new();
        for (id, slot) in &self.slots {
            let observation = self
                .provider
                .call(json!({"action":"capabilities","slot_id":id}));
            match observation {
                Ok(mut value) if value.is_object() => {
                    value["dependencies"] = json!(names);
                    slots.push(json!({"id":id,"recipe":slot.recipe,"capabilities":value,
                        "admission":if busy.contains(id){"reserved"}else{"assess_at_acquire"}}));
                }
                Ok(_) | Err(_) => slots.push(json!({"id":id,"admission":"unverified"})),
            }
        }
        let recipes = self
            .slots
            .values()
            .map(|slot| &slot.recipe)
            .collect::<BTreeSet<_>>();
        let mut response = json!({"api_version":1,"recipe":LINUX,"recipes":recipes,"observed_at":now(),"slots":slots});
        if slots.len() == 1 {
            if slots[0]["capabilities"].is_null() {
                return Err(Rejected::new(
                    "capacity_unverified",
                    "The slot could not be verified.",
                    503,
                ));
            }
            response["capabilities"] = slots[0]["capabilities"].clone();
        }
        Ok(response)
    }

    fn busy(&self) -> Result<HashSet<String>, Rejected> {
        let mut query = self
            .database()
            .prepare("SELECT slot_id FROM lease WHERE phase!='released'")?;
        Ok(query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?)
    }

    pub(crate) fn totals(&self, slot: &str) -> Result<(i64, i64), Rejected> {
        Ok(self.database().query_row(
            "SELECT coalesce(sum(memory_mib),0),coalesce(sum(storage_mib),0)
            FROM capacity_reservation WHERE slot_id=? AND released_at IS NULL",
            [slot],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    fn select(&self, recipe: &str, requirements: &Value) -> Result<(String, Prepared), Rejected> {
        let busy = self.busy()?;
        let mut choices = Vec::new();
        let mut reasons = BTreeMap::new();
        for (id, slot) in &self.slots {
            if recipe != slot.recipe {
                continue;
            }
            if busy.contains(id) {
                reasons.insert(
                    id,
                    Rejected::new(
                        "capacity_exhausted",
                        "The slot already has an active lease.",
                        409,
                    ),
                );
                continue;
            }
            let previous: Option<String> = self
                .database()
                .query_row(
                    "SELECT clean_rollback FROM lease WHERE slot_id=?
                AND clean_rollback IS NOT NULL ORDER BY rowid DESC LIMIT 1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            let result = self
                .provider
                .call(
                    json!({"action":"prepare","slot_id":id,"requirements":requirements,
                "clean_rollback":previous,"reservations":self.totals(id)?}),
                )
                .and_then(|prepared| {
                    serde_json::from_value::<Prepared>(prepared).map_err(|_| unavailable())
                });
            match result {
                Ok(choice) => choices.push((id.clone(), choice)),
                Err(error) => {
                    reasons.insert(id, error);
                }
            }
        }
        if let Some(best) = choices
            .into_iter()
            .min_by_key(|(id, p)| (p.memory, p.storage, id.clone()))
        {
            return Ok(best);
        }
        if reasons.is_empty() {
            return Err(Rejected::new(
                "unsupported_recipe",
                "The requested recipe has no enrolled provider.",
                422,
            ));
        }
        if self.slots.len() == 1 {
            return Err(reasons.into_values().next().expect("a refused slot"));
        }
        let details = reasons
            .into_iter()
            .map(|(id, reason)| format!("{id}: {}", reason.code()))
            .collect::<Vec<_>>()
            .join(", ");
        Err(Rejected::new(
            "capacity_exhausted",
            format!("No eligible slot ({details})."),
            409,
        ))
    }

    pub fn acquire(&mut self, data: &Value, owner: &str) -> Result<Value, Rejected> {
        let request = AcquireRequest::parse(data, self.reclaim)?;
        let key = request.key(owner);
        let existed = self
            .database()
            .query_row(
                "SELECT * FROM lease WHERE request_key=?",
                [&key],
                sqlite_json,
            )
            .optional()?;
        if let Some(row) = existed {
            replay_matches(&request, &row)?;
            return self.inspect(text(&row, "id")?, owner);
        }
        request.validate_new()?;
        let point = match request.resume_from.as_deref() {
            Some(source) => Some(self.matching_point(source, owner, &request)?),
            None => None,
        };
        let required = point
            .as_ref()
            .map_or(&request.requirements, |point| &point["requirements"]);
        let recipe = point
            .as_ref()
            .and_then(|point| point["recipe"].as_str())
            .unwrap_or_else(|| request.preferred_recipe());
        self.ledger.verify()?;
        let (slot, prepared) = self.select(recipe, required)?;
        let assigned = uuid::Uuid::new_v4().simple().to_string();
        let requested_at = now();
        let prior = serde_json::to_string(&prepared.prior_tasks).map_err(|_| invalid_data())?;
        self.ledger.mutate(|tx|{
            tx.execute("INSERT INTO lease (id,request_key,task,initial_seconds,phase,deadline,prior_tasks,owner,ssh_key,
                reclaim_on_expiry,requirements,resume_point,generation,slot_id,recipe) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![assigned,key,request.task,request.seconds as i64,prepared.phase,requested_at+request.seconds as f64,
                    prior,owner,request.ssh_key,1,required.to_string(),point.as_ref().map(Value::to_string),
                    prepared.generation,slot,recipe])?;
            if let Some(point)=&point{
                tx.execute("INSERT INTO restore_hold VALUES (?,?,?)",
                    params![assigned,text(point,"lease")?,point["checkpoint_id"].as_str()])?;
            }
            Ok(())
        })?;
        let row = lease(self.database(), &assigned, Some(owner))?;
        let row = self.dispatch_recorded(&row)?;
        self.public(&row, None)
    }

    fn matching_point(
        &self,
        source: &str,
        owner: &str,
        request: &AcquireRequest,
    ) -> Result<Value, Rejected> {
        let source_row = lease(self.database(), source, Some(owner))?;
        if source_row["ssh_key"] == request.ssh_key {
            return Err(Rejected::new(
                "new_ssh_key_required",
                "Generate a fresh SSH key for the replacement.",
                400,
            ));
        }
        if phase(&source_row)? != LeasePhase::Released {
            return Err(Rejected::new(
                "source_active",
                "Reconnect to the existing lease until reclamation finishes.",
                409,
            ));
        }
        let requirements = stored(&source_row["requirements"])?;
        if request.requirements_explicit && request.requirements != requirements {
            return Err(Rejected::new(
                "not_matching",
                "A matching replacement uses the predecessor requirements.",
                422,
            ));
        }
        if request
            .recipe
            .as_deref()
            .is_some_and(|recipe| source_row["recipe"] != recipe)
        {
            return Err(Rejected::new(
                "not_matching",
                "A matching replacement uses the predecessor recipe.",
                422,
            ));
        }
        let candidates = if let Some(key) = &request.checkpoint_id {
            vec![Some(key.clone())]
        } else {
            let mut query = self.database().prepare(
                "SELECT id FROM incremental_checkpoint WHERE lease_id=? AND receipt IS NOT NULL
                ORDER BY created_at DESC",
            )?;
            let keys = query
                .query_map([source], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            std::iter::once(None)
                .chain(keys.into_iter().map(Some))
                .collect()
        };
        for candidate in &candidates {
            match available_receipt(
                self.database(),
                &source_row,
                candidate.as_deref(),
                now(),
                false,
            ) {
                Ok(receipt) => {
                    return Ok(
                        json!({"lease":source,"requested_checkpoint":request.checkpoint_id,
                    "checkpoint_id":candidate,"sha256":receipt["sha256"],"verified_at":receipt["verified_at"],
                    "scope":"declared workspace","capture":if candidate.is_some(){"incremental"}else{"final"},
                    "recipe":source_row["recipe"],"requirements":requirements}),
                    );
                }
                Err(error) if error.code() == "checkpoint_unavailable" => continue,
                Err(error) => return Err(error),
            }
        }
        Err(no_checkpoint())
    }

    pub(crate) fn provider_call(&self, query: Value) -> Result<Value, Rejected> {
        self.provider.call(query)
    }

    pub(crate) fn call(&self, row: &Value, action: &str) -> Result<Value, Rejected> {
        self.provider.call(provider_shape(row, action)?)
    }

    pub(crate) fn ledger(&mut self) -> &mut Ledger<Generation> {
        &mut self.ledger
    }

    pub(crate) fn dispatch_recorded(&mut self, row: &Value) -> Result<Value, Rejected> {
        self.ledger.verify()?;
        let action = if phase(row)? == LeasePhase::Cloning {
            "clone"
        } else {
            "dispatch"
        };
        let mut input = provider_shape(row, action)?;
        let disposition = stored(&row["export_disposition"])?;
        input["discard"] = json!(disposition["mode"] == "discard");
        let result = self.provider.call(input)?;
        let task = result
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(unavailable)?;
        self.save_task(row, task)
    }

    fn save_task(&mut self, row: &Value, task: &str) -> Result<Value, Rejected> {
        let id = text(row, "id")?;
        let name = text(row, "phase")?;
        self.ledger.mutate(|tx| {
            let updated = tx.execute(
                "UPDATE lease SET upstream=? WHERE id=? AND phase=? AND upstream IS NULL",
                params![task, id, name],
            )?;
            if updated != 1 {
                return Err(rollback_needed());
            }
            lease(tx, id, None)
        })
    }

    pub(crate) fn dispatch(&mut self, row: &Value, next: &str) -> Result<Value, Rejected> {
        self.ledger.verify()?;
        let mut history = provider_shape(row, "task-history")?;
        history["phase"] = json!(next);
        let known = self.provider.call(history)?;
        if !known.is_array() {
            return Err(unavailable());
        }
        let id = text(row, "id")?;
        let before = text(row, "phase")?;
        let advanced=self.ledger.mutate(|tx|{
            let updated=tx.execute("UPDATE lease SET phase=?,upstream=NULL,address=NULL,prior_tasks=? WHERE id=? AND phase=?",
                params![next,known.to_string(),id,before])?;
            if updated!=1{return Err(rollback_needed());}
            lease(tx,id,None)
        })?;
        self.dispatch_recorded(&advanced)
    }

    fn recover_provider_task(&mut self, row: &Value) -> Result<Option<Value>, Rejected> {
        let prior = stored(&row["prior_tasks"])?;
        let Some(prior) = prior.as_array() else {
            return Ok(None);
        };
        let seen = self.call(row, "task-history")?;
        let seen = seen.as_array().ok_or_else(unavailable)?;
        let next = seen
            .iter()
            .filter_map(Value::as_str)
            .filter(|task| !prior.iter().any(|before| before == task))
            .collect::<BTreeSet<_>>();
        if next.len() != 1 {
            return Ok(None);
        }
        let task = next.into_iter().next().expect("one distinct task");
        Ok(Some(self.save_task(row, task)?))
    }

    pub fn inspect(&mut self, id: &str, owner: &str) -> Result<Value, Rejected> {
        let mut row = lease(self.database(), id, Some(owner))?;
        let begin = phase(&row)?;
        if begin == LeasePhase::Released {
            return self.public(&row, None);
        }
        if matches!(
            begin,
            LeasePhase::Cloning
                | LeasePhase::Resizing
                | LeasePhase::Starting
                | LeasePhase::Stopping
                | LeasePhase::ExportStopping
                | LeasePhase::Resetting
                | LeasePhase::Deleting
        ) {
            if row["upstream"].is_null() {
                let Some(recovered) = self.recover_provider_task(&row)? else {
                    return self.public(&row, Some("outcome-unknown"));
                };
                row = recovered;
            }
            if self.call(&row, "task-state")? != true {
                return self.public(&row, None);
            }
        }
        match begin {
            LeasePhase::Cloning => {
                let pins = self.call(&row, "describe-clone")?;
                let generation = text(&pins, "generation")?;
                let previous = stored(&row["dynamic_guest"])?;
                if !previous.is_null() && previous != pins {
                    return Err(Rejected::new(
                        "dynamic_identity_changed",
                        "Provisioned guest identity changed after the clone.",
                        409,
                    ));
                }
                if previous.is_null() {
                    row = self.ledger.mutate(|tx| {
                        tx.execute(
                            "UPDATE lease SET dynamic_guest=?,generation=? WHERE id=?",
                            params![pins.to_string(), generation, id],
                        )?;
                        lease(tx, id, Some(owner))
                    })?;
                }
                self.ledger.verify()?;
                let next = if self.slot(&row)?.kind == ProviderKind::Tart {
                    "starting"
                } else {
                    let shape = self.call(&row, "shape")?;
                    let (actual, desired) = shape["current_disk_mib"]
                        .as_i64()
                        .zip(shape["disk_mib"].as_i64())
                        .ok_or_else(unavailable)?;
                    if actual > desired {
                        return Err(Rejected::new(
                            "dynamic_shape_unverified",
                            "Clone exceeds the admitted disk size.",
                            409,
                        ));
                    }
                    if actual < desired {
                        "resizing"
                    } else {
                        "starting"
                    }
                };
                row = self.dispatch(&row, next)?;
            }
            LeasePhase::Resizing => {
                self.call(&row, "verify-shape")?;
                row = self.dispatch(&row, "starting")?;
            }
            LeasePhase::Starting => {
                let observation = self.call(&row, "guest-observation")?;
                if observation["status"] != "running" {
                    return Err(Rejected::new(
                        "unexpected_guest_state",
                        "The provider completed start but the guest is not running.",
                        409,
                    ));
                }
                let Some(address) = observation["address"].as_str() else {
                    return self.public(&row, Some("waiting-for-guest-ip"));
                };
                let mut input = provider_shape(&row, "install-key")?;
                input["ssh_key"] = json!(row["ssh_key"].as_str().ok_or_else(|| Rejected::new(
                    "missing_lease_key",
                    "The lease has no task SSH key.",
                    409
                ))?);
                self.ledger.verify()?;
                let installed = self.provider.call(input)?;
                let Some(host_key) = installed.as_str() else {
                    return self.public(&row, Some("waiting-for-guest-ip"));
                };
                row=self.ledger.mutate(|tx|{
                    tx.execute("UPDATE lease SET phase='ready',upstream=NULL,prior_tasks=NULL,address=?,dynamic_host_key=? WHERE id=?",
                        params![address,host_key,id])?;
                    lease(tx,id,Some(owner))
                })?;
            }
            LeasePhase::Stopping => {
                self.stopped(&row)?;
                row = self.dispatch(&row, "resetting")?;
            }
            LeasePhase::ExportStopping => {
                self.stopped(&row)?;
                row = self.request_export(&row)?;
            }
            LeasePhase::Resetting | LeasePhase::Deleting => {
                self.call(&row, "verify-after-cleanup")?;
                let rollback = if begin == LeasePhase::Resetting {
                    row["upstream"].as_str()
                } else {
                    None
                };
                row = self.finish_release(&row, rollback)?;
            }
            LeasePhase::Ready => {
                let observation = self.call(&row, "guest-observation")?;
                if observation["address"] != row["address"] {
                    row = self.ledger.mutate(|tx| {
                        tx.execute(
                            "UPDATE lease SET address=?,upstream=NULL,prior_tasks=NULL WHERE id=?",
                            params![observation["address"].as_str(), id],
                        )?;
                        lease(tx, id, Some(owner))
                    })?;
                }
                if observation["address"].is_null() {
                    return self.public(
                        &row,
                        Some(if observation["status"] == "running" {
                            "waiting-for-guest-ip"
                        } else {
                            "guest-stopped"
                        }),
                    );
                }
            }
            LeasePhase::AwaitingExport => {
                let mut input = provider_shape(&row, "guest-observation")?;
                input["include_address"] = json!(false);
                self.provider.call(input)?;
            }
            LeasePhase::Released => {}
        }
        let status = if phase(&row)? == LeasePhase::Ready
            && row["deadline"].as_f64().is_some_and(|until| now() >= until)
        {
            Some("overdue")
        } else {
            None
        };
        self.public(&row, status)
    }

    fn stopped(&self, row: &Value) -> Result<(), Rejected> {
        let mut input = provider_shape(row, "guest-observation")?;
        input["include_address"] = json!(false);
        if self.provider.call(input)?["status"] != "stopped" {
            return Err(Rejected::new(
                "unexpected_guest_state",
                "The guest did not stop. Preserve its disk.",
                409,
            ));
        }
        Ok(())
    }

    pub(crate) fn request_export(&mut self, row: &Value) -> Result<Value, Rejected> {
        self.ledger.verify()?;
        let source = self.call(row, "export-source")?;
        let id = text(row, "id")?;
        let challenge = uuid::Uuid::new_v4().simple().to_string();
        self.ledger.mutate(|tx|{
            tx.execute("UPDATE lease SET phase='awaiting-export',upstream=NULL,prior_tasks=NULL,address=NULL,
                export_challenge=?,storage_challenge=?,export_requested_at=?,export_start_tasks=?,export_last_attempt_at=NULL WHERE id=?",
                params![challenge,challenge,now(),source["start_tasks"].to_string(),id])?;
            lease(tx,id,None)
        })
    }

    fn finish_release(&mut self, row: &Value, rollback: Option<&str>) -> Result<Value, Rejected> {
        let id = text(row, "id")?;
        let released = now();
        let retention = released + RETENTION;
        let mut receipt = stored(&row["checkpoint"])?;
        if receipt.is_object() {
            receipt["retain_through"] = json!(retention);
        }
        let receipt = (!receipt.is_null()).then(|| receipt.to_string());
        self.ledger.mutate(|tx|{
            tx.execute("UPDATE lease SET phase='released',clean_rollback=?,upstream=NULL,address=NULL,prior_tasks=NULL,
                checkpoint=?,released_at=? WHERE id=?",params![rollback,receipt,released,id])?;
            tx.execute("UPDATE incremental_checkpoint SET receipt=json_set(receipt,'$.retain_through',?) WHERE lease_id=? AND receipt IS NOT NULL",
                params![retention,id])?;
            tx.execute("DELETE FROM restore_hold WHERE replacement=?",[id])?;
            lease(tx,id,None)
        })
    }

    pub fn list(&self, owner: &str) -> Result<Value, Rejected> {
        let mut statement = self.database().prepare(
            "SELECT id,task,phase,deadline,recipe,released_at,resume_point,checkpoint
            FROM lease WHERE owner=? ORDER BY phase='released',rowid DESC LIMIT 50",
        )?;
        let records = statement.query_map([owner], sqlite_json)?;
        let mut output = Vec::new();
        for row in records {
            let row = row?;
            let point = stored(&row["resume_point"])?;
            let checkpoint = stored(&row["checkpoint"])?;
            output.push(json!({"id":row["id"],"task":row["task"],"phase":row["phase"],"deadline":row["deadline"],"recipe":row["recipe"],
                "released_at":row["released_at"],"predecessor":point["lease"],"final_checkpoint":checkpoint.is_object()&&checkpoint["purged_at"].is_null()}));
        }
        Ok(json!({"leases":output}))
    }

    pub(crate) fn public(&mut self, row: &Value, status: Option<&str>) -> Result<Value, Rejected> {
        let mut result = row.as_object().ok_or_else(invalid_data)?.clone();
        for key in [
            "prior_tasks",
            "ssh_key",
            "request_key",
            "export_challenge",
            "storage_challenge",
            "export_start_tasks",
            "checkpoint",
            "dynamic_guest",
            "dynamic_host_key",
        ] {
            result.remove(key);
        }
        let mut result = Value::Object(result);
        let status = status.unwrap_or(text(row, "phase")?);
        let requirements = stored(&row["requirements"])?;
        if let Some(keys) = requirements["dependencies"]
            .as_array()
            .filter(|keys| !keys.is_empty() && status != "released")
        {
            let mut selected = BTreeMap::new();
            let mut missing = Vec::new();
            for key in keys.iter().filter_map(Value::as_str) {
                if let Some(dependency) = self.dependencies.get(key) {
                    selected.insert(key, dependency);
                } else {
                    missing.push(key);
                }
            }
            result["dependencies"] = json!(selected);
            if !missing.is_empty() {
                result["dependencies_unavailable"] = json!(missing);
            }
        }
        result["requirements"] = requirements;
        result["status"] = json!(status);
        result["resume_point"] = stored(&row["resume_point"])?;
        let id = text(row, "id")?;
        let incremental = CheckpointStore::new(&mut self.ledger).list(id)?;
        if incremental.as_array().is_some_and(|rows| !rows.is_empty()) {
            result["incremental_checkpoints"] = incremental;
        }
        let mut receipt = stored(&row["checkpoint"])?;
        if let Some(value) = receipt.as_object_mut() {
            value.remove("purge_claim");
            result["checkpoint"] = receipt;
        }
        let disposition = stored(&row["export_disposition"])?;
        if !disposition.is_null() {
            result["export_disposition"] = disposition;
        }
        if let Some(failed) = row["export_failed_at"].as_f64() {
            result["export_needs_decision"] =
                json!(row["export_disposition"].is_null() && now() >= failed + EXPORT_REVIEW);
        }
        if let Some(address) = row["address"].as_str().filter(|_| status != "released") {
            let slot = self.slot(row)?;
            let alias = slot.name.clone().unwrap_or_else(|| format!("lease-{id}"));
            let host_key = if slot.kind == ProviderKind::Fixed {
                slot.host_key.as_deref()
            } else {
                row["dynamic_host_key"].as_str()
            };
            result["ssh"] =
                json!({"host":address,"user":slot.user,"host_key_alias":alias,"host_key":host_key});
            if slot.kind == ProviderKind::Tart {
                result["workspace"] = json!("/Volumes/My Shared Files/workspace/project");
            }
        }
        Ok(result)
    }

    pub fn renew(&mut self, id: &str, owner: &str, request: &Value) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        let checkpoint = stored(&row["checkpoint"])?;
        let next =
            life(&row, &checkpoint)?.renew(request["seconds"].as_u64().unwrap_or(0), now())?;
        self.ledger.mutate(|tx| {
            tx.execute("UPDATE lease SET deadline=? WHERE id=?", params![next, id])?;
            Ok(())
        })?;
        self.inspect(id, owner)
    }

    pub fn release(&mut self, id: &str, owner: &str, data: &Value) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        let checkpoint = stored(&row["checkpoint"])?;
        let lifecycle = life(&row, &checkpoint)?;
        if lifecycle.phase == LeasePhase::Starting
            && lifecycle.reclaim_on_expiry
            && self.slot(&row)?.kind == ProviderKind::Tart
            && row["upstream"].is_string()
            && self.call(&row, "task-state")? == true
        {
            let mut query = provider_shape(&row, "guest-observation")?;
            query["include_address"] = json!(false);
            if self.provider.call(query)?["status"] == "stopped" {
                // A proven terminal start may exit before the task ever receives SSH.
                // Keep the usual hold and export policy while reclaiming that exact clone.
                let policy = Lifecycle { phase: LeasePhase::Ready, ..lifecycle };
                policy.release(data)?;
                let updated = self.request_export(&row)?;
                return self.public(&updated, None);
            }
        }
        let action = lifecycle.release(data)?;
        if action == ReclaimAction::Inspect {
            return self.inspect(id, owner);
        }
        let updated = self.stop(&row, action == ReclaimAction::StopForExport)?;
        self.public(&updated, None)
    }

    fn stop(&mut self, row: &Value, export: bool) -> Result<Value, Rejected> {
        let mut input = provider_shape(row, "guest-observation")?;
        input["include_address"] = json!(false);
        let observed = self.provider.call(input)?;
        if observed["status"] == "stopped" {
            if export {
                self.request_export(row)
            } else {
                self.dispatch(row, "resetting")
            }
        } else {
            self.dispatch(
                row,
                if export {
                    "export-stopping"
                } else {
                    "stopping"
                },
            )
        }
    }

    pub fn advance(&mut self) -> Result<(), Rejected> {
        if !self.reclaim {
            return Ok(());
        }
        let records = {
            let mut query = self
                .database()
                .prepare("SELECT * FROM lease WHERE phase!='released' AND reclaim_on_expiry=1")?;
            query
                .query_map([], sqlite_json)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut failed = None;
        for row in records {
            if let Err(error) = self.advance_one(&row) {
                failed.get_or_insert(error);
            }
        }
        if let Some(error) = failed {
            Err(error)
        } else {
            Ok(())
        }
    }

    fn advance_one(&mut self, row: &Value) -> Result<(), Rejected> {
        let checkpoint = stored(&row["checkpoint"])?;
        let id = text(row, "id")?;
        match life(row, &checkpoint)?.advance(now(), self.grace) {
            ReclaimAction::Inspect => {
                self.inspect(id, text(row, "owner")?)?;
            }
            ReclaimAction::StopForExport => {
                self.stop(row, true)?;
            }
            ReclaimAction::ReverifyExport => {
                self.ledger.mutate(|tx| {
                    tx.execute("UPDATE lease SET checkpoint=NULL WHERE id=?", [id])?;
                    Ok(())
                })?;
            }
            ReclaimAction::CleanupVerified => {
                self.check_source(row)?;
                let kind = &self.slot(row)?.kind;
                let next = if *kind == ProviderKind::Fixed {
                    "resetting"
                } else {
                    "deleting"
                };
                self.dispatch(row, next)?;
            }
            ReclaimAction::Wait | ReclaimAction::DiscardLegacyWorkspace => {}
        }
        Ok(())
    }

    pub(crate) fn check_source(&mut self, row: &Value) -> Result<Value, Rejected> {
        self.ledger.verify()?;
        let mut input = provider_shape(row, "export-source")?;
        input["start_tasks"] = stored(&row["export_start_tasks"])?;
        self.provider.call(input)
    }
}

fn life<'a>(row: &'a Value, checkpoint: &'a Value) -> Result<Lifecycle<'a>, Rejected> {
    Ok(Lifecycle {
        phase: phase(row)?,
        reclaim_on_expiry: enrolled(row),
        deadline: row["deadline"].as_f64().ok_or_else(invalid_data)?,
        owner_hold: row["hold_mode"].as_str(),
        reviewer_hold: row["reviewer_hold"].as_str(),
        checkpoint: (!checkpoint.is_null()).then_some(checkpoint),
    })
}

fn collect_unbound<'a>(
    db: &Connection,
    slots: &'a BTreeMap<String, Slot>,
) -> Result<Vec<(&'a String, &'a Slot)>, Rejected> {
    let mut active = db.prepare(
        "SELECT slot_id FROM lease WHERE phase!='released' UNION
                              SELECT slot_id FROM capacity_reservation WHERE released_at IS NULL",
    )?;
    for id in active.query_map([], |row| row.get::<_, String>(0))? {
        if !slots.contains_key(&id?) {
            return Err(rollback_needed());
        }
    }
    let mut missing = Vec::new();
    for (id, slot) in slots {
        let pin: Option<String> = db
            .query_row(
                "SELECT identity FROM slot_binding WHERE id=?",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        match pin.as_deref() {
            Some(pin) if pin != slot.identity => return Err(rollback_needed()),
            Some(_) => {}
            None => missing.push((id, slot)),
        }
    }
    Ok(missing)
}

fn validate_dependencies(value: &Value) -> Result<(), Rejected> {
    value
        .as_object()
        .filter(|items| {
            items.len() <= 32
                && items.iter().all(|(key, value)| {
                    let valid = crate::request::dependency(key)
                        && value.as_object().is_some_and(|item| item.len() == 2)
                        && value["service_id"]
                            .as_str()
                            .is_some_and(crate::request::dependency);
                    let url = value["endpoint"]
                        .as_str()
                        .filter(|url| {
                            url.is_ascii()
                                && !url
                                    .bytes()
                                    .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
                        })
                        .and_then(|url| url::Url::parse(url).ok());
                    valid
                        && url.is_some_and(|url| {
                            url.scheme() == "https"
                                && url.host().is_some()
                                && url.username().is_empty()
                                && url.password().is_none()
                                && url.query().is_none()
                                && url.fragment().is_none()
                        })
                })
        })
        .map(|_| ())
        .ok_or_else(|| {
            Rejected::new(
                "configuration_invalid",
                "Invalid shared dependency bindings.",
                503,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::replay_matches;
    use crate::{AcquireRequest, protocol::LINUX};
    use serde_json::json;

    #[test]
    fn retries_are_bound_to_exact_task_inputs_and_recovery_identity() {
        let mut request = AcquireRequest {
            task: "same".into(),
            request_key: "key".into(),
            seconds: 900,
            ssh_key: "public".into(),
            requirements: json!({"memory_mib":8192}),
            requirements_explicit: true,
            recipe: None,
            resume_from: None,
            checkpoint_id: None,
            reclaim: true,
            reclaim_explicit: false,
        };
        let row = json!({"recipe":LINUX,"task":"same","initial_seconds":900,"ssh_key":"public",
            "requirements":"{\"memory_mib\":8192}","reclaim_on_expiry":1});
        assert!(replay_matches(&request, &row).is_ok());
        request.seconds = 901;
        assert_eq!(
            replay_matches(&request, &row).unwrap_err().code(),
            "request_key_conflict"
        );
        request.seconds = 900;
        request.resume_from = Some("a".repeat(32));
        assert_eq!(
            replay_matches(&request, &row).unwrap_err().code(),
            "request_key_conflict"
        );
    }
}

pub fn enroll(config: &Path) -> Result<(), Rejected> {
    let invalid = || {
        Rejected::new(
            "configuration_invalid",
            "Missing allocator ledger configuration.",
            503,
        )
    };
    let settings = std::fs::read(config)
        .ok()
        .and_then(|body| serde_json::from_slice::<Value>(&body).ok())
        .ok_or_else(invalid)?;
    let database = settings["database"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(Path::new)
        .ok_or_else(invalid)?;
    let witness = settings["ledger_witness"]
        .as_str()
        .filter(|path| !path.is_empty())
        .map(Path::new)
        .ok_or_else(invalid)?;
    let provider = Provider::native(&settings)?;
    let slots = provider.slots()?;
    for id in slots.keys() {
        provider.call(json!({"action":"enrollment-check","slot_id":id}))?;
    }
    let bindings = slots
        .iter()
        .map(|(id, slot)| (id.as_str(), slot.identity.as_str()))
        .collect::<Vec<_>>();
    Ledger::enroll(database, witness, || provider.generation(), &bindings)?;
    Ok(())
}
