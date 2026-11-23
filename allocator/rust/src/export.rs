use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use crate::{
    Controller, LeasePhase, ProviderKind, Rejected,
    controller::{EXPORT_REVIEW, now, provider_shape},
    recovery::short_reason,
    rows::{lease, phase, sqlite_json, stored},
    tokens::{hex, valid_hash},
};

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
    pub fn next_export(&mut self) -> Result<Value, Rejected> {
        let selected=self.database().query_row("SELECT id FROM lease WHERE phase='awaiting-export' AND checkpoint IS NULL
            AND (export_failed_at IS NULL OR export_failed_at>?)
            ORDER BY export_last_attempt_at IS NOT NULL,coalesce(export_last_attempt_at,export_requested_at),id LIMIT 1",
            [now()-EXPORT_REVIEW],|row|row.get::<_,String>(0)).optional()?;
        let export = if let Some(id) = selected {
            self.pending_export(&id)?
        } else {
            Value::Null
        };
        Ok(json!({"export":export}))
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
