use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

use crate::{
    Controller, LeasePhase, Rejected,
    controller::now,
    lifecycle::duration,
    recovery::short_reason,
    rows::{lease, phase, sqlite_json},
    tokens::{hash, token, valid_token},
};

fn unknown_delegation() -> Rejected {
    Rejected::new(
        "delegation_unavailable",
        "No valid renewal delegation.",
        401,
    )
}

fn hold_unavailable() -> Rejected {
    Rejected::new(
        "hold_unavailable",
        "Only an enrolled lease before reset can be held.",
        409,
    )
}

fn needs_duration(value: &Value) -> Result<u64, Rejected> {
    let seconds = value.as_u64().unwrap_or(0);
    duration(seconds)?;
    Ok(seconds)
}

pub(crate) fn mode(value: &Value) -> Result<&str, Rejected> {
    match value.get("mode") {
        None => Ok("disk"),
        Some(value) => value
            .as_str()
            .filter(|mode| matches!(*mode, "running" | "disk" | "clear"))
            .ok_or_else(|| Rejected::new("invalid_hold", "Select running, disk, or clear.", 400)),
    }
}

impl Controller {
    pub fn delegate(&mut self, id: &str, owner: &str, request: &Value) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        if request["revoke"] == true {
            return self.ledger().mutate(|tx| {
                tx.execute("DELETE FROM renewal_delegate WHERE lease_id=?", [id])?;
                Ok(json!({"revoked":true}))
            });
        }
        if !phase(&row)?.renewable() {
            return Err(Rejected::new(
                "lease_not_renewable",
                "Only a live or starting lease can delegate renewal.",
                409,
            ));
        }
        let window = needs_duration(&request["duration_seconds"])?;
        let extension = needs_duration(&request["extension_seconds"])?;
        if extension >= window {
            return Err(Rejected::new(
                "invalid_delegation",
                "The extension must be shorter than the delegation window.",
                400,
            ));
        }
        let stamp = now();
        let valid_until = stamp + window as f64;
        let credential = token()?;
        self.ledger().mutate(|tx|{
            tx.execute("INSERT INTO renewal_delegate VALUES (?,?,?,?)
                ON CONFLICT(lease_id) DO UPDATE SET token_hash=excluded.token_hash,valid_until=excluded.valid_until,extension=excluded.extension",
                params![id,hash(&credential),valid_until,extension as i64])?;
            Ok(json!({"token":credential,"valid_until":valid_until,"extension_seconds":extension}))
        })
    }

    pub fn delegated_renew(&mut self, id: &str, credential: &str) -> Result<Value, Rejected> {
        if !valid_token(credential, 43, 43) {
            return Err(unknown_delegation());
        }
        let stamp = now();
        let secret = hash(credential);
        self.ledger().mutate(|tx|{
            let row=tx.query_row("SELECT token_hash,valid_until,extension FROM renewal_delegate WHERE lease_id=?",[id],
                |row|Ok((row.get::<_,String>(0)?,row.get::<_,f64>(1)?,row.get::<_,i64>(2)?))).optional()?;
            let Some((valid,until,extension))=row.filter(|(valid,until,_)|*until>stamp
                &&bool::from(valid.as_bytes().ct_eq(secret.as_bytes())))else{return Err(unknown_delegation());};
            drop(valid);
            let lease=lease(tx,id,None)?;
            if !phase(&lease)?.renewable(){
                return Err(Rejected::new("lease_not_renewable","Reclamation has already begun.",409));
            }
            let previous=lease["deadline"].as_f64().ok_or_else(crate::rows::invalid_data)?;
            let next=previous.max((stamp+extension as f64).min(until));
            tx.execute("UPDATE lease SET deadline=? WHERE id=?",params![next,id])?;
            Ok(json!({"deadline":next,"delegated_until":until}))
        })
    }

    pub fn reviewer(&mut self, id: &str, owner: &str, request: &Value) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        if !matches!(
            phase(&row)?,
            LeasePhase::Starting | LeasePhase::Ready | LeasePhase::AwaitingExport
        ) || row["reclaim_on_expiry"] != 1
        {
            return Err(Rejected::new(
                "hold_unavailable",
                "Only an enrolled lease before reset accepts review.",
                409,
            ));
        }
        let target = match &request["reviewer"] {
            Value::Null => None,
            Value::String(value) if !value.is_empty() && value != owner => {
                let active: bool = self.database().query_row(
                    "SELECT EXISTS(SELECT 1 FROM task_credential WHERE owner=?
                    AND revoked_at IS NULL AND expires_at>?)",
                    params![value, now()],
                    |row| row.get(0),
                )?;
                if !self.principals.contains(value) && !active {
                    return Err(Rejected::new(
                        "invalid_reviewer",
                        "Choose an active task principal.",
                        400,
                    ));
                }
                Some(value.as_str())
            }
            _ => {
                return Err(Rejected::new(
                    "invalid_reviewer",
                    "Choose a separate existing task principal.",
                    400,
                ));
            }
        };
        if row["reviewer_hold"]
            .as_str()
            .is_some_and(|hold| !hold.is_empty())
            && target != row["reviewer"].as_str()
        {
            return Err(Rejected::new(
                "lease_held",
                "Resolve the designated reviewer hold before changing reviewers.",
                409,
            ));
        }
        let lease = self.ledger().mutate(|tx| {
            tx.execute(
                "UPDATE lease SET reviewer=? WHERE id=?",
                params![target, id],
            )?;
            lease(tx, id, Some(owner))
        })?;
        self.public(&lease, None)
    }

    pub fn override_reviewer(
        &mut self,
        id: &str,
        owner: &str,
        request: &Value,
    ) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        let reason = short_reason(&request["reason"]);
        if row["reviewer_hold"].as_str().is_none_or(str::is_empty)
            || request["confirm_reviewer"] != row["reviewer"]
            || reason.is_none()
        {
            return Err(Rejected::new(
                "review_confirmation_required",
                "Confirm the affected reviewer and give a reason.",
                409,
            ));
        }
        let decision = json!({"at":now(),"owner":owner,"reviewer":row["reviewer"],"hold":row["reviewer_hold"],
            "prior_reason":row["reviewer_reason"],"reason":reason,"disposition":"Remove reviewer hold; standard checkpoint policy applies."});
        let lease=self.ledger().mutate(|tx|{
            tx.execute("UPDATE lease SET reviewer=NULL,reviewer_hold=NULL,reviewer_reason=NULL,reviewer_override=? WHERE id=?",
                params![decision.to_string(),id])?;
            lease(tx,id,Some(owner))
        })?;
        self.public(&lease, None)
    }

    pub fn hold(
        &mut self,
        id: &str,
        principal: &str,
        request: &Value,
        delegated: bool,
    ) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, None)?;
        let identity = if delegated { "reviewer" } else { "owner" };
        if row[identity] != principal {
            return Err(crate::rows::no_lease());
        }
        let phase = phase(&row)?;
        if row["reclaim_on_expiry"] != 1
            || !matches!(phase, LeasePhase::Ready | LeasePhase::AwaitingExport)
        {
            return Err(hold_unavailable());
        }
        let action = mode(request)?;
        if action == "running" && phase != LeasePhase::Ready {
            return Err(Rejected::new(
                "already_stopped",
                "The guest has already stopped for export.",
                409,
            ));
        }
        let cleaned = if action == "clear" {
            None
        } else {
            Some(short_reason(&request["reason"]).ok_or_else(|| {
                Rejected::new("hold_reason_required", "Provide a short hold reason.", 400)
            })?)
        };
        let stored = (action != "clear").then_some(action);
        let changed = self.ledger().mutate(|tx| {
            let statement = if delegated {
                "UPDATE lease SET reviewer_hold=?,reviewer_reason=? WHERE id=?"
            } else {
                "UPDATE lease SET hold_mode=?,hold_reason=? WHERE id=?"
            };
            tx.execute(statement, params![stored, cleaned, id])?;
            lease(tx, id, None)
        })?;
        if delegated {
            Ok(json!({"id":id,"hold":action,"reviewer":principal}))
        } else {
            self.public(&changed, None)
        }
    }

    pub fn reservations(&self, owner: &str) -> Result<Value, Rejected> {
        let mut statement=self.database().prepare("SELECT * FROM capacity_reservation WHERE owner=? AND released_at IS NULL ORDER BY created_at,id")?;
        let records = statement
            .query_map([owner], sqlite_json)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({"reservations":records}))
    }

    pub fn reserve(&mut self, owner: &str, request: &Value) -> Result<Value, Rejected> {
        let invalid = || {
            Rejected::new(
                "invalid_reservation",
                "Provide a key, purpose and nonzero memory or storage demand.",
                400,
            )
        };
        let slot = request
            .get("slot_id")
            .map_or(Some("primary"), Value::as_str)
            .filter(|slot| slot.len() <= 32 && crate::request::dependency(slot))
            .ok_or_else(|| {
                Rejected::new(
                    "invalid_slot_id",
                    "Provide the configured slot identifier for this reservation.",
                    400,
                )
            })?;
        let sizes = |key: &str| {
            request[key]
                .as_i64()
                .filter(|amount| (0..1_i64 << 53).contains(amount))
                .ok_or_else(invalid)
        };
        let (memory, disk) = (sizes("memory_mib")?, sizes("storage_mib")?);
        let purpose = short_reason(&request["purpose"])
            .filter(|_| memory > 0 || disk > 0)
            .ok_or_else(invalid)?;
        let key = request["request_key"]
            .as_str()
            .filter(|key| (1..=128).contains(&key.chars().count()))
            .ok_or_else(invalid)?;
        let previous = self
            .database()
            .query_row(
                "SELECT * FROM capacity_reservation WHERE owner=? AND request_key=?",
                params![owner, key],
                sqlite_json,
            )
            .optional()?;
        if let Some(previous) = previous {
            if previous["memory_mib"] != memory
                || previous["storage_mib"] != disk
                || previous["purpose"] != purpose
                || previous["slot_id"] != slot
            {
                return Err(Rejected::new(
                    "reservation_key_conflict",
                    "This key already identifies a different reservation.",
                    409,
                ));
            }
            return Ok(previous);
        }
        let active: bool = self.database().query_row(
            "SELECT EXISTS(SELECT 1 FROM lease WHERE slot_id=? AND phase!='released')",
            [slot],
            |row| row.get(0),
        )?;
        let (reserved_memory, reserved_disk) = self.totals(slot)?;
        let memory_total = memory.checked_add(reserved_memory).ok_or_else(invalid)?;
        let disk_total = disk.checked_add(reserved_disk).ok_or_else(invalid)?;
        self.provider_call(json!({"action":"reservation-capacity","slot_id":slot,"active":active,"reservations":[memory_total,disk_total]}))?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.ledger().mutate(|tx|{
            tx.execute("INSERT INTO capacity_reservation(id,owner,request_key,memory_mib,storage_mib,purpose,created_at,slot_id)
                VALUES(?,?,?,?,?,?,?,?)",params![id,owner,key,memory,disk,purpose,now(),slot])?;
            Ok(tx.query_row("SELECT * FROM capacity_reservation WHERE id=?",[id],sqlite_json)?)
        })
    }

    pub fn cancel_reservation(&mut self, owner: &str, id: &str) -> Result<Value, Rejected> {
        self.ledger().mutate(|tx|{
            let changed=tx.execute("UPDATE capacity_reservation SET released_at=coalesce(released_at,?) WHERE id=? AND owner=?",
                params![now(),id,owner])?;
            if changed!=1{return Err(Rejected::new("reservation_unknown","No matching reservation for this principal.",404));}
            Ok(tx.query_row("SELECT * FROM capacity_reservation WHERE id=?",[id],sqlite_json)?)
        })
    }
}
