use std::collections::BTreeSet;

use serde_json::{Map, Value};
use url::Url;

use crate::Problem;

const FIELDS: &[&str] = &[
    "name",
    "host",
    "owner",
    "capabilities",
    "endpoints",
    "lifecycle",
    "runbook",
    "notes",
    "dependencies",
    "credential_ref",
    "deployment",
    "resources",
    "backup",
    "config_backup",
    "probe",
];

pub fn valid_id(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn invalid(detail: impl Into<String>) -> Problem {
    Problem::new(400, detail)
}

fn known_keys(value: &Map<String, Value>, allowed: &[&str]) -> bool {
    value.keys().all(|key| allowed.contains(&key.as_str()))
}

fn endpoint(url: &Value) -> Result<Url, Problem> {
    let url = url
        .as_str()
        .ok_or_else(|| invalid("URL must be a string"))?;
    let no_credentials = url.split_once("://").is_some_and(|(_, address)| {
        address
            .split(['/', '?', '#'])
            .next()
            .is_some_and(|host| !host.contains('@'))
    });
    Url::parse(url)
        .ok()
        .filter(|parsed| {
            matches!(parsed.scheme(), "http" | "https" | "ssh")
                && parsed.host().is_some()
                && parsed.query().is_none()
                && parsed.fragment().is_none()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && no_credentials
        })
        .ok_or_else(|| {
            invalid("Use http/https/ssh URLs without embedded credentials, query or fragment")
        })
}

pub(crate) fn spec(doc: &Value, local_checks: &BTreeSet<String>) -> Result<(), Problem> {
    let spec = doc
        .as_object()
        .filter(|spec| known_keys(spec, FIELDS))
        .ok_or_else(|| invalid("Unknown fields; store credential references, never secrets"))?;
    for key in ["name", "host", "owner", "runbook"] {
        if !doc[key]
            .as_str()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(invalid(format!("Required nonempty string: {key}")));
        }
    }
    for key in ["capabilities", "dependencies"] {
        if !doc[key].as_array().is_some_and(|values| {
            values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| !value.is_empty()))
        }) {
            return Err(invalid(format!("Required string array: {key}")));
        }
    }
    if !matches!(
        doc["lifecycle"].as_str(),
        Some("active" | "planned" | "retired")
    ) {
        return Err(invalid("Invalid lifecycle"));
    }
    let endpoints = doc["endpoints"]
        .as_object()
        .ok_or_else(|| invalid("endpoints must be an object"))?;
    for url in endpoints.values() {
        endpoint(url)?;
    }
    for key in ["notes", "credential_ref", "deployment", "backup"] {
        if spec.get(key).is_some_and(|value| !value.is_string()) {
            return Err(invalid(format!("{key} must be a string")));
        }
    }
    if let Some(backup) = spec.get("config_backup") {
        backup_settings(backup)?;
    }
    if let Some(resources) = spec.get("resources") {
        let resources = resources
            .as_object()
            .ok_or_else(|| invalid("resources must be an object"))?;
        if let Some(rule) = resources.get("registration") {
            address_policy(rule, false)?;
        }
        if let Some(bindings) = resources.get("address_bindings") {
            let rules = bindings
                .as_array()
                .filter(|rules| rules.len() <= 32)
                .ok_or_else(|| invalid("Invalid address bindings"))?;
            for rule in rules {
                address_policy(rule, true)?;
            }
        }
    }
    if let Some(probe) = spec.get("probe") {
        probe_settings(probe, local_checks)?;
    }
    Ok(())
}

fn backup_settings(value: &Value) -> Result<(), Problem> {
    let fields = value
        .as_object()
        .filter(|fields| known_keys(fields, &["status", "locations", "notes"]))
        .ok_or_else(|| invalid("Invalid configuration backup metadata"))?;
    let status = value["status"]
        .as_str()
        .filter(|status| matches!(*status, "unknown" | "manual" | "scheduled"))
        .ok_or_else(|| invalid("Invalid configuration backup metadata"))?;
    let locations = value["locations"]
        .as_array()
        .filter(|items| status == "unknown" || !items.is_empty())
        .ok_or_else(|| invalid("Configuration backup locations required"))?;
    for location in locations {
        if !location
            .as_object()
            .is_some_and(|location| location.len() == 2)
            || !location["host"].as_str().is_some_and(valid_id)
            || !location["path"]
                .as_str()
                .is_some_and(|path| path.starts_with('/') && !path.chars().any(|c| c < ' '))
        {
            return Err(invalid(
                "Use a machine name and absolute backup path, never credentials",
            ));
        }
    }
    if fields.get("notes").is_some_and(|value| !value.is_string()) {
        return Err(invalid("Backup notes must be a string"));
    }
    Ok(())
}

fn address_policy(value: &Value, binding: bool) -> Result<(), Problem> {
    let allowed = [
        "endpoint_keys",
        "access_ids",
        "metrics",
        "probe",
        "machine_id",
    ];
    let fields = value
        .as_object()
        .filter(|fields| known_keys(fields, &allowed[..if binding { 5 } else { 4 }]))
        .ok_or_else(|| invalid("Invalid address bindings"))?;
    if binding && !value["machine_id"].as_str().is_some_and(valid_id) {
        return Err(invalid("Invalid address bindings"));
    }
    for key in ["endpoint_keys", "access_ids"] {
        if let Some(values) = fields.get(key)
            && !values.as_array().is_some_and(|values| {
                values.len() <= 32
                    && values.iter().all(|value| {
                        value
                            .as_str()
                            .is_some_and(|value| (1..128).contains(&value.chars().count()))
                    })
            })
        {
            return Err(invalid("Invalid address bindings"));
        }
    }
    for key in ["metrics", "probe"] {
        if fields.get(key).is_some_and(|value| !value.is_boolean()) {
            return Err(invalid("Invalid address bindings"));
        }
    }
    Ok(())
}

fn probe_settings(value: &Value, local_checks: &BTreeSet<String>) -> Result<(), Problem> {
    let fields = value.as_object().ok_or_else(|| invalid("Invalid probe"))?;
    let vantage = fields
        .get("vantage")
        .map_or(Some("registry"), Value::as_str);
    if let Some(host) = fields.get("tcp_host") {
        if !known_keys(fields, &["tcp_host", "tcp_port", "vantage"])
            || !host
                .as_str()
                .is_some_and(|host| !host.trim().is_empty() && !host.contains(['/', '@', '?', '#']))
        {
            return Err(invalid("Invalid TCP host"));
        }
        if !value["tcp_port"]
            .as_u64()
            .is_some_and(|port| (1..=65535).contains(&port))
            || !matches!(vantage, Some("desktop-node" | "registry"))
        {
            return Err(invalid("Invalid TCP probe"));
        }
        return Ok(());
    }
    if let Some(check) = fields.get("check") {
        if fields.len() == 2
            && fields.contains_key("vantage")
            && vantage.is_some_and(valid_id)
            && check.as_str().is_some_and(|key| local_checks.contains(key))
        {
            return Ok(());
        }
        return Err(invalid("Invalid local check"));
    }
    if !known_keys(
        fields,
        &[
            "url",
            "expected_status",
            "vantage",
            "bearer_ref",
            "json_not_in",
        ],
    ) || !fields.contains_key("url")
    {
        return Err(invalid("Invalid probe"));
    }
    let url = endpoint(&value["url"])?;
    if (fields.contains_key("bearer_ref") || fields.contains_key("json_not_in"))
        && vantage != Some("registry")
    {
        return Err(invalid(
            "Authenticated/JSON probes require registry vantage",
        ));
    }
    if fields
        .get("bearer_ref")
        .is_some_and(|key| !key.as_str().is_some_and(valid_id))
    {
        return Err(invalid("Invalid bearer credential reference"));
    }
    if let Some(rules) = fields.get("json_not_in")
        && !rules.as_object().is_some_and(|rules| {
            !rules.is_empty()
                && rules.values().all(|values| {
                    values
                        .as_array()
                        .is_some_and(|values| values.iter().all(Value::is_string))
                })
        })
    {
        return Err(invalid("Invalid JSON health rules"));
    }
    if !vantage.is_some_and(valid_id) {
        return Err(invalid("Unknown probe vantage"));
    }
    if !matches!(url.scheme(), "http" | "https") {
        return Err(invalid("Probe requires HTTP(S)"));
    }
    if !fields
        .get("expected_status")
        .map_or(Some(200), Value::as_u64)
        .is_some_and(|status| (100..=599).contains(&status))
    {
        return Err(invalid("Invalid probe status"));
    }
    Ok(())
}

pub(crate) fn observation(value: &Value) -> Result<(&str, &str, u64), Problem> {
    let known = value
        .as_object()
        .filter(|value| known_keys(value, &["status", "evidence", "ttl_seconds"]))
        .ok_or_else(|| invalid("Invalid observation"))?;
    let status = value["status"]
        .as_str()
        .filter(|status| matches!(*status, "healthy" | "unhealthy" | "unknown"))
        .ok_or_else(|| invalid("Invalid health status"))?;
    let evidence = value["evidence"]
        .as_str()
        .filter(|evidence| !evidence.trim().is_empty() && evidence.chars().count() <= 2000)
        .ok_or_else(|| invalid("Evidence required, maximum 2000 characters"))?;
    let ttl = known
        .get("ttl_seconds")
        .map_or(Some(300), Value::as_u64)
        .filter(|ttl| (1..=86400).contains(ttl))
        .ok_or_else(|| invalid("TTL must be 1..86400 seconds"))?;
    Ok((status, evidence, ttl))
}

pub(crate) fn runtime_report(value: &Value) -> Result<(&str, u64), Problem> {
    if !value
        .as_object()
        .is_some_and(|obj| obj.len() == 2 && known_keys(obj, &["state", "ttl_seconds"]))
    {
        return Err(invalid("Report must contain a state and TTL"));
    }
    let state = value["state"]
        .as_str()
        .filter(|state| matches!(*state, "present" | "missing" | "unverified"));
    let ttl = value["ttl_seconds"]
        .as_u64()
        .filter(|ttl| (60..=3600).contains(ttl));
    state
        .zip(ttl)
        .ok_or_else(|| invalid("Invalid runtime state or TTL"))
}
