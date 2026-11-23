use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::{Rejected, lifecycle::duration, protocol::{APPLE, LINUX, public_key}, tokens::hex};

#[derive(Clone, Debug)]
pub struct AcquireRequest {
    pub task: String,
    pub request_key: String,
    pub seconds: u64,
    pub ssh_key: String,
    pub requirements: Value,
    pub requirements_explicit: bool,
    pub recipe: Option<String>,
    pub resume_from: Option<String>,
    pub checkpoint_id: Option<String>,
    pub reclaim: bool,
    pub reclaim_explicit: bool,
}

impl AcquireRequest {
    pub fn parse(data: &Value, reclaim_available: bool) -> Result<Self, Rejected> {
        let short = |key: &str| {
            data[key]
                .as_str()
                .filter(|value| (1..=128).contains(&value.chars().count()))
                .map(str::to_owned)
                .ok_or_else(|| {
                    Rejected::new(
                        "invalid_request",
                        "Provide a task and a request_key of 1–128 characters.",
                        400,
                    )
                })
        };
        let task = short("task")?;
        let request_key = short("request_key")?;
        let seconds = data["seconds"].as_u64().unwrap_or(0);
        duration(seconds)?;
        let reclaim = data
            .get("reclaim_on_expiry")
            .map_or(Some(true), Value::as_bool)
            .filter(|requested| !requested || reclaim_available)
            .ok_or_else(|| {
                Rejected::new(
                    "reclamation_unavailable",
                    "Automatic reclamation is unavailable for this request.",
                    422,
                )
            })?;
        let ssh_key = public_key(&data["ssh_public_key"])?;
        let selected = data
            .get("requirements")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let requirements = serde_json::from_str(&canonical_requirements(&selected)?)
            .expect("requirements were serialized immediately before parsing");
        let recipe = match &data["recipe"] {
            Value::Null => None,
            Value::String(value) if matches!(value.as_str(), LINUX | APPLE) => Some(value.clone()),
            _ => {
                return Err(Rejected::new(
                    "unsupported_recipe",
                    "Choose a recipe advertised by the allocator.",
                    422,
                ));
            }
        };
        let resume_from = match &data["resume_from"] {
            Value::Null => None,
            Value::String(value) if hex(value, 32) => Some(value.clone()),
            _ => {
                return Err(Rejected::new(
                    "invalid_source",
                    "Provide the predecessor lease ID.",
                    400,
                ));
            }
        };
        if !data["checkpoint_id"].is_null() && resume_from.is_none() {
            return Err(Rejected::new(
                "invalid_source",
                "Select a predecessor lease for this checkpoint.",
                400,
            ));
        }
        let checkpoint_id = match &data["checkpoint_id"] {
            Value::Null => None,
            Value::String(value) if hex(value, 32) => Some(value.clone()),
            _ => {
                return Err(Rejected::new(
                    "invalid_checkpoint",
                    "A checkpoint ID must be 32 lowercase hex characters.",
                    400,
                ));
            }
        };
        Ok(Self {
            task,
            request_key,
            seconds,
            ssh_key,
            requirements,
            recipe,
            resume_from,
            checkpoint_id,
            reclaim,
            requirements_explicit: data.get("requirements").is_some(),
            reclaim_explicit: data.get("reclaim_on_expiry").is_some(),
        })
    }

    pub fn key(&self, owner: &str) -> String {
        format!("{owner}/{}", self.request_key)
    }

    pub fn preferred_recipe(&self) -> &str {
        self.recipe.as_deref().unwrap_or_else(|| {
            if self.requirements["os"] == "macos" {
                APPLE
            } else {
                LINUX
            }
        })
    }

    pub fn validate_new(&self) -> Result<(), Rejected> {
        if self.reclaim {
            return Ok(());
        }
        Err(Rejected::new(
            "reclamation_required",
            "Every new lease uses checkpoint-gated reclamation.",
            422,
        ))
    }
}

pub(crate) fn dependency(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

pub fn canonical_requirements(value: &Value) -> Result<String, Rejected> {
    let raw = value.as_object().ok_or_else(|| {
        Rejected::new(
            "invalid_requirements",
            "Provide a requirements object.",
            400,
        )
    })?;
    let mut normalized = raw.clone();
    for (key, entry) in raw {
        match key.as_str() {
            "memory_mib" | "disk_mib" | "vcpus" => {
                if !entry.as_u64().is_some_and(|value| value > 0) {
                    return Err(Rejected::new(
                        "invalid_requirements",
                        format!("{key} must be a positive integer."),
                        400,
                    ));
                }
            }
            "gpu" | "os" | "architecture" | "distribution" => {
                if !entry.as_str().is_some_and(|value| !value.is_empty()) {
                    return Err(Rejected::new(
                        "invalid_requirements",
                        format!("{key} must be a nonempty string."),
                        400,
                    ));
                }
            }
            "dependencies" => {
                let names = entry
                    .as_array()
                    .filter(|values| values.len() <= 16)
                    .and_then(|values| {
                        values
                            .iter()
                            .map(|value| value.as_str().filter(|name| dependency(name)))
                            .collect::<Option<BTreeSet<_>>>()
                            .filter(|names| names.len() == values.len())
                    })
                    .ok_or_else(|| {
                        Rejected::new(
                            "invalid_requirements",
                            "Provide up to 16 distinct shared dependency names.",
                            400,
                        )
                    })?;
                normalized.insert(key.clone(), json!(names));
            }
            _ => {
                return Err(Rejected::new(
                    "unsupported_capability",
                    format!("This pool cannot qualify requirement {key:?}."),
                    422,
                ));
            }
        }
    }
    Ok(Value::Object(normalized).to_string())
}
