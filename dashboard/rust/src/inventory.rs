use std::collections::HashMap;

use serde_json::{Value, json};

use crate::Snapshot;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Machine,
    Service,
    Backup,
    Archive,
}

pub(crate) fn kind(record: &Value) -> Kind {
    let spec = &record["spec"];
    let capabilities = spec["capabilities"].as_array();
    let has =
        |name: &str| capabilities.is_some_and(|values| values.iter().any(|value| value == name));
    match spec["resources"]["kind"].as_str() {
        Some("machine" | "virtual-machine" | "container") => Kind::Machine,
        _ if has("machine") => Kind::Machine,
        Some("historical-evidence") => Kind::Archive,
        _ if has("inventory-history") => Kind::Archive,
        Some("backup-job") => Kind::Backup,
        _ => Kind::Service,
    }
}

pub(crate) fn guest(record: &Value) -> bool {
    matches!(
        record["spec"]["resources"]["kind"].as_str(),
        Some("virtual-machine" | "container")
    )
}

pub(crate) fn access<'a>(record: &'a Value, scope: Option<&str>) -> &'a Value {
    let resources = &record["spec"]["resources"];
    match scope {
        None => &resources["management_access"],
        Some(target) if record["id"].as_str() == Some(target) => {
            &resources["staging_review"]["access"]["management_access"]
        }
        _ => &Value::Null,
    }
}

pub(crate) fn fields(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(key).map(|value| (key.to_string(), value.clone())))
            .collect(),
    )
}

fn array(value: &Value) -> Value {
    value
        .as_array()
        .map_or_else(|| json!([]), |items| Value::Array(items.clone()))
}

pub fn project(
    records: &[Value],
    metrics: &HashMap<String, Value>,
    scope: Option<&str>,
) -> Snapshot {
    let mut parents = HashMap::new();
    for record in records {
        if kind(record) == Kind::Machine && !guest(record) {
            if let Some(host) = record["spec"]["host"].as_str() {
                parents.insert(host, &record["id"]);
            }
        }
    }
    let mut snapshot = Snapshot::default();
    for record in records {
        let spec = &record["spec"];
        let classification = kind(record);
        if classification == Kind::Archive || spec["lifecycle"] == "retired" {
            continue;
        }
        let resources = &spec["resources"];
        let mut backup = fields(&spec["config_backup"], &["status", "notes"]);
        if spec["config_backup"].is_null() {
            backup["status"] = json!("unknown");
        }
        backup["locations"] = Value::Array(
            spec["config_backup"]["locations"]
                .as_array()
                .map_or_else(Vec::new, |locations| {
                    locations
                        .iter()
                        .map(|item| fields(item, &["host", "path"]))
                        .collect()
                }),
        );
        let mut item = json!({
            "id":record["id"],"name":spec["name"],"host":spec["host"],"capabilities":spec["capabilities"],
            "endpoints":spec["endpoints"],"lifecycle":spec["lifecycle"],"config_backup":backup,
            "health":fields(&record["health"], &["status","evidence","checked","expires"]),
            "runtime":fields(&resources["runtime"], &["state","checked","expires","evidence"]),
            "dependencies":array(&spec["dependencies"]),
            "recorded_state":resources["state"],
            "deployment":spec["deployment"].as_str().unwrap_or_default(),
        });
        if classification == Kind::Machine {
            let is_guest = guest(record);
            item["kind"] = json!("machine");
            item["machine_type"] = if is_guest {
                resources["kind"].clone()
            } else {
                json!("machine")
            };
            item["machine_id"] = if is_guest {
                resources["machine_id"].clone()
            } else {
                Value::Null
            };
            item["display_name"] = if is_guest {
                spec["name"].clone()
            } else {
                spec["host"].clone()
            };
            item["access"] = array(access(record, scope));
            item["console_login"] = if scope.is_none() {
                resources["console_login"].clone()
            } else {
                Value::Null
            };
            item["model"] = resources["model"].clone();
            item["cpu"] = resources["hardware"]["cpu"].clone();
            item["cores"] = resources["hardware"]["cores"].clone();
            item["ram_mib"] = resources["hardware"]["memory_mib"].clone();
            item["os"] = resources["os"]["name"].clone();
            item["metrics"] = record["id"]
                .as_str()
                .and_then(|id| metrics.get(id))
                .cloned()
                .unwrap_or_else(|| json!({"status":"unconfigured"}));
            snapshot.machines.push(item);
            continue;
        }
        item["machine_id"] = resources["machine_id"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map_or_else(
                || {
                    spec["host"]
                        .as_str()
                        .and_then(|host| parents.get(host))
                        .map_or(Value::Null, |id| (*id).clone())
                },
                |parent| json!(parent),
            );
        if classification == Kind::Backup {
            item["kind"] = json!("backup");
            snapshot.backups.push(item);
        } else {
            item["kind"] = json!("service");
            item["access"] = array(access(record, scope));
            snapshot.services.push(item);
        }
    }
    snapshot
}

pub fn address_audit(records: &[Value]) -> Option<Value> {
    records.iter().find_map(|record| {
        let run = &record["spec"]["resources"]["fleet_address_audit"]["latest_run"];
        let observed_at = run["at"].as_str().filter(|stamp| !stamp.is_empty())?;
        Some(json!({
            "status":run["status"].as_str().unwrap_or("unknown"),
            "at":observed_at,
            "warnings":run["warnings"].as_array().map_or(0, Vec::len),
            "unreserved_static":run["verified_unreserved_static_count"],
            "record":record["id"],
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(id: &str, kind: &str, binding: Option<&str>) -> Value {
        json!({
            "id":id,
            "spec":{
                "name":id,"host":"test","capabilities":[],"endpoints":{},"lifecycle":"active",
                "resources":{"kind":kind,"machine_id":binding,"fixture_key":"private-metadata"}
            },
            "health":{"status":"healthy","checked":1,"expires":2,"fixture_key":"private-metadata"}
        })
    }

    #[test]
    fn catalog_projection_keeps_explicit_placement_and_excludes_internal_records() {
        let records = vec![
            row("physical", "machine", Some("unrelated")),
            row("guest", "virtual-machine", Some("physical")),
            row("service", "other", Some("guest")),
            row("backup", "backup-job", None),
            row("archive", "historical-evidence", None),
        ];
        let snapshot = project(&records, &Default::default(), None);
        assert_eq!(snapshot.machines.len(), 2);
        assert_eq!(snapshot.services.len(), 1);
        assert_eq!(snapshot.backups.len(), 1);
        assert!(snapshot.machines[0]["machine_id"].is_null());
        assert_eq!(snapshot.machines[1]["machine_id"], "physical");
        assert_eq!(snapshot.services[0]["machine_id"], "guest");
        assert_eq!(snapshot.backups[0]["machine_id"], "physical");
        assert_eq!(
            snapshot.machines[0]["metrics"],
            json!({"status":"unconfigured"})
        );
        assert!(
            !serde_json::to_string(&snapshot)
                .unwrap()
                .contains("private-metadata")
        );
    }

    #[test]
    fn staging_projection_exposes_only_the_disposable_login_paths() {
        let mut owned = row("owned", "machine", None);
        owned["spec"]["resources"]["management_access"] = json!([{"id":"production"}]);
        owned["spec"]["resources"]["staging_review"]["access"] = json!({
            "management_access":[{"id":"os","label":"SSH","browser_terminal":"/terminal/disposable/"}],
            "terminal_id":"disposable",
        });
        let mut unrelated = row("other", "machine", None);
        unrelated["spec"]["resources"]["management_access"] = json!([{"id":"another-target"}]);
        let snapshot = project(&[owned, unrelated], &Default::default(), Some("owned"));
        assert_eq!(snapshot.machines[0]["access"][0]["id"], "os");
        assert_eq!(snapshot.machines[1]["access"], json!([]));
    }
}
