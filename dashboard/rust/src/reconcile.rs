use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::time::Instant;

use crate::{
    guests::{Report, binding},
    inventory::{Kind, kind},
    upstream::{lease_page, read_json},
};

type GuestIdentity = (String, String, String, String);

#[derive(Default)]
pub(crate) struct LeaseOwnership {
    config: PathBuf,
    nodes: HashMap<String, String>,
    previous: HashSet<(GuestIdentity, String)>,
    claims: HashMap<GuestIdentity, String>,
    unverified: HashSet<GuestIdentity>,
    retry_at: Option<Instant>,
    retry_wait: Option<Duration>,
}

impl LeaseOwnership {
    pub async fn from_config(config: &Path) -> Result<Self, &'static str> {
        if !config.is_file() {
            return Ok(Self::default());
        }
        #[derive(Deserialize)]
        struct Nodes {
            #[serde(default)]
            inventory_machines: HashMap<String, String>,
        }
        let settings: Nodes = read_json(config).await?;
        let values = settings.inventory_machines.values().collect::<HashSet<_>>();
        if values.len() != settings.inventory_machines.len()
            || values.iter().any(|name| !name.starts_with("machine-"))
        {
            return Err("Map each provider node to its distinct registry machine ID");
        }
        Ok(Self {
            config: config.to_path_buf(),
            nodes: settings.inventory_machines,
            ..Self::default()
        })
    }

    pub fn retry(&self) -> Option<Instant> {
        self.retry_at
    }

    pub async fn update(&mut self, reports: &HashMap<String, Report>, force: bool, now: f64) {
        if self.nodes.is_empty() {
            return;
        }
        let mut observed = HashSet::new();
        let machines: HashSet<_> = self.nodes.values().collect();
        for (machine, report) in reports {
            if !machines.contains(machine) || !report.fresh(now) {
                continue;
            }
            for guest in report.entries.as_deref().unwrap_or_default() {
                let identity = identity(machine, guest);
                if identity.3 == "qemu" && lease_name(&identity.2) {
                    observed.insert((identity, guest["state"].as_str().unwrap_or_default().into()));
                }
            }
        }
        let changed = self.previous != observed;
        if !force && !changed && self.retry_at.is_none_or(|target| Instant::now() < target) {
            return;
        }
        if changed || force {
            self.retry_wait = None;
        }
        self.previous = observed;
        self.claims.clear();
        self.unverified.clear();
        self.retry_at = None;
        if self.previous.is_empty() {
            return;
        }
        let keys = self.previous.iter().map(|(key, _)| key.clone()).collect();
        let result = lease_page(&self.config, "inventory", None)
            .await
            .and_then(|response| match_leases(&self.nodes, &keys, &response["guests"]));
        match result {
            Ok(claims) => {
                self.claims = claims;
                self.retry_wait = None;
            }
            Err(reason) => {
                tracing::warn!(reason, "Allocator inventory unavailable");
                self.unverified = keys;
                let delay = self.retry_wait.unwrap_or(Duration::from_secs(5));
                self.retry_at = Some(Instant::now() + delay);
                self.retry_wait = Some((delay * 2).min(Duration::from_secs(60)));
            }
        }
    }
}

fn lease_name(value: &str) -> bool {
    value.strip_prefix("lease-").is_some_and(|id| {
        id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

fn identity(machine: &str, guest: &Value) -> GuestIdentity {
    let field = |name: &str| guest[name].as_str().unwrap_or_default().to_owned();
    (
        machine.to_owned(),
        field("id"),
        field("name"),
        field("type"),
    )
}

fn match_leases(
    nodes: &HashMap<String, String>,
    seen: &HashSet<GuestIdentity>,
    guests: &Value,
) -> Result<HashMap<GuestIdentity, String>, &'static str> {
    let entries = guests
        .as_array()
        .filter(|items| items.len() <= 8)
        .ok_or("Invalid allocator inventory")?;
    let mut claims = HashMap::new();
    let mut unique = HashSet::new();
    for entry in entries {
        let id = entry["id"].as_str().ok_or("Invalid allocator guest ID")?;
        let lease = entry["lease"]
            .as_str()
            .ok_or("Missing allocator lease ID")?;
        let name = entry["name"]
            .as_str()
            .ok_or("Invalid allocator guest name")?;
        if !(3..=9).contains(&id.len())
            || id.starts_with('0')
            || !id.bytes().all(|b| b.is_ascii_digit())
            || entry["type"] != "qemu"
            || !lease_name(name)
            || name.strip_prefix("lease-") != Some(lease)
        {
            return Err("Invalid allocator guest identity");
        }
        let node = entry["node"].as_str().ok_or("Missing guest provider")?;
        let Some(machine) = nodes.get(node) else {
            continue;
        };
        let key = identity(machine, entry);
        if !unique.insert(key.clone()) {
            return Err("Duplicate allocator guest identity");
        }
        if seen.contains(&key) {
            claims.insert(key, lease.into());
        }
    }
    Ok(claims)
}

fn finding(code: &str, target: &str, message: &str) -> Value {
    let category = match code {
        "missing-guest" | "unregistered-guest" => "guest-mismatch",
        "collector-unverified" | "lease-ownership-unverified" | "deployment-unverified" => {
            "coverage"
        }
        _ => "record-review",
    };
    json!({"code":code,"category":category,"target":target,"message":message})
}

fn record_issues(
    record: &Value,
    now: f64,
    findings: &mut Vec<Value>,
    deployments: &mut Vec<Value>,
) {
    let target = record["id"].as_str().unwrap_or_default();
    let spec = &record["spec"];
    let resources = &spec["resources"];
    let runtime = &resources["runtime_binding"];
    if matches!(runtime["kind"].as_str(), Some("native" | "external")) {
        let value = &record["runtime_observation"];
        let checked = value["checked"].as_f64();
        let expires = value["expires"].as_f64();
        let fresh = checked.zip(expires).is_some_and(|(checked, expires)| {
            checked <= now + 5.0 && checked <= expires && now < expires.min(checked + 3600.0)
        });
        let state = value["state"]
            .as_str()
            .filter(|state| fresh && matches!(*state, "present" | "missing"))
            .unwrap_or("unverified");
        deployments.push(json!({"record_id":target,"kind":runtime["kind"],"state":state,"checked":checked,"expires":expires}));
        if state == "missing" {
            findings.push(finding(
                "deployment-missing",
                target,
                "Enrolled provider reports the bound deployment missing.",
            ));
        } else if state == "unverified"
            && runtime["provider"].as_str().is_some_and(|s| !s.is_empty())
            && runtime["instance"].as_str().is_some_and(|s| !s.is_empty())
        {
            findings.push(finding(
                "deployment-unverified",
                target,
                "No fresh verified observation for the bound deployment.",
            ));
        }
    } else if kind(record) == Kind::Service {
        let bound = binding(spec);
        if bound.0.is_none() || bound.1.is_none() {
            findings.push(finding(
                "deployment-unmapped",
                target,
                "Deployment has no structured runtime binding; presence is unverified.",
            ));
        }
    }
    if spec["owner"].is_null() || spec["owner"] == "" {
        findings.push(finding("owner-missing", target, "Record has no owner."));
    }
    let expiry = &resources["expires_at"];
    if !expiry.is_null() && expiry != "" {
        match expiry
            .as_str()
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
        {
            Some(stamp) if stamp.unix_timestamp() as f64 >= now => {}
            Some(_) => {
                let mut issue = finding(
                    "expired-lease",
                    target,
                    "Recorded lease expired; review or renew it. No automatic teardown.",
                );
                issue["expires_at"] = expiry.clone();
                findings.push(issue);
            }
            None => findings.push(finding(
                "invalid-lease",
                target,
                "Lease expiry must be an ISO timestamp with timezone.",
            )),
        }
    }
}

pub(crate) fn comparison(
    records: &[Value],
    reports: &HashMap<String, Report>,
    ownership: &LeaseOwnership,
    now: f64,
) -> Value {
    let mut active_hosts = Vec::new();
    let mut claims = HashMap::<(String, String), Vec<&Value>>::new();
    let mut findings = Vec::new();
    let mut host_reports = Vec::new();
    let mut deployments = Vec::new();
    for record in records.iter().filter(|record| {
        record["spec"]["lifecycle"].as_str().unwrap_or("active") == "active"
            && kind(record) != Kind::Archive
    }) {
        let (parent, guest) = binding(&record["spec"]);
        if guest.is_none() && kind(record) == Kind::Machine {
            active_hosts.push(record);
        }
        if let (Some(parent), Some(guest)) = (parent, guest) {
            claims.entry((parent, guest)).or_default().push(record);
        }
        record_issues(record, now, &mut findings, &mut deployments);
    }
    let claimed_hosts: HashSet<_> = claims.keys().map(|(host, _)| host.as_str()).collect();
    let mut expected = 0;
    let mut verified = 0;
    let mut not_applicable = 0;
    for host in &active_hosts {
        let id = host["id"].as_str().unwrap_or_default();
        let resources = &host["spec"]["resources"];
        let report = reports.get(id);
        if resources["metrics_stream"]["enabled"] != true && report.is_none() {
            let reason = resources["guest_inventory"]["not_applicable_reason"]
                .as_str()
                .map(str::trim)
                .filter(|reason| !reason.is_empty() && !claimed_hosts.contains(id));
            if let Some(reason) = reason {
                not_applicable += 1;
                host_reports.push(
                    json!({"machine_id":id,"status":"not_applicable","reason":reason,
                    "guests":[],"checked":null}),
                );
            } else {
                host_reports.push(json!({"machine_id":id,"status":"unconfigured",
                    "guests":[],"checked":null}));
            }
            continue;
        }
        expected += 1;
        let empty = Report::default();
        let report = report.unwrap_or(&empty);
        if !report.fresh(now) {
            findings.push(finding(
                "collector-unverified",
                id,
                report.error.as_deref().unwrap_or(
                    "Guest report absent, expired or invalid; missing guests cannot be inferred.",
                ),
            ));
            host_reports.push(json!({"machine_id":id,"status":"unverified","guests":[],"checked":report.checked,"hardware":report.hardware}));
            continue;
        }
        verified += 1;
        let entries = report.entries.as_deref().unwrap_or_default();
        let mut guests = Vec::new();
        let mut observed = HashSet::new();
        for guest in entries {
            let key = identity(id, guest);
            let id_and_guest = (id.into(), key.1.clone());
            observed.insert(key.1.clone());
            let linked = claims
                .get(&id_and_guest)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let lease = ownership.claims.get(&key);
            let explicit = &resources["templates"];
            let template = guest["template"] == true
                || explicit.get(&key.1).is_some()
                || explicit.as_array().is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry
                            .get("id")
                            .or_else(|| entry.get("vmid"))
                            .or_else(|| entry.get("name"))
                            .is_some_and(|value| {
                                value == &key.1 || value.is_number() && value.to_string() == key.1
                            })
                    })
                });
            let mut info = guest.clone();
            info["records"] = json!(linked.iter().map(|item| &item["id"]).collect::<Vec<_>>());
            info["template"] = json!(template);
            if let Some(lease) = lease {
                info["lease"] = json!(lease);
            }
            guests.push(info);
            if linked.is_empty() && !template && lease.is_none() {
                let mut issue = if ownership.unverified.contains(&key) {
                    finding(
                        "lease-ownership-unverified",
                        id,
                        "The allocator was unavailable to verify this guest.",
                    )
                } else {
                    let mut issue = finding(
                        "unregistered-guest",
                        id,
                        "Observed guest has no registered deployment.",
                    );
                    issue["name"] = json!(guest["name"].as_str().unwrap_or(&key.1));
                    issue
                };
                issue["guest_id"] = json!(key.1);
                findings.push(issue);
            }
        }
        host_reports.push(json!({"machine_id":id,"status":"verified","checked":report.checked,"expires":report.deadline(),
            "guests":guests,"hardware":report.hardware}));
        let mut host_claims: Vec<_> = claims
            .iter()
            .filter(|((parent, _), _)| parent == id)
            .collect();
        host_claims.sort_by_key(|((_, guest), _)| guest);
        for ((_, guest), linked) in host_claims {
            if !observed.contains(guest) {
                for record in linked {
                    let mut issue = finding(
                        "missing-guest",
                        record["id"].as_str().unwrap_or_default(),
                        "Registered guest absent from a fresh complete host inventory; verify before retirement.",
                    );
                    issue["machine_id"] = json!(id);
                    issue["guest_id"] = json!(guest);
                    findings.push(issue);
                }
            }
            let machines: Vec<_> = linked
                .iter()
                .filter(|record| kind(record) == Kind::Machine)
                .map(|record| &record["id"])
                .collect();
            if machines.len() > 1 {
                let mut issue = finding(
                    "duplicate-machine",
                    id,
                    "Multiple machine records claim the same guest.",
                );
                issue["guest_id"] = json!(guest);
                issue["records"] = json!(machines);
                findings.push(issue);
            }
        }
    }
    let mut all_claims: Vec<_> = claims.iter().collect();
    all_claims.sort_by_key(|(identity, _)| *identity);
    for ((parent, guest), linked) in all_claims {
        if active_hosts.iter().any(|record| record["id"] == *parent) {
            continue;
        }
        for record in linked {
            let mut issue = finding(
                "parent-unregistered",
                record["id"].as_str().unwrap_or_default(),
                "Deployment parent is not an active registered host.",
            );
            issue["machine_id"] = json!(parent);
            issue["guest_id"] = json!(guest);
            findings.push(issue);
        }
    }
    let coverage = if verified == 0 {
        "unverified"
    } else if verified < expected {
        "partial"
    } else {
        "verified"
    };
    json!({"checked":now,"coverage_status":coverage,"status":if findings.is_empty(){coverage}else{"attention"},
           "coverage":{"verified":verified,"expected":expected,
                       "unconfigured":active_hosts.len()-expected-not_applicable,"not_applicable":not_applicable},
           "findings":findings,"hosts":host_reports,"deployments":deployments,
           "scope":"Deployment presence from guest inventories and enrolled runtime sources. Application health is reported separately."})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guest_coverage_separates_devices_without_guests_from_missing_collectors() {
        let machine = |id, resources| {
            json!({"id":id,"spec":{
                "lifecycle":"active","capabilities":["machine"],"resources":resources
            }})
        };
        let exempt = json!({"guest_inventory":{"not_applicable_reason":"Desktop-only workload"}});
        let records = [
            machine("machine-desktop", exempt.clone()),
            machine("machine-unknown", json!({})),
            machine("machine-contradicted", exempt),
            json!({"id":"active-guest","spec":{"lifecycle":"active","resources":{
                "runtime_binding":{"kind":"guest","machine_id":"machine-contradicted","guest_id":"100"}
            }}}),
        ];
        let comparison = comparison(&records, &HashMap::new(), &LeaseOwnership::default(), 103.0);
        assert_eq!(comparison["coverage"]["not_applicable"], 1);
        assert_eq!(comparison["coverage"]["unconfigured"], 2);
        let hosts = comparison["hosts"].as_array().unwrap();
        assert_eq!(hosts[0]["status"], "not_applicable");
        assert_eq!(hosts[0]["reason"], "Desktop-only workload");
        assert_eq!(hosts[1]["status"], "unconfigured");
        assert_eq!(hosts[2]["status"], "unconfigured");
    }

    #[test]
    fn only_fresh_complete_inventory_can_report_a_missing_guest() {
        let host = json!({"id":"machine-test","spec":{
            "host":"test","owner":"fixture","lifecycle":"active",
            "capabilities":["machine"],"resources":{"metrics_stream":{"enabled":true}}
        }});
        let service = json!({"id":"service-test","spec":{"host":"test","owner":"fixture","lifecycle":"active",
            "resources":{"runtime_binding":{"kind":"guest","machine_id":"machine-test","guest_id":"100"}}
        }});
        let records = [host, service];
        let report = Report {
            checked: Some(100.0),
            expires: Some(110.0),
            entries: Some(vec![]),
            ..Report::default()
        };
        let reports = HashMap::from([("machine-test".into(), report)]);
        let fresh = comparison(&records, &reports, &LeaseOwnership::default(), 103.0);
        assert_eq!(fresh["coverage"]["verified"], 1);
        assert_eq!(fresh["findings"][0]["code"], "missing-guest");
        let stale = comparison(&records, &reports, &LeaseOwnership::default(), 111.0);
        assert_eq!(stale["coverage"]["verified"], 0);
        assert_eq!(stale["findings"].as_array().unwrap().len(), 1);
        assert_eq!(stale["findings"][0]["code"], "collector-unverified");
    }

    #[test]
    fn allocator_ownership_requires_the_same_host_id_name_and_type() {
        let nodes = HashMap::from([("provider".to_owned(), "machine-test".to_owned())]);
        let lease = "72c63ca830af4d6287089aed16976d80";
        let guest_name = format!("lease-{lease}");
        let value =
            json!([{"node":"provider","id":"100","name":guest_name,"type":"qemu","lease":lease}]);
        let exact = (
            "machine-test".into(),
            "100".into(),
            guest_name.clone(),
            "qemu".into(),
        );
        let wrong = (
            "machine-test".into(),
            "100".into(),
            guest_name,
            "lxc".into(),
        );
        let seen = HashSet::from([exact.clone(), wrong.clone()]);
        let claims = match_leases(&nodes, &seen, &value).unwrap();
        assert_eq!(claims.get(&exact).unwrap(), lease);
        assert!(!claims.contains_key(&wrong));
    }
}
