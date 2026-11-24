use std::{fs, path::Path};

use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    Controller, LeasePhase, Provider, ProviderKind, Rejected,
    controller::{EXPORT_REVIEW, now, provider_shape},
    recovery::short_reason,
    rows::{lease, phase, sqlite_json, stored},
    tokens::{hex, valid_hash},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportReconciliation {
    lease_id: String,
    additional_start: String,
    workspace_sha256: String,
    reason: String,
}

fn invalid_reconciliation() -> Rejected {
    Rejected::new(
        "invalid_export_reconciliation",
        "Provide the exact lease, additional Proxmox start task, verified workspace archive digest and reason.",
        400,
    )
}

pub fn reconcile_final_export(config: &Path, proof: &Path) -> Result<Value, Rejected> {
    let invalid_config = || {
        Rejected::new("configuration_invalid", "Read the allocator configuration.", 503)
    };
    let settings = fs::read(config).map_err(|_| invalid_config())?;
    let settings = serde_json::from_slice(&settings).map_err(|_| invalid_config())?;
    let proof = fs::read(proof).map_err(|_| invalid_reconciliation())?;
    let proof: ExportReconciliation =
        serde_json::from_slice(&proof).map_err(|_| invalid_reconciliation())?;
    let reason = proof.reason.trim();
    if !hex(&proof.lease_id, 32)
        || !valid_hash(&proof.workspace_sha256)
        || !proof.additional_start.starts_with("UPID:")
        || proof.additional_start.len() > 256
        || !proof.additional_start.bytes().all(|byte| byte.is_ascii_graphic())
        || !(1..=240).contains(&reason.chars().count())
    {
        return Err(invalid_reconciliation());
    }
    let provider = Provider::native(&settings)?;
    Controller::open(&settings, provider)?.reconcile_export_source(&proof)
}

fn final_receipt(id: &str, value: &Value, stamp: f64) -> Result<Value, Rejected> {
    if value["outcome"] != "verified"
        || !value["snapshot"].as_str().is_some_and(valid_hash)
        || value["path"] != format!("/lease-{id}.tar.gz")
        || !value["sha256"].as_str().is_some_and(valid_hash)
    {
        return Err(Rejected::new(
            "invalid_export_receipt",
            "Provide a verified encrypted snapshot and download digest.",
            400,
        ));
    }
    if value["storage_managed"] != true || value["restore_verified"] != true {
        return Err(Rejected::new(
            "invalid_export_receipt",
            "Final export requires managed storage and verified workspace restore.",
            400,
        ));
    }
    Ok(
        json!({"snapshot":value["snapshot"],"path":value["path"],"sha256":value["sha256"],"verified_at":stamp,
        "storage_managed":true,"restore_verified":true}),
    )
}

fn pending(row: &Value, stamp: f64) -> Result<(), Rejected> {
    if phase(row)? != LeasePhase::AwaitingExport || !row["checkpoint"].is_null() {
        return Err(Rejected::new(
            "export_not_pending",
            "This lease is not waiting for a final export.",
            409,
        ));
    }
    if row["export_failed_at"]
        .as_f64()
        .is_some_and(|failed| stamp >= failed + EXPORT_REVIEW)
    {
        return Err(Rejected::new(
            "export_decision_required",
            "Export has failed for 24 hours. Owner disposition is required.",
            409,
        ));
    }
    Ok(())
}

impl Controller {
    fn reconcile_export_source(&mut self, proof: &ExportReconciliation) -> Result<Value, Rejected> {
        let id = &proof.lease_id;
        let row = lease(self.database(), id, None)?;
        let entry = json!({
            "lease_id": id,
            "additional_start": proof.additional_start,
            "workspace_sha256": proof.workspace_sha256,
            "reason": proof.reason.trim(),
        });
        let previous = stored(&row["export_reconciliation"])?;
        if previous["proof"] == entry {
            return Ok(previous);
        }
        if !previous.is_null()
            || phase(&row)? != LeasePhase::AwaitingExport
            || !row["checkpoint"].is_null()
            || !row["export_disposition"].is_null()
            || self.slot(&row)?.kind == ProviderKind::Tart
        {
            return Err(Rejected::new(
                "export_reconciliation_unavailable",
                "Only an unresolved stopped Proxmox export without a prior reconciliation or owner disposition can be reconciled.",
                409,
            ));
        }
        self.ledger().verify()?;
        let mut query = provider_shape(&row, "export-source")?;
        let original_starts = stored(&row["export_start_tasks"])?;
        query["start_tasks"] = original_starts.clone();
        query["additional_start"] = json!(proof.additional_start);
        let verified = self.provider_call(query)?;
        let challenge = uuid::Uuid::new_v4().simple().to_string();
        let receipt = json!({
            "proof": entry,
            "reconciled_at": now(),
            "original_start_tasks": original_starts,
            "verified_start_tasks": verified["start_tasks"],
        });
        self.ledger().mutate(|tx| {
            tx.execute(
                "UPDATE lease SET export_start_tasks=?,export_reconciliation=?,
                    export_challenge=?,export_last_attempt_at=NULL,
                    export_failed_at=NULL,export_error=NULL WHERE id=?",
                params![verified["start_tasks"].to_string(),receipt.to_string(),challenge,id],
            )?;
            Ok(())
        })?;
        Ok(receipt)
    }

    pub fn next_export(&mut self) -> Result<Value, Rejected> {
        let eligible = self
            .database()
            .prepare("SELECT id FROM lease WHERE phase='awaiting-export' AND checkpoint IS NULL
                AND (export_failed_at IS NULL OR export_failed_at>?)
                ORDER BY export_last_attempt_at IS NOT NULL,coalesce(export_last_attempt_at,export_requested_at),id")?
            .query_map([now() - EXPORT_REVIEW], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut blocked = None;
        for id in eligible {
            match self.pending_export(&id) {
                Ok(export) => return Ok(json!({"export":export})),
                Err(problem) => {
                    tracing::warn!(lease = id, code = problem.code(), "Final export remains held");
                    blocked.get_or_insert(problem);
                }
            }
        }
        match blocked {
            Some(problem) => Err(problem),
            None => Ok(json!({"export":null})),
        }
    }

    pub fn pending_export(&mut self, id: &str) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, None)?;
        pending(&row, now())?;
        let verified = self.check_source(&row)?;
        let slot = self.slot(&row)?;
        let mut result = json!({"lease_id":id,"challenge":row["export_challenge"],"storage_challenge":row["storage_challenge"],
            "slot_id":row["slot_id"],"guest_id":slot.guest_id,"requested_at":row["export_requested_at"]});
        if slot.kind == ProviderKind::Dynamic {
            result["dynamic_source"] = verified["dynamic_source"].clone();
        }
        if slot.kind == ProviderKind::Tart {
            result["workspace_source"] = verified["workspace_source"].clone();
        }
        Ok(result)
    }

    pub fn export_result(&mut self, id: &str, value: &Value) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, None)?;
        let prior = stored(&row["checkpoint"])?;
        if prior.is_object() {
            let same = value["outcome"] == "verified"
                && value["challenge"] == row["export_challenge"]
                && ["snapshot", "path", "sha256", "storage_managed"]
                    .into_iter()
                    .all(|field| value[field] == prior[field]);
            if same {
                return self.public(&row, None);
            }
            return Err(Rejected::new(
                "export_already_verified",
                "A different receipt cannot replace a verified export.",
                409,
            ));
        }
        let stamp = now();
        pending(&row, stamp)?;
        if value["challenge"] != row["export_challenge"] {
            return Err(Rejected::new(
                "export_identity_mismatch",
                "Receipt did not match the current export challenge.",
                409,
            ));
        }
        self.check_source(&row)?;
        let alerts = self.alerts;
        let row = if value["outcome"] == "failed" {
            let reason = short_reason(&value["reason"]).ok_or_else(|| {
                Rejected::new(
                    "invalid_export_error",
                    "Provide a short failure reason.",
                    400,
                )
            })?;
            self.ledger().mutate(|tx|{
                tx.execute("UPDATE lease SET export_failed_at=coalesce(export_failed_at,?),export_error=?,export_last_attempt_at=? WHERE id=?",
                    params![stamp,reason,stamp,id])?;
                if alerts{
                    tx.execute("INSERT OR IGNORE INTO export_alert SELECT ?,export_challenge,id,owner,export_failed_at,NULL FROM lease WHERE id=?",
                        params![uuid::Uuid::new_v4().simple().to_string(),id])?;
                }
                lease(tx,id,None)
            })?
        } else {
            let receipt = final_receipt(id, value, stamp)?;
            self.ledger().mutate(|tx|{
                tx.execute("UPDATE lease SET checkpoint=?,export_failed_at=NULL,export_error=NULL WHERE id=?",
                    params![receipt.to_string(),id])?;
                lease(tx,id,None)
            })?
        };
        self.public(&row, None)
    }

    pub fn retry_export(&mut self, id: &str, owner: &str) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        if phase(&row)? != LeasePhase::AwaitingExport
            || !row["checkpoint"].is_null()
            || row["export_failed_at"].is_null()
        {
            return Err(Rejected::new(
                "export_retry_unavailable",
                "Only a failed pending export can be retried.",
                409,
            ));
        }
        self.check_source(&row)?;
        let challenge = uuid::Uuid::new_v4().simple().to_string();
        let changed=self.ledger().mutate(|tx|{
            tx.execute("UPDATE lease SET export_challenge=?,export_failed_at=NULL,export_error=NULL,export_disposition=NULL WHERE id=?",
                params![challenge,id])?;
            lease(tx,id,Some(owner))
        })?;
        self.public(&changed, None)
    }

    pub fn export_disposition(
        &mut self,
        id: &str,
        owner: &str,
        request: &Value,
    ) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        let mode = request["mode"]
            .as_str()
            .filter(|mode| matches!(*mode, "retain" | "discard"));
        let Some((mode, reason)) = mode.zip(short_reason(&request["reason"])) else {
            return Err(Rejected::new(
                "invalid_disposition",
                "Choose retain or discard and give a short reason.",
                400,
            ));
        };
        if mode == "discard" && request["confirm_lease"] != id {
            return Err(Rejected::new(
                "discard_confirmation_required",
                "Explicitly confirm the exact lease ID.",
                400,
            ));
        }
        let previous = stored(&row["export_disposition"])?;
        if previous["mode"] == "discard" {
            return self.inspect(id, owner);
        }
        let stamp = now();
        let unresolved = phase(&row)? == LeasePhase::AwaitingExport
            && row["checkpoint"].is_null()
            && row["export_failed_at"]
                .as_f64()
                .is_some_and(|failure| stamp >= failure + EXPORT_REVIEW);
        if !unresolved {
            return Err(Rejected::new(
                "disposition_unavailable",
                "This lease has no unresolved 24-hour export failure.",
                409,
            ));
        }
        let decision = json!({"mode":mode,"reason":reason,"at":stamp}).to_string();
        if mode == "retain" {
            let changed=self.ledger().mutate(|tx|{
                tx.execute("UPDATE lease SET hold_mode='disk',hold_reason=?,export_disposition=? WHERE id=?",
                    params![reason,decision,id])?;
                lease(tx,id,Some(owner))
            })?;
            return self.public(&changed, None);
        }
        if row["reviewer_hold"]
            .as_str()
            .is_some_and(|hold| !hold.is_empty())
        {
            return Err(Rejected::new(
                "lease_held",
                "Resolve the designated reviewer hold before discarding this disk.",
                409,
            ));
        }
        self.check_source(&row)?;
        let phase = if self.slot(&row)?.kind == ProviderKind::Fixed {
            "resetting"
        } else {
            "deleting"
        };
        let mut query = provider_shape(&row, "task-history")?;
        query["phase"] = json!(phase);
        let history = self.provider_call(query)?;
        let changed=self.ledger().mutate(|tx|{
            tx.execute("UPDATE lease SET phase=?,upstream=NULL,prior_tasks=?,export_disposition=?,hold_mode=NULL,hold_reason=NULL WHERE id=?",
                params![phase,history.to_string(),decision,id])?;
            lease(tx,id,Some(owner))
        })?;
        let dispatched = self.dispatch_recorded(&changed)?;
        self.public(&dispatched, None)
    }

    pub fn export_alert(&self) -> Result<Value, Rejected> {
        let alert = self
            .database()
            .query_row(
                "SELECT id,lease_id,owner,failed_at FROM export_alert
            WHERE delivered_at IS NULL ORDER BY failed_at,id LIMIT 1",
                [],
                sqlite_json,
            )
            .optional()?;
        Ok(json!({"alert":alert}))
    }

    pub fn acknowledge_alert(&mut self, request: &Value) -> Result<Value, Rejected> {
        let id = request["alert_id"]
            .as_str()
            .filter(|value| hex(value, 32))
            .ok_or_else(|| Rejected::new("invalid_alert", "Provide a valid alert ID.", 400))?;
        self.ledger().mutate(|tx| {
            let count = tx.execute(
                "UPDATE export_alert SET delivered_at=coalesce(delivered_at,?) WHERE id=?",
                params![now(), id],
            )?;
            if count != 1 {
                return Err(Rejected::new(
                    "unknown_alert",
                    "No matching export alert.",
                    404,
                ));
            }
            Ok(json!({"delivered":true}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_receipt_requires_the_exact_storage_object_and_a_verified_restore() {
        let lease = "a".repeat(32);
        let mut input = json!({"outcome":"verified","snapshot":"d".repeat(64),"path":format!("/lease-{lease}.tar.gz"),
                            "sha256":"e".repeat(64),"storage_managed":true,"restore_verified":true});
        assert_eq!(
            final_receipt(&lease, &input, 100.0).unwrap()["verified_at"],
            100.0
        );
        input["restore_verified"] = json!(false);
        assert_eq!(
            final_receipt(&lease, &input, 100.0).unwrap_err().code(),
            "invalid_export_receipt"
        );
        input["restore_verified"] = json!(true);
        input["path"] = json!("/lease-another.tar.gz");
        assert_eq!(
            final_receipt(&lease, &input, 100.0).unwrap_err().code(),
            "invalid_export_receipt"
        );
    }
}
