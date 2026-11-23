use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    sync::watch,
    task::JoinHandle,
    time::{Instant, sleep, timeout_at},
};

use crate::{
    collector::Target,
    inventory::{Kind, fields, kind},
    metrics::Frames,
    unix_time,
};

const MAX_AGE: f64 = 900.0;
const FRAME_LIMIT: usize = 4 << 20;
const GUEST_TIMEOUT: Duration = Duration::from_secs(75);

#[derive(Clone, Default)]
pub(crate) struct Report {
    pub checked: Option<f64>,
    pub expires: Option<f64>,
    pub entries: Option<Vec<Value>>,
    pub hardware: Option<Value>,
    pub error: Option<String>,
}

impl Report {
    fn parse(value: &Value, now: f64) -> Result<Self, &'static str> {
        let hardware = verified_hardware(&value["hardware"], now);
        match value["status"].as_str() {
            Some("waiting" | "unavailable") => {
                let reason = value["warnings"]
                    .as_array()
                    .and_then(|warnings| warnings.first())
                    .and_then(Value::as_str)
                    .unwrap_or("Collector waiting");
                Ok(Self {
                    checked: Some(now),
                    hardware,
                    error: Some(reason.chars().take(200).collect()),
                    ..Self::default()
                })
            }
            Some("available") => {
                let checked = value["checked"]
                    .as_f64()
                    .ok_or("Missing observation time")?;
                let expires = value["expires"]
                    .as_f64()
                    .ok_or("Missing observation expiry")?;
                if checked > now + 5.0 || expires < checked {
                    return Err("Invalid observation lifetime");
                }
                let guests = value["guests"]
                    .as_array()
                    .ok_or("Missing complete guest list")?;
                let mut ids = HashSet::new();
                let mut entries = Vec::with_capacity(guests.len());
                for guest in guests {
                    let id = guest["id"].as_str().ok_or("Invalid guest identity")?;
                    if !ids.insert(id) || !guest["state"].is_string() {
                        return Err("Duplicate guest identity or invalid state");
                    }
                    entries.push(fields(guest, &["id", "name", "type", "state", "template"]));
                }
                Ok(Self {
                    checked: Some(checked),
                    expires: Some(expires),
                    entries: Some(entries),
                    hardware,
                    error: None,
                })
            }
            _ => Err("Invalid guest inventory status"),
        }
    }

    pub fn fresh(&self, now: f64) -> bool {
        self.error.is_none()
            && self.entries.is_some()
            && self.checked.is_some_and(|checked| {
                checked <= now + 5.0
                    && self
                        .expires
                        .is_some_and(|expires| checked <= expires && now < self.deadline())
            })
    }

    pub fn deadline(&self) -> f64 {
        let maximum = self.checked.unwrap_or(0.0) + MAX_AGE;
        self.expires.map_or(maximum, |expires| maximum.min(expires))
    }
}

fn verified_hardware(value: &Value, now: f64) -> Option<Value> {
    if !matches!(
        value["os"].as_str(),
        Some("darwin" | "linux" | "windows" | "freebsd")
    ) || !matches!(
        value["architecture"].as_str(),
        Some("amd64" | "arm64" | "386" | "arm" | "riscv64" | "ppc64le" | "s390x")
    ) || !value["logical_cpus"]
        .as_u64()
        .is_some_and(|count| (1..=65536).contains(&count))
        || !value["memory_bytes"]
            .as_u64()
            .is_some_and(|bytes| (1..=(1 << 60)).contains(&bytes))
    {
        return None;
    }
    let checked = value["checked"]
        .as_f64()
        .filter(|checked| *checked > 0.0 && *checked <= now + 5.0)?;
    let mut hardware = fields(
        value,
        &[
            "os",
            "architecture",
            "logical_cpus",
            "memory_bytes",
            "checked",
        ],
    );
    hardware["expires"] = json!(checked + MAX_AGE);
    Some(hardware)
}

fn identifier(value: &Value) -> Option<String> {
    if value.is_number() {
        return Some(value.to_string());
    }
    value.as_str().map(str::to_owned)
}

pub(crate) fn binding(spec: &Value) -> (Option<String>, Option<String>) {
    let resources = &spec["resources"];
    let audit = &resources["network_authority_audit"];
    let binding = &resources["runtime_binding"];
    if binding["kind"] == "guest" {
        return (
            identifier(&binding["machine_id"]),
            identifier(&binding["guest_id"]),
        );
    }
    let guest = resources
        .get("guest_id")
        .or_else(|| resources.get("vmid"))
        .or_else(|| audit.get("guest_id"))
        .and_then(identifier)
        .or_else(|| identifier(&resources["vm_name"]))
        .or_else(|| identifier(&resources["deployment"]["vmid"]));
    let parent = identifier(&resources["parent_machine"])
        .filter(|parent| !parent.is_empty())
        .or_else(|| {
            guest.as_ref()?;
            Some(format!(
                "machine-{}",
                audit["host"]
                    .as_str()
                    .or_else(|| spec["host"].as_str())
                    .unwrap_or_default()
            ))
        });
    (parent, guest)
}

pub(crate) fn overlay(records: &mut [Value], reports: &HashMap<String, Report>, now: f64) {
    for record in records {
        if kind(record) != Kind::Machine {
            continue;
        }
        let (Some(parent), Some(guest)) = binding(&record["spec"]) else {
            continue;
        };
        let empty = Report::default();
        let report = reports.get(&parent).unwrap_or(&empty);
        let observed = report
            .entries
            .as_ref()
            .filter(|_| report.fresh(now))
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry["id"].as_str() == Some(guest.as_str()))
            })
            .and_then(|entry| entry["state"].as_str());
        let state = observed
            .filter(|state| matches!(*state, "running" | "stopped"))
            .unwrap_or("unknown");
        let evidence = if observed.is_some() {
            "Authenticated metrics agent guest inventory"
        } else {
            "Guest inventory unavailable, expired or guest missing"
        };
        record["spec"]["resources"]["runtime"] = json!({
            "state":state, "checked":report.checked, "expires":report.deadline(), "evidence":evidence,
        });
    }
}

pub(crate) struct GuestInventory {
    workers: Arc<Mutex<Fleet>>,
    private: PathBuf,
    changed: watch::Sender<u64>,
}

#[derive(Default)]
struct Fleet {
    sequence: u64,
    workers: HashMap<String, Worker>,
    reports: HashMap<String, Report>,
}

struct Worker {
    target: Target,
    generation: u64,
    task: JoinHandle<()>,
}

impl GuestInventory {
    pub fn new(private: PathBuf) -> Self {
        Self {
            workers: Arc::new(Mutex::new(Fleet::default())),
            private,
            changed: watch::Sender::new(0),
        }
    }

    pub fn updates(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub fn snapshot(&self) -> HashMap<String, Report> {
        self.workers
            .lock()
            .expect("guest inventory lock")
            .reports
            .clone()
    }

    pub fn configure(&self, configured: &HashMap<String, Target>, restart: bool) {
        let mut fleet = self.workers.lock().expect("guest inventory lock");
        let retired: Vec<_> = fleet
            .workers
            .iter()
            .filter_map(|(id, worker)| {
                (restart || configured.get(id) != Some(&worker.target)).then_some(id.clone())
            })
            .collect();
        for id in retired {
            if let Some(worker) = fleet.workers.remove(&id) {
                worker.task.abort();
            }
            fleet.reports.remove(&id);
        }
        for (id, target) in configured {
            if fleet.workers.contains_key(id) {
                continue;
            }
            fleet.sequence += 1;
            let generation = fleet.sequence;
            let observer = Observer {
                id: id.clone(),
                generation,
                fleet: Arc::downgrade(&self.workers),
                changed: self.changed.clone(),
            };
            let task = tokio::spawn(collect_guest_reports(
                target.clone(),
                self.private.clone(),
                observer,
            ));
            fleet.workers.insert(
                id.clone(),
                Worker {
                    target: target.clone(),
                    generation,
                    task,
                },
            );
        }
    }
}

impl Drop for GuestInventory {
    fn drop(&mut self) {
        let fleet = self.workers.lock().expect("guest inventory lock");
        for worker in fleet.workers.values() {
            worker.task.abort();
        }
    }
}

struct Observer {
    id: String,
    generation: u64,
    fleet: Weak<Mutex<Fleet>>,
    changed: watch::Sender<u64>,
}

impl Observer {
    fn publish(&self, report: Report) {
        let Some(fleet) = self.fleet.upgrade() else {
            return;
        };
        let mut fleet = fleet.lock().expect("guest inventory lock");
        if fleet
            .workers
            .get(&self.id)
            .is_some_and(|worker| worker.generation == self.generation)
        {
            fleet.reports.insert(self.id.clone(), report);
            self.changed.send_modify(|version| *version += 1);
        }
    }
}

async fn collect_guest_reports(target: Target, private: PathBuf, observer: Observer) {
    let mut retry = Duration::from_secs(1);
    loop {
        if let Err(reason) = guest_stream(&target, &private, &observer, &mut retry).await {
            tracing::warn!(reason, "Guest inventory stream unavailable");
            observer.publish(Report {
                checked: Some(unix_time()),
                error: Some(
                    "Inventory stream unavailable; collector upgrade or recovery required".into(),
                ),
                ..Report::default()
            });
        }
        sleep(retry).await;
        retry = (retry * 2).min(Duration::from_secs(300));
    }
}

async fn guest_stream(
    target: &Target,
    private: &std::path::Path,
    observer: &Observer,
    retry: &mut Duration,
) -> Result<(), &'static str> {
    let mut response = target
        .open(
            private,
            "/guests/stream",
            "application/x-ndjson",
            GUEST_TIMEOUT,
        )
        .await?;
    let mut frames = Frames::bounded(FRAME_LIMIT);
    let mut deadline = Instant::now() + GUEST_TIMEOUT;
    loop {
        let chunk = timeout_at(deadline, response.chunk())
            .await
            .map_err(|_| "Guest inventory report timed out")?
            .map_err(|_| "Guest inventory stream interrupted")?
            .ok_or("Guest inventory stream closed")?;
        frames.receive(&chunk, |frame| {
            observer.publish(Report::parse(&frame, unix_time())?);
            *retry = Duration::from_secs(1);
            deadline = Instant::now() + GUEST_TIMEOUT;
            Ok(())
        })?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn authoritative_guest_reports_require_complete_fresh_entries_and_verified_hardware() {
        let sample = json!({"status":"available","checked":100,"expires":1080,
            "guests":[{"id":"123","name":"test","type":"qemu","state":"running"}],
            "hardware":{"os":"linux","architecture":"amd64","logical_cpus":4,"memory_bytes":1234,"checked":100}});
        let report = Report::parse(&sample, 102.0).unwrap();
        assert!(report.fresh(102.0));
        assert!(!report.fresh(1000.0));
        assert_eq!(report.hardware.as_ref().unwrap()["logical_cpus"], 4);
        assert_eq!(report.entries.as_ref().unwrap()[0]["state"], "running");

        let mut duplicate = sample.clone();
        duplicate["guests"] = json!([sample["guests"][0], sample["guests"][0]]);
        assert!(Report::parse(&duplicate, 102.0).is_err());
        assert!(Report::parse(&sample, 90.0).is_err());
        let unavailable = Report::parse(&json!({"status":"waiting"}), 102.0).unwrap();
        assert!(!unavailable.fresh(102.0));
        assert!(unavailable.entries.is_none());
    }

    #[test]
    fn explicit_binding_precedes_legacy_host_and_guest_metadata() {
        assert_eq!(
            binding(&json!({"host":"secondary","resources":{
                "guest_id":55,"parent_machine":"old",
                "runtime_binding":{"kind":"guest","machine_id":"exact","guest_id":100}
            }})),
            (Some("exact".into()), Some("100".into()))
        );
        assert_eq!(
            binding(&json!({"host":"parent","resources":{"vmid":55}})),
            (Some("machine-parent".into()), Some("55".into()))
        );
    }
}
