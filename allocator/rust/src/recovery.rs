use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::{
    CheckpointStore, Controller, LeasePhase, Rejected,
    controller::{RETENTION, now},
    rows::{available_receipt, lease, phase, selected_receipt, sqlite_json, stored, text},
    tokens::{hash, token, valid_token},
};

const TICKET_LIFETIME: f64 = 300.0;

pub(crate) fn short_reason(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| (1..=240).contains(&value.chars().count()))
}

fn expiry_mismatch() -> Rejected {
    Rejected::new(
        "expiry_receipt_mismatch",
        "No matching expiry claim and verified removal.",
        409,
    )
}

fn verify_expiry(checkpoint: &Value, response: &Value) -> Result<(), Rejected> {
    if !checkpoint["purge_claim"].is_string()
        || response["claim"] != checkpoint["purge_claim"]
        || response["snapshot"] != checkpoint["snapshot"]
        || response["absent"] != true
    {
        return Err(expiry_mismatch());
    }
    Ok(())
}

fn save(
    db: &Connection,
    id: &str,
    checkpoint: Option<&str>,
    receipt: &Value,
) -> Result<(), Rejected> {
    if let Some(checkpoint) = checkpoint {
        db.execute(
            "UPDATE incremental_checkpoint SET receipt=?,error=NULL WHERE id=? AND lease_id=?",
            params![receipt.to_string(), checkpoint, id],
        )?;
    } else {
        db.execute(
            "UPDATE lease SET checkpoint=? WHERE id=?",
            params![receipt.to_string(), id],
        )?;
    }
    Ok(())
}

impl Controller {
    pub fn grant_upload(
        &mut self,
        id: &str,
        owner: &str,
        request: &Value,
    ) -> Result<Value, Rejected> {
        let endpoint = self.upload.clone();
        CheckpointStore::new(self.ledger()).grant(id, owner, request, endpoint.as_deref(), now())
    }

    pub fn redeem_upload(&mut self, request: &Value) -> Result<Value, Rejected> {
        if self.upload.is_none() {
            return Err(Rejected::new(
                "checkpoint_unavailable",
                "Incremental uploads are unavailable.",
                503,
            ));
        }
        let secret = request["ticket"].as_str().unwrap_or_default();
        CheckpointStore::new(self.ledger()).redeem(secret, now())
    }

    pub fn complete_upload(&mut self, request: &Value) -> Result<Value, Rejected> {
        let secret = request["ticket"]
            .as_str()
            .filter(|token| valid_token(token, 43, 43))
            .ok_or_else(|| {
                Rejected::new("ticket_unavailable", "Upload ticket is unavailable.", 404)
            })?;
        let released = self
            .database()
            .query_row(
                "SELECT l.released_at FROM incremental_checkpoint c JOIN lease l ON c.lease_id=l.id
            WHERE c.token_hash=? AND c.redeemed=1 AND c.upload_expires>?",
                params![hash(secret), now()],
                |row| row.get::<_, Option<f64>>(0),
            )
            .optional()?
            .flatten();
        let retention = released.map(|released| released + RETENTION);
        CheckpointStore::new(self.ledger()).complete(request, retention, now())
    }

    pub fn pin(&mut self, id: &str, owner: &str, request: &Value) -> Result<Value, Rejected> {
        let row = lease(self.database(), id, Some(owner))?;
        let key = request["checkpoint_id"].as_str();
        let mut receipt = selected_receipt(self.database(), &row, key)?;
        if receipt["storage_managed"] != true
            || (key.is_none() && phase(&row)? != LeasePhase::Released)
        {
            return Err(Rejected::new(
                "checkpoint_not_managed",
                "Only an available managed checkpoint can be pinned.",
                409,
            ));
        }
        if !receipt["purge_claim"].is_null() || !receipt["purged_at"].is_null() {
            return Err(Rejected::new(
                "checkpoint_expiring",
                "This checkpoint has already entered expiry.",
                409,
            ));
        }
        let pinned = request["pinned"]
            .as_bool()
            .filter(|pinned| !*pinned || short_reason(&request["reason"]).is_some())
            .ok_or_else(|| {
                Rejected::new(
                    "invalid_pin",
                    "Choose pin with a short reason, or unpin.",
                    400,
                )
            })?;
        if pinned {
            receipt["pin"] = json!({"reason":short_reason(&request["reason"]),"at":now()});
        } else if let Some(doc) = receipt.as_object_mut() {
            doc.remove("pin");
        }
        let row = self.ledger().mutate(|tx| {
            save(tx, id, key, &receipt)?;
            lease(tx, id, Some(owner))
        })?;
        self.public(&row, None)
    }

    pub fn finish_restore(
        &mut self,
        id: &str,
        owner: &str,
        request: &Value,
    ) -> Result<Value, Rejected> {
        self.ledger().mutate(|tx| {
            let row = lease(tx, id, Some(owner))?;
            let point = stored(&row["resume_point"])?;
            if !point.is_object()
                || !point["sha256"].is_string()
                || point["sha256"] != request["sha256"]
                || phase(&row)? != LeasePhase::Ready
            {
                return Err(Rejected::new(
                    "restore_mismatch",
                    "Restore acknowledgment must match the ready replacement.",
                    409,
                ));
            }
            tx.execute(
                "UPDATE lease SET restore_completed_at=coalesce(restore_completed_at,?) WHERE id=?",
                params![now(), id],
            )?;
            tx.execute("DELETE FROM restore_hold WHERE replacement=?", [id])?;
            Ok(json!({"restored":true}))
        })
    }

    pub fn recovery_ticket(
        &mut self,
        id: &str,
        owner: &str,
        request: &Value,
    ) -> Result<Value, Rejected> {
        let endpoint = self.recovery.clone().ok_or_else(|| {
            Rejected::new(
                "recovery_unavailable",
                "Checkpoint recovery is unavailable.",
                503,
            )
        })?;
        let selected = request["checkpoint_id"].as_str();
        let stamp = now();
        let secret = token()?;
        self.ledger().mutate(|tx|{
            let row=lease(tx,id,Some(owner))?;
            let receipt=available_receipt(tx,&row,selected,stamp,false)?;
            tx.execute("DELETE FROM recovery_ticket WHERE expires_at<=?",[stamp])?;
            tx.execute("DELETE FROM recovery_ticket WHERE lease_id=? AND redeemed=0",[id])?;
            tx.execute("INSERT INTO recovery_ticket(token_hash,lease_id,expires_at,checkpoint_id) VALUES (?,?,?,?)",
                params![hash(&secret),id,stamp+TICKET_LIFETIME,selected])?;
            Ok(json!({"endpoint":endpoint,"ticket":secret,"expires_at":stamp+TICKET_LIFETIME,"sha256":receipt["sha256"]}))
        })
    }

    pub fn redeem_recovery(&mut self, request: &Value) -> Result<Value, Rejected> {
        let failed = || Rejected::new("ticket_unavailable", "Recovery ticket is unavailable.", 404);
        let secret = request["ticket"]
            .as_str()
            .filter(|token| valid_token(token, 43, 43))
            .ok_or_else(failed)?;
        let hash = hash(secret);
        let stamp = now();
        self.ledger().mutate(|tx|{
            let ticket=tx.query_row("SELECT lease_id,checkpoint_id FROM recovery_ticket WHERE token_hash=? AND redeemed=0 AND expires_at>?",
                params![hash,stamp],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?))).optional()?.ok_or_else(failed)?;
            let row=lease(tx,&ticket.0,None)?;
            let receipt=available_receipt(tx,&row,ticket.1.as_deref(),stamp,true)?;
            tx.execute("UPDATE recovery_ticket SET redeemed=1,expires_at=? WHERE token_hash=?",params![stamp+TICKET_LIFETIME,hash])?;
            Ok(json!({"snapshot":receipt["snapshot"],"path":receipt["path"],"sha256":receipt["sha256"]}))
        })
    }

    pub fn claim_expiry(&mut self) -> Result<Value, Rejected> {
        if !self.purge {
            return Ok(json!({"expiry":null}));
        }
        let stamp = now();
        let record=self.database().query_row(
            "WITH artifacts AS (
                SELECT id AS lease_id,NULL AS checkpoint_id,storage_challenge AS challenge,checkpoint AS receipt
                    FROM lease WHERE phase='released'
                UNION ALL
                SELECT lease_id,id,id,receipt FROM incremental_checkpoint
             ) SELECT * FROM artifacts WHERE json_extract(receipt,'$.storage_managed')=1
                AND json_extract(receipt,'$.pin') IS NULL AND json_extract(receipt,'$.purged_at') IS NULL
                AND json_extract(receipt,'$.retain_through')<=?
                AND NOT EXISTS (SELECT 1 FROM restore_hold h WHERE h.source=artifacts.lease_id AND h.checkpoint_id IS artifacts.checkpoint_id)
                AND NOT EXISTS (SELECT 1 FROM recovery_ticket WHERE lease_id=artifacts.lease_id AND expires_at>?)
             ORDER BY json_extract(receipt,'$.retain_through'),lease_id,checkpoint_id LIMIT 1",
             params![stamp,stamp],sqlite_json).optional()?;
        let Some(record) = record else {
            return self.claim_unacknowledged(stamp);
        };
        let id = text(&record, "lease_id")?;
        let key = record["checkpoint_id"].as_str();
        let mut receipt = stored(&record["receipt"])?;
        if receipt["purge_claim"].is_null() {
            receipt["purge_claim"] = json!(uuid::Uuid::new_v4().simple().to_string());
            receipt["purge_started_at"] = json!(stamp);
            self.ledger().mutate(|tx| save(tx, id, key, &receipt))?;
        }
        Ok(
            json!({"expiry":{"lease_id":id,"challenge":record["challenge"],"checkpoint_id":key,
            "claim":receipt["purge_claim"],"snapshot":receipt["snapshot"],"path":receipt["path"]}}),
        )
    }

    fn claim_unacknowledged(&mut self, stamp: f64) -> Result<Value, Rejected> {
        let attempt=self.database().query_row(
            "WITH attempts AS (
               SELECT id AS source,NULL AS checkpoint_id,storage_challenge AS challenge,released_at
                    FROM lease WHERE phase='released' AND checkpoint IS NULL
               UNION ALL
               SELECT l.id,i.id,i.id,l.released_at FROM incremental_checkpoint i
                    JOIN lease l ON l.id=i.lease_id WHERE l.phase='released' AND i.receipt IS NULL
              ) SELECT a.*,e.claim FROM attempts a LEFT JOIN unacknowledged_expiry e USING(challenge)
                WHERE a.challenge IS NOT NULL AND e.purged_at IS NULL AND a.released_at<=?
                ORDER BY a.released_at LIMIT 1",[stamp-RETENTION],sqlite_json).optional()?;
        let Some(attempt) = attempt else {
            return Ok(json!({"expiry":null}));
        };
        let id = text(&attempt, "source")?;
        let challenge = text(&attempt, "challenge")?;
        let key = attempt["checkpoint_id"].as_str();
        let claim = attempt["claim"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
        self.ledger().mutate(|tx| {
            tx.execute(
                "INSERT OR IGNORE INTO unacknowledged_expiry VALUES (?,?,NULL)",
                params![challenge, claim],
            )?;
            Ok(())
        })?;
        let suffix = key.map_or_else(String::new, |key| format!("-{key}"));
        Ok(
            json!({"expiry":{"lease_id":id,"checkpoint_id":key,"challenge":challenge,"claim":claim,
            "snapshot":null,"unacknowledged":true,"path":format!("/lease-{id}{suffix}.tar.gz")}}),
        )
    }

    pub fn confirm_expiry(&mut self, response: &Value) -> Result<Value, Rejected> {
        let id = text(response, "lease_id")?;
        let row = lease(self.database(), id, None)?;
        let key = response["checkpoint_id"].as_str();
        if response["unacknowledged"] == true {
            if let Some(key) = key {
                selected_receipt(self.database(), &row, Some(key))?;
            }
            let challenge = key.or_else(|| row["storage_challenge"].as_str());
            if !self.purge || phase(&row)? != LeasePhase::Released || response["absent"] != true {
                return Err(expiry_mismatch());
            }
            return self.ledger().mutate(|tx|{
                let changed=tx.execute("UPDATE unacknowledged_expiry SET purged_at=coalesce(purged_at,?) WHERE challenge=? AND claim=?",
                    params![now(),challenge,response["claim"].as_str()])?;
                if changed!=1{return Err(expiry_mismatch());}
                Ok(json!({"absent":true}))
            });
        }
        let mut receipt = selected_receipt(self.database(), &row, key)?;
        if !self.purge {
            return Err(expiry_mismatch());
        }
        verify_expiry(&receipt, response)?;
        let row = if receipt["purged_at"].is_null() {
            receipt["purged_at"] = json!(now());
            self.ledger().mutate(|tx| {
                save(tx, id, key, &receipt)?;
                lease(tx, id, None)
            })?
        } else {
            row
        };
        self.public(&row, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expiry_acknowledgment_cannot_mark_a_different_snapshot_or_unverified_removal() {
        let checkpoint = json!({"purge_claim":"claim","snapshot":"snapshot"});
        assert!(
            verify_expiry(
                &checkpoint,
                &json!({"claim":"claim","snapshot":"snapshot","absent":true})
            )
            .is_ok()
        );
        assert!(
            verify_expiry(
                &checkpoint,
                &json!({"claim":"claim","snapshot":"different","absent":true})
            )
            .is_err()
        );
        assert!(
            verify_expiry(
                &checkpoint,
                &json!({"claim":"claim","snapshot":"snapshot","absent":false})
            )
            .is_err()
        );
    }
}
