use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use crate::{
    Ledger, Rejected,
    tokens::{hash, token, valid_token},
};

pub struct TaskCredentials<'a, G> {
    ledger: &'a mut Ledger<G>,
    maximum: u64,
}

pub(crate) fn task_owner(
    connection: &Connection,
    secret: &str,
    now: f64,
) -> Result<Option<String>, Rejected> {
    if !valid_token(secret, 43, 43) {
        return Ok(None);
    }
    Ok(connection.query_row("SELECT owner FROM task_credential WHERE token_hash=? AND revoked_at IS NULL AND expires_at>?",
        params![hash(secret),now],|row|row.get(0)).optional()?)
}

impl<'a, G: FnMut() -> Result<String, Rejected>> TaskCredentials<'a, G> {
    pub fn new(ledger: &'a mut Ledger<G>, maximum: u64) -> Result<Self, Rejected> {
        if maximum == 0 {
            return Err(Rejected::new(
                "credential_unavailable",
                "Configure a positive task credential lifetime.",
                503,
            ));
        }
        Ok(Self { ledger, maximum })
    }

    pub fn owner(&self, secret: &str, now: f64) -> Result<Option<String>, Rejected> {
        task_owner(self.ledger.connection(), secret, now)
    }

    pub fn issue(&mut self, issuer: &str, request: &Value, now: f64) -> Result<Value, Rejected> {
        let task = request["task_ref"]
            .as_str()
            .filter(|task| valid_token(task, 1, 96))
            .ok_or_else(|| {
                Rejected::new(
                    "invalid_task_ref",
                    "Supply a task reference for this issuer.",
                    400,
                )
            })?;
        let owner = format!("issued:{issuer}:{task}");
        if request["revoke"] == true {
            return self.ledger.mutate(|tx|{
                tx.execute("UPDATE task_credential SET revoked_at=coalesce(revoked_at,?) WHERE owner=?",
                    params![now,owner])?;
                tx.execute("DELETE FROM renewal_delegate WHERE lease_id IN (SELECT id FROM lease WHERE owner=?)",[&owner])?;
                Ok(json!({"revoked":true,"principal":owner}))
            });
        }
        let duration = request["seconds"]
            .as_u64()
            .filter(|seconds| (1..=self.maximum).contains(seconds))
            .ok_or_else(|| {
                Rejected::new(
                    "invalid_credential_duration",
                    format!(
                        "Task access must be between 1 and {} seconds.",
                        self.maximum
                    ),
                    400,
                )
            })?;
        let expires = now + duration as f64;
        let secret = token()?;
        self.ledger.mutate(|tx|{
            tx.execute("INSERT INTO task_credential VALUES(?,?,?,NULL)
                ON CONFLICT(owner) DO UPDATE SET token_hash=excluded.token_hash,expires_at=excluded.expires_at,revoked_at=NULL",
                params![owner,hash(&secret),expires])?;
            Ok(json!({"token":secret,"principal":owner,"expires_at":expires}))
        })
    }
}
