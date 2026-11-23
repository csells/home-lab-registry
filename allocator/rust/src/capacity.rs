use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{
    Rejected,
    proxmox::{GuestPin, Proxmox, api_id, as_list, config_text, escape, identifier, truthy},
    proxmox_dynamic::{DynamicScope, Shape},
    tokens::hex,
};

pub(crate) const MIB: i64 = 1024 * 1024;

pub(crate) struct Capacity {
    pub storage: String,
    memory_floor: i64,
    storage_floor: i64,
    other_memory: i64,
    protected: BTreeSet<String>,
}

pub(crate) struct Observation {
    pub shape: Shape,
    pub memory: i64,
    pub storage: i64,
}

fn unverified() -> Rejected {
    Rejected::new(
        "capacity_unverified",
        "Fresh host capacity could not be verified. Try again later.",
        503,
    )
}

fn bytes(value: &Value) -> Option<i64> {
    value.as_i64().filter(|value| *value >= 0)?.checked_mul(MIB)
}

fn positive(value: &Value) -> Result<i64, Rejected> {
    value
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(unverified)
}

impl Capacity {
    pub fn open(config: &Value, guest: &str) -> Result<Self, Rejected> {
        let invalid = || {
            Rejected::new(
                "provider_configuration_invalid",
                "Configure the pinned storage, capacity floors and protected guest IDs.",
                503,
            )
        };
        let storage = config_text(config, "storage")?;
        let protected = config["protected_guest_ids"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|id| api_id(id).ok_or_else(invalid))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if !identifier(storage) || protected.is_empty() || protected.contains(guest) {
            return Err(invalid());
        }
        Ok(Self {
            storage: storage.to_owned(),
            memory_floor: bytes(&config["minimum_memory_mib"]).ok_or_else(invalid)?,
            storage_floor: bytes(&config["minimum_storage_mib"]).ok_or_else(invalid)?,
            other_memory: bytes(&config["other_reserved_memory_mib"]).ok_or_else(invalid)?,
            protected,
        })
    }

    fn calculate(
        &self,
        total: i64,
        free: i64,
        committed: i64,
        storage: i64,
        slot_memory: i64,
        slot_disk: i64,
    ) -> Result<(i64, i64), Rejected> {
        let allowed = free
            .min(total.checked_sub(committed).ok_or_else(unverified)?)
            .checked_sub(self.other_memory)
            .and_then(|value| value.checked_sub(slot_memory))
            .ok_or_else(unverified)?;
        Ok((
            allowed,
            storage.checked_sub(slot_disk).ok_or_else(unverified)?,
        ))
    }

    pub fn admit(
        &self,
        observed: (i64, i64),
        reservations: [i64; 2],
    ) -> Result<(i64, i64), Rejected> {
        let total = |available: i64, reserved: i64| {
            reserved
                .checked_mul(MIB)
                .and_then(|value| available.checked_sub(value))
                .ok_or_else(unverified)
        };
        let memory = total(observed.0, reservations[0])?;
        let storage = total(observed.1, reservations[1])?;
        for (available, minimum, label, code) in [
            (memory, self.memory_floor, "memory", "host_memory_headroom"),
            (
                storage,
                self.storage_floor,
                "storage",
                "host_storage_headroom",
            ),
        ] {
            if available < minimum {
                return Err(Rejected::new(
                    code,
                    format!(
                        "This request would leave {} MiB of host {label} headroom; {} MiB is required.",
                        available.max(0) / MIB,
                        minimum / MIB
                    ),
                    409,
                ));
            }
        }
        Ok((memory, storage))
    }

    pub fn observe(
        &self,
        api: &Proxmox,
        kind: Pool<'_>,
        require_slot: bool,
        allow_running: bool,
        requested: Option<Shape>,
    ) -> Result<Observation, Rejected> {
        let node = format!("/nodes/{}", api.node);
        let facts = api
            .audit(&format!("{node}/status"))
            .map_err(|_| unverified())?;
        let rows = as_list(
            api.audit("/cluster/resources?type=vm")
                .map_err(|_| unverified())?,
        )
        .map_err(|_| unverified())?;
        let store = api
            .audit(&format!("{node}/storage/{}/status", escape(&self.storage)))
            .map_err(|_| unverified())?;
        let mut guests = BTreeMap::new();
        for row in rows {
            if row["node"] != api.node {
                continue;
            }
            let id = api_id(&row["vmid"]).ok_or_else(unverified)?;
            if !matches!(row["status"].as_str(), Some("running" | "stopped"))
                || guests.insert(id, row).is_some()
            {
                return Err(unverified());
            }
        }
        if !self.protected.iter().all(|id| guests.contains_key(id)) {
            return Err(unverified());
        }
        let slot = guests.get(&api.guest_id);
        let (offered, admission) = match kind {
            Pool::Dynamic(scope) => {
                if let Some(slot) = slot {
                    let named = slot["name"]
                        .as_str()
                        .and_then(|name| name.strip_prefix("lease-"))
                        .is_some_and(|name| hex(name, 32));
                    if !allow_running || truthy(&slot["template"]) || !named {
                        return Err(unverified());
                    }
                }
                scope.preflight_storage(&store)?;
                (scope.maximum, requested.unwrap_or(scope.default))
            }
            Pool::Fixed(pin) => {
                let slot = slot.ok_or_else(unverified)?;
                if slot["name"] != pin.name
                    || truthy(&slot["template"])
                    || (slot["status"] != "stopped" && !allow_running)
                {
                    return Err(unverified());
                }
                let shape = Shape {
                    memory_mib: positive(&slot["maxmem"])? / MIB,
                    disk_mib: positive(&slot["maxdisk"])? / MIB,
                    vcpus: positive(&slot["maxcpu"])?,
                };
                (shape, shape)
            }
        };
        let mut committed = 0_i64;
        for (id, guest) in guests.iter() {
            if guest["status"] == "running" || self.protected.contains(id) {
                committed = committed
                    .checked_add(positive(&guest["maxmem"])?)
                    .ok_or_else(unverified)?;
            }
        }
        let total = positive(&facts["memory"]["total"])?;
        let free = positive(&facts["memory"]["available"])?;
        if free > total
            || !truthy(&store["active"])
            || !truthy(&store["enabled"])
            || store.get("shared").is_none_or(truthy)
        {
            return Err(unverified());
        }
        let available = positive(&store["avail"])?;
        let (memory, storage) = self.calculate(
            total,
            free,
            committed,
            available,
            if require_slot && slot.is_none_or(|row| row["status"] == "stopped") {
                admission
                    .memory_mib
                    .checked_mul(MIB)
                    .ok_or_else(unverified)?
            } else {
                0
            },
            if require_slot {
                admission.disk_mib.checked_mul(MIB).ok_or_else(unverified)?
            } else {
                0
            },
        )?;
        Ok(Observation {
            shape: offered,
            memory,
            storage,
        })
    }
}

pub(crate) enum Pool<'a> {
    Fixed(&'a GuestPin),
    Dynamic(&'a DynamicScope),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projected_memory_preserves_stopped_protected_guests_and_applies_reservations() {
        let pool = Capacity {
            storage: "scoped".into(),
            memory_floor: 100 * MIB,
            storage_floor: 50 * MIB,
            other_memory: 50 * MIB,
            protected: BTreeSet::new(),
        };
        let available = pool
            .calculate(
                1000 * MIB,
                800 * MIB,
                400 * MIB,
                450 * MIB,
                200 * MIB,
                300 * MIB,
            )
            .unwrap();
        assert_eq!(available, (350 * MIB, 150 * MIB));
        assert!(pool.admit(available, [250, 90]).is_ok());
        assert_eq!(
            pool.admit(available, [251, 90]).unwrap_err().code(),
            "host_memory_headroom"
        );
        assert_eq!(
            pool.admit(available, [250, 101]).unwrap_err().code(),
            "host_storage_headroom"
        );
    }
}
