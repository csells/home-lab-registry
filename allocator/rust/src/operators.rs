use std::{collections::BTreeSet, io};

use serde_json::{Map, Value, json};

use crate::{
    Rejected, canonical_requirements, native_provider::verify_requirements,
    proxmox_dynamic::legacy_json, tokens::hash,
};

const SHAPE: [&str; 3] = ["memory_mib", "disk_mib", "vcpus"];
const REPLACE: [&str; 5] = [
    "image_sha256",
    "bootstrap_revision",
    "network",
    "storage",
    "worker",
];

fn input() -> io::Error {
    io::Error::other("Provide complete verified observations and capability requirements.")
}

fn sizes(value: &Value, keys: &[&str], positive: bool) -> io::Result<Vec<i64>> {
    keys.iter()
        .map(|key| {
            value[*key]
                .as_i64()
                .filter(|value| *value >= i64::from(positive))
                .ok_or_else(input)
        })
        .collect()
}

pub fn placement(
    request: &Value,
    observations: &Value,
    now: f64,
) -> Result<Value, Box<dyn std::error::Error>> {
    canonical_requirements(request)?;
    let rows = observations
        .as_array()
        .filter(|rows| (1..=128).contains(&rows.len()))
        .ok_or_else(input)?;
    let mut eligible = Vec::new();
    let mut rejected = Vec::new();
    let mut ids = BTreeSet::new();
    for row in rows {
        let id = row["worker"]
            .as_str()
            .filter(|id| {
                id.strip_prefix("machine-").is_some_and(|host| {
                    !host.is_empty()
                        && host.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
                        })
                })
            })
            .ok_or_else(input)?;
        if !ids.insert(id) {
            return Err(input().into());
        }
        let fail = |reason| json!({"worker":id,"reason":reason});
        if row["checked_at"]
            .as_f64()
            .is_none_or(|checked| !(now - 120.0..=now + 5.0).contains(&checked))
        {
            rejected.push(fail("capacity_unverified"));
            continue;
        }
        let size = sizes(&row["recipe"], &SHAPE, true)?;
        let remaining = sizes(&row["headroom"], &SHAPE[..2], false)?;
        let floor = sizes(&row["minimum_headroom"], &SHAPE[..2], false)?;
        let mut offer = Map::from_iter([
            ("os".to_owned(), json!("linux")),
            ("architecture".to_owned(), json!("amd64")),
            ("distribution".to_owned(), json!("debian13")),
        ]);
        offer.extend(row["recipe"].as_object().ok_or_else(input)?.clone());
        offer.insert("gpu".to_owned(), json!("none"));
        if let Err(error) = verify_requirements(request, &Value::Object(offer), &json!({})) {
            rejected.push(fail(error.code()));
            continue;
        }
        let memory = remaining[0] - size[0];
        let disk = remaining[1] - size[1];
        if memory < floor[0] {
            rejected.push(fail("host_memory_headroom"));
        } else if disk < floor[1] {
            rejected.push(fail("host_storage_headroom"));
        } else {
            eligible.push((memory, disk, id, &row["checked_at"]));
        }
    }
    let winner=eligible.iter().min_by_key(|(memory,disk,id,_)|(*memory,*disk,*id))
        .map(|(memory,disk,id,checked)|json!({"worker":id,"checked_at":checked,"headroom_after_start":{"memory_mib":memory,"disk_mib":disk}}));
    Ok(
        json!({"placement":winner,"eligible":eligible.iter().map(|(_,_,id,_)|id).collect::<Vec<_>>(),"rejected":rejected}),
    )
}

pub fn maintenance(
    current: &Value,
    desired: &Value,
    active: &Value,
    spare: &Value,
    replacement: Option<bool>,
) -> io::Result<Value> {
    let keys = SHAPE.into_iter().chain(REPLACE).collect::<BTreeSet<_>>();
    for recipe in [current, desired] {
        let fields = recipe.as_object().ok_or_else(input)?;
        if fields.len() != keys.len()
            || keys.iter().any(|key| !fields.contains_key(*key))
            || REPLACE
                .iter()
                .any(|key| recipe[*key].as_str().is_none_or(str::is_empty))
        {
            return Err(input());
        }
    }
    let before = sizes(current, &SHAPE, true)?;
    let after = sizes(desired, &SHAPE, true)?;
    let mut active = active
        .as_array()
        .ok_or_else(input)?
        .iter()
        .map(|lease| Some((lease["id"].as_str()?, lease["owner"].as_str()?)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(input)?;
    active.sort();
    let changed = keys
        .into_iter()
        .filter(|key| current[*key] != desired[*key])
        .collect::<Vec<_>>();
    let action = if REPLACE.iter().any(|key| changed.contains(key)) || after[1] < before[1] {
        "replace"
    } else if changed.contains(&"memory_mib") || changed.contains(&"vcpus") {
        "reboot"
    } else if changed.is_empty() {
        "none"
    } else {
        "grow-disk"
    };
    let amounts = if action == "replace" {
        after
    } else {
        after
            .iter()
            .zip(before)
            .map(|(after, before)| (after - before).max(0))
            .collect()
    };
    let required: Map<String, Value> = SHAPE
        .iter()
        .zip(&amounts)
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect();
    let mut missing = Map::new();
    if !spare.is_null() {
        for ((key, required), available) in
            SHAPE.iter().zip(&amounts).zip(sizes(spare, &SHAPE, false)?)
        {
            if *required > available {
                missing.insert((*key).to_owned(), json!(required - available));
            }
        }
    }
    let requires_review = !active.is_empty() && matches!(action, "replace" | "reboot");
    let blocker = if spare.is_null() && amounts.iter().any(|amount| *amount > 0) {
        Some("capacity_unverified")
    } else if !missing.is_empty() {
        Some(if action == "replace" {
            "overlap_capacity_unavailable"
        } else {
            "capacity_unavailable"
        })
    } else if action == "replace" && replacement != Some(true) {
        Some(if replacement == Some(false) {
            "replacement_slot_unavailable"
        } else {
            "replacement_slot_unverified"
        })
    } else if requires_review {
        Some("active_owners_require_review")
    } else {
        None
    };
    Ok(json!({"action":action,"changed":changed,
        "affected_leases":active.into_iter().filter(|_|action!="none").map(|(id,owner)|json!({"id":id,"owner":owner})).collect::<Vec<_>>(),
        "additional_capacity":required,"missing_capacity":missing,"blocker":blocker,"requires_verified_checkpoint":requires_review}))
}

pub fn cuda_preflight(
    recipe: &Value,
    config: &Value,
    observation: &Value,
    now: f64,
) -> Result<Value, Rejected> {
    let denied = |code, reason| Rejected::new(code, reason, 409);
    let stamp = &observation["observed_at"];
    if stamp
        .as_f64()
        .is_none_or(|stamp| !(now - 45.0..=now + 5.0).contains(&stamp))
    {
        return Err(denied(
            "cuda_observation_stale",
            "Check live GPU usage and reservations before admission.",
        ));
    }
    let mut clean = config.clone();
    if let Some(fields) = clean.as_object_mut() {
        fields.remove("digest");
        fields.remove("lock");
    }
    if config["template"] != 1
        || config["unprivileged"] != 1
        || config["arch"] != "amd64"
        || config["ostype"] != "debian"
        || recipe["template_config_sha256"] != hash(&legacy_json(&clean, false))
        || config["rootfs"]
            .as_str()
            .and_then(|rootfs| rootfs.split(',').next())
            != recipe["volume"].as_str()
        || !recipe["volume"].is_string()
    {
        return Err(denied(
            "cuda_template_changed",
            "The enrolled unprivileged Debian CUDA template changed.",
        ));
    }
    let fields = config.as_object().expect("verified object");
    let devices = fields
        .iter()
        .filter(|(key, _)| {
            key.strip_prefix("dev").is_some_and(|index| {
                !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .map(|(_, value)| {
            value
                .as_str()
                .and_then(|value| value.split(',').next())
                .unwrap_or_default()
                .trim_start_matches("path=")
        })
        .collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "/dev/nvidia0",
        "/dev/nvidiactl",
        "/dev/nvidia-uvm",
        "/dev/nvidia-uvm-tools",
    ]);
    if devices != expected
        || fields
            .keys()
            .any(|name| name.starts_with("lxc.") || name.starts_with("mp"))
    {
        return Err(denied(
            "cuda_device_scope",
            "The template has unexpected device mappings or host mounts.",
        ));
    }
    let gpu = &observation["gpu"];
    if gpu["uuid"] != recipe["gpu_uuid"]
        || gpu["driver"] != recipe["driver"]
        || !gpu["uuid"].is_string()
        || !gpu["driver"].is_string()
        || gpu["memory_mib"]
            .as_u64()
            .zip(recipe["minimum_vram_mib"].as_u64())
            .is_none_or(|(actual, minimum)| actual < minimum)
    {
        return Err(denied(
            "cuda_worker_changed",
            "The observed GPU or driver differs from the qualified recipe.",
        ));
    }
    let active = |value: &Value| {
        if let Some(rows) = value.as_array() {
            !rows.is_empty()
        } else if let Some(count) = value.as_u64() {
            count > 0
        } else {
            value != false
        }
    };
    if active(&observation["compute_processes"]) || active(&observation["active_reservations"]) {
        return Err(denied(
            "cuda_busy",
            "The GPU has an active compute process or another reservation.",
        ));
    }
    Ok(
        json!({"gpu":"cuda","access_mode":"exclusive-lease-shared-host-driver","vram_mib":gpu["memory_mib"],
        "driver":gpu["driver"],"observed_at":stamp}),
    )
}
