use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::Path,
    time::Duration,
};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep_until},
};

use crate::{
    Dashboard, Registry,
    catalog::Change,
    collector::Target,
    guests::{GuestInventory, overlay},
    inventory::{address_audit, project},
    reconcile::{LeaseOwnership, comparison},
    unix_time,
    upstream::{lease_page, read_json},
};

#[derive(Deserialize)]
struct Endpoint {
    url: String,
    server_name: String,
}

#[derive(Default)]
struct CatalogState {
    records: Vec<Value>,
    checked: Option<f64>,
    retry_at: Option<Instant>,
    retry_delay: Option<Duration>,
    refresh: bool,
    health: BTreeSet<String>,
    feed: &'static str,
    error: Option<&'static str>,
}

impl CatalogState {
    fn action(&mut self, change: Change) {
        match change {
            Change::Catalog => self.refresh = true,
            Change::Health(ids) => {
                self.health.extend(ids);
                if self.health.len() > 4096 {
                    self.health.clear();
                    self.refresh = true;
                }
            }
            Change::Connection(connected) => {
                self.feed = if connected {
                    "connected"
                } else {
                    "disconnected"
                };
                self.refresh |= connected;
            }
        }
    }

    fn failed(&mut self, reason: &'static str) {
        tracing::warn!(reason, "Dashboard catalog update failed");
        self.refresh = true;
        self.error = Some("Registry refresh failed; displayed readings may be stale.");
        let delay = self.retry_delay.unwrap_or(Duration::from_secs(1));
        self.retry_at = Some(Instant::now() + delay);
        self.retry_delay = Some((delay * 2).min(Duration::from_secs(30)));
    }

    fn refreshed(&mut self, records: Vec<Value>) {
        self.records = records;
        self.checked = Some(unix_time());
        self.retry_at = None;
        self.retry_delay = None;
        self.refresh = false;
        self.error = None;
        self.health.clear();
    }
}

fn declared_targets(records: &[Value]) -> HashMap<String, Target> {
    records
        .iter()
        .filter_map(|record| {
            let spec = &record["spec"];
            let cfg = &spec["resources"]["metrics_stream"];
            if spec["lifecycle"] != "active" || cfg["enabled"] != true {
                return None;
            }
            let id = record["id"].as_str()?;
            let endpoint = cfg["url"].as_str()?;
            let identity = cfg["server_name"].as_str();
            if !cfg["server_name"].is_null() && identity.is_none() {
                return None;
            }
            match Target::parse(endpoint, identity) {
                Ok(target) => Some((id.to_owned(), target)),
                Err(reason) => {
                    tracing::warn!(reason, "Registry declared an invalid collector endpoint");
                    None
                }
            }
        })
        .collect()
}

async fn overrides(
    path: &Path,
    endpoints: &HashMap<String, Target>,
) -> Result<HashMap<String, Target>, &'static str> {
    let mut result = endpoints.clone();
    if path.is_file() {
        let values: HashMap<String, Endpoint> = read_json(path).await?;
        for (id, endpoint) in values {
            let target = Target::parse(&endpoint.url, Some(&endpoint.server_name))?;
            result.insert(id, target);
        }
    }
    Ok(result)
}

async fn refresh_health(
    registry: &Registry,
    changed: &BTreeSet<String>,
) -> Result<HashMap<String, Value>, &'static str> {
    let changed: Vec<_> = changed.iter().cloned().collect();
    let mut health = HashMap::new();
    for batch in changed.chunks(128) {
        health.extend(registry.health(batch).await?);
    }
    Ok(health)
}

impl Dashboard {
    pub async fn maintain(&self, registry: Registry) -> Result<(), &'static str> {
        let mut stop = self.shared.shutdown.clone();
        let mut requests = self.requested_updates();
        let mut viewers = self.shared.viewers.subscribe();
        let (feed, mut changes) = mpsc::channel(64);
        let feed_task = registry
            .changes_enabled
            .then(|| tokio::spawn(registry.clone().watch(feed.clone(), stop.clone())));
        let lease_settings = self.private.join("lease-review.json");
        let mut ownership = LeaseOwnership::from_config(&lease_settings).await?;
        let mut lease_issues = if self.shared.lease_review_enabled {
            Some(json!({"status":"checking"}))
        } else {
            None
        };
        let mut lease_due = Instant::now();
        let fleet = GuestInventory::new(self.private.clone());
        let mut fleet_changes = fleet.updates();
        let override_file = self.private.join("guest-endpoints.json");
        let mut catalog = CatalogState {
            refresh: true,
            error: Some("Loading registry"),
            feed: if registry.changes_enabled {
                "waiting"
            } else {
                "disabled"
            },
            ..CatalogState::default()
        };
        let mut registered_collectors = HashSet::new();
        let mut applied_generation = 0;
        let mut compared = None;
        let mut current_comparison = None;
        let mut comparison_deadline = Instant::now();
        let mut seen_viewers = 0;
        while !*stop.borrow() {
            let request = requests.borrow_and_update().clone();
            let new_comparison =
                request.comparison_id.is_some() && request.comparison_id != compared;
            let reader_count = *viewers.borrow_and_update();
            if reader_count > 0 && seen_viewers == 0 {
                lease_due = Instant::now();
            }
            seen_viewers = reader_count;
            if request.catalog_generation > applied_generation || new_comparison {
                catalog.refresh = true;
            }
            if catalog.refresh && catalog.retry_at.is_none_or(|when| when <= Instant::now()) {
                let fetched = async {
                    let records = registry.records().await?;
                    let metrics = declared_targets(&records);
                    let guests = overrides(&override_file, &metrics).await?;
                    Ok::<_, &'static str>((records, metrics, guests))
                }
                .await;
                match fetched {
                    Ok((records, metrics, guests)) => {
                        registered_collectors = metrics.keys().cloned().collect();
                        catalog.refreshed(records);
                        applied_generation = request.catalog_generation;
                        self.metrics.configure(metrics);
                        if new_comparison {
                            current_comparison.clone_from(&request.comparison_id);
                            compared.clone_from(&request.comparison_id);
                            comparison_deadline = Instant::now() + Duration::from_secs(40);
                            lease_due = Instant::now();
                        }
                        fleet.configure(&guests, new_comparison);
                    }
                    Err(reason) => catalog.failed(reason),
                }
            } else if !catalog.refresh && !catalog.health.is_empty() {
                match refresh_health(&registry, &catalog.health).await {
                    Ok(health) => {
                        for record in &mut catalog.records {
                            let Some(id) = record["id"].as_str() else {
                                continue;
                            };
                            if catalog.health.contains(id) {
                                record["health"] = health
                                    .get(id)
                                    .cloned()
                                    .unwrap_or_else(|| json!({"status":"unknown"}));
                            }
                        }
                        catalog.health.clear();
                    }
                    Err(reason) => catalog.failed(reason),
                }
            }
            if reader_count > 0 && lease_issues.is_some() && Instant::now() >= lease_due {
                let (issue, delay) = read_lease_issues(&lease_settings).await;
                lease_due = Instant::now() + delay;
                let mut current = lease_issues.clone().unwrap_or_default();
                if let Some(object) = current.as_object_mut() {
                    object.remove("as_of");
                }
                if current != issue {
                    let mut issue = issue;
                    issue["as_of"] = json!(unix_time());
                    lease_issues = Some(issue);
                }
            }
            let now = unix_time();
            let reports = fleet.snapshot();
            ownership
                .update(
                    &reports,
                    new_comparison && compared == request.comparison_id,
                    now,
                )
                .await;
            let mut accuracy = comparison(&catalog.records, &reports, &ownership, now);
            let pending = registered_collectors
                .iter()
                .any(|id| !reports.contains_key(id));
            accuracy["pending"] = json!(pending);
            let mut records = catalog.records.clone();
            overlay(&mut records, &reports, now);
            let mut snapshot = project(
                &records,
                &self.metrics.readings(),
                self.access_target.as_deref(),
            );
            snapshot.inventory_accuracy = Some(accuracy);
            snapshot.address_audit = address_audit(&catalog.records);
            snapshot.fetched_at = Some(now);
            snapshot.error = catalog.error.map(str::to_owned);
            snapshot.catalog = Some(
                json!({"checked":catalog.checked,"feed":catalog.feed,"snapshot_at":registry.snapshot_at}),
            );
            snapshot.lease_issues.clone_from(&lease_issues);
            self.publish(snapshot);
            if catalog.error.is_none() {
                self.finish_refresh(applied_generation);
            }
            if let Some(id) = &current_comparison
                && (!pending || Instant::now() >= comparison_deadline)
            {
                self.finish_comparison(id, catalog.error.is_none());
                current_comparison = None;
            }

            let mut wake = catalog
                .retry_at
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(90));
            if let Some(retry) = ownership.retry() {
                wake = wake.min(retry);
            }
            if reader_count > 0 && lease_issues.is_some() {
                wake = wake.min(lease_due);
            }
            if current_comparison.is_some() {
                wake = wake.min(comparison_deadline);
            }
            tokio::select! {
                _ = stop.wait_for(|stopping| *stopping) => break,
                _ = requests.changed() => {},
                _ = viewers.changed() => {},
                _ = fleet_changes.changed() => {},
                change = changes.recv() => {
                    if let Some(change) = change {
                        catalog.action(change);
                        while let Ok(change) = changes.try_recv() {
                            catalog.action(change);
                        }
                    }
                }
                _ = sleep_until(wake) => {},
            }
        }
        if let Some(task) = feed_task {
            task.abort();
        }
        Ok(())
    }
}

async fn read_lease_issues(path: &Path) -> (Value, Duration) {
    const INTERVAL: Duration = Duration::from_secs(20);
    let result = lease_page(path, "issues", None).await.and_then(|report| {
        let time = report["as_of"]
            .as_f64()
            .ok_or("Invalid allocator timestamp")?;
        if !report["issues"].is_array() || !report["counts"].is_object() {
            return Err("Invalid allocator issues response");
        }
        let delay = if let Some(deadline) = report["next_deadline"].as_f64() {
            Duration::from_secs_f64((deadline - time).clamp(0.1, INTERVAL.as_secs_f64()))
        } else if report["next_deadline"].is_null() {
            INTERVAL
        } else {
            return Err("Invalid allocator deadline");
        };
        Ok((
            json!({"status":"verified","issues":report["issues"],"counts":report["counts"]}),
            delay,
        ))
    });
    match result {
        Ok(value) => value,
        Err(reason) => {
            tracing::warn!(reason, "Lease issues unavailable");
            (json!({"status":"unavailable"}), INTERVAL)
        }
    }
}
