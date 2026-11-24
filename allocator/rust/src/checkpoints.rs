use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Value, json};

use crate::{
    Ledger, Rejected,
    tokens::{hash, token, valid_hash, valid_token},
    workspace::MAX_ARCHIVE,
};

const UPLOAD_SECONDS: f64 = 300.0;
const SELECT: &str =
    "SELECT id,lease_id,created_at,size,sha256,receipt,error FROM incremental_checkpoint";

pub struct CheckpointStore<'a, G> {
    ledger: &'a mut Ledger<G>,
}

struct Stored {
    id: String,
    lease: String,
    created: f64,
    size: i64,
    digest: String,
    receipt: Option<Value>,
    error: Option<String>,
}

fn unavailable() -> Rejected {
    Rejected::new(
        "checkpoint_unavailable",
        "Incremental uploads are unavailable.",
        503,
    )
}
fn ticket_unavailable() -> Rejected {
    Rejected::new("ticket_unavailable", "Upload ticket is unavailable.", 404)
}
fn invalid_receipt() -> Rejected {
    Rejected::new(
        "ledger_unavailable",
        "Invalid retained checkpoint data.",
        503,
    )
}

impl Stored {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let receipt = row
            .get::<_, Option<String>>(5)?
            .map(|value| {
                serde_json::from_str(&value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            })
            .transpose()?;
        Ok(Self {
            id: row.get(0)?,
            lease: row.get(1)?,
            created: row.get(2)?,
            size: row.get(3)?,
            digest: row.get(4)?,
            receipt,
            error: row.get(6)?,
        })
    }

    fn display(&self) -> Value {
        let mut output =
            json!({"id":self.id,"requested_at":self.created,"error":self.error,"size":self.size});
        if let Some(receipt) = self.receipt.as_ref().and_then(Value::as_object) {
            for (field, value) in receipt {
                if field != "purge_claim" {
                    output[field] = value.clone();
                }
            }
        }
        output
    }

    fn path(&self) -> String {
        format!("/lease-{}-{}.tar.gz", self.lease, self.id)
    }
}

fn by_id(db: &Connection, id: &str) -> Result<Stored, Rejected> {
    db.query_row(&format!("{SELECT} WHERE id=?"), [id], Stored::from_row)
        .optional()?
        .ok_or_else(invalid_receipt)
}

fn by_ticket(db: &Connection, secret: &str, redeemed: bool, now: f64) -> Result<Stored, Rejected> {
    if !valid_token(secret, 43, 43) {
        return Err(ticket_unavailable());
    }
    db.query_row(
        &format!("{SELECT} WHERE token_hash=? AND redeemed=? AND upload_expires>?"),
        params![hash(secret), redeemed, now],
        Stored::from_row,
    )
    .optional()?
    .ok_or_else(ticket_unavailable)
}

impl<'a, G: FnMut() -> Result<String, Rejected>> CheckpointStore<'a, G> {
    pub fn new(ledger: &'a mut Ledger<G>) -> Self {
        Self { ledger }
    }

    pub fn list(&self, lease: &str) -> Result<Value, Rejected> {
        let mut rows = self.ledger.connection().prepare(&format!(
            "{SELECT}
            WHERE lease_id=? ORDER BY created_at DESC,id DESC LIMIT 32"
        ))?;
        let selected = rows.query_map([lease], Stored::from_row)?;
        let mut result = Vec::new();
        for row in selected {
            result.push(row?.display());
        }
        Ok(json!(result))
    }

    pub fn grant(
        &mut self,
        lease: &str,
        owner: &str,
        request: &Value,
        upload: Option<&str>,
        now: f64,
    ) -> Result<Value, Rejected> {
        self.ledger.mutate(|tx|{
            let bound=tx.query_row("SELECT phase,reclaim_on_expiry FROM lease WHERE id=? AND owner=?",
                params![lease,owner],|row|Ok((row.get::<_,String>(0)?,row.get::<_,bool>(1)?))).optional()?
                .ok_or_else(||Rejected::new("unknown_lease","No lease has this identifier.",404))?;
            let fields=request["request_key"].as_str().filter(|key|(1..=128).contains(&key.chars().count()))
                .zip(request["sha256"].as_str().filter(|digest|valid_hash(digest)))
                .zip(request["size"].as_i64().filter(|size|(1..=MAX_ARCHIVE as i64).contains(size)));
            let Some(((key,digest),size))=fields else {
                return Err(Rejected::new("invalid_checkpoint","Supply a request key, digest and permitted archive size.",400));
            };
            let previous=tx.query_row(&format!("{SELECT} WHERE lease_id=? AND request_key=?"),
                params![lease,key],Stored::from_row).optional()?;
            if let Some(prior)=&previous{
                if prior.size!=size || prior.digest!=digest{
                    return Err(Rejected::new("checkpoint_key_conflict","This key already identifies different bytes.",409));
                }
                if prior.receipt.is_some(){return Ok(json!({"checkpoint":prior.display()}));}
            }
            let upload=upload.filter(|_|bound.0=="ready"&&bound.1).ok_or_else(unavailable)?;
            let busy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM incremental_checkpoint
                    WHERE lease_id=? AND receipt IS NULL AND upload_expires>?)",params![lease,now],|row|row.get(0))?;
            if busy{return Err(Rejected::new("checkpoint_in_progress","Another upload grant is active. Inspect its status before retrying.",409));}
            let id=if let Some(prior)=previous{prior.id}else{
                let id=uuid::Uuid::new_v4().simple().to_string();
                tx.execute("INSERT INTO incremental_checkpoint (id,lease_id,request_key,created_at,sha256,size) VALUES (?,?,?,?,?,?)",
                    params![id,lease,key,now,digest,size])?;
                id
            };
            let secret=token()?;
            tx.execute("UPDATE incremental_checkpoint SET token_hash=?,upload_expires=?,redeemed=0,error=NULL WHERE id=?",
                params![hash(&secret),now+UPLOAD_SECONDS,id])?;
            Ok(json!({"id":id,"endpoint":upload,"ticket":secret}))
        })
    }

    pub fn redeem(&mut self, secret: &str, now: f64) -> Result<Value, Rejected> {
        self.ledger.mutate(|tx|{
            let current=by_ticket(tx,secret,false,now)?;
            let ready=tx.query_row("SELECT phase='ready' FROM lease WHERE id=?",[&current.lease],
                |row|row.get::<_,bool>(0)).optional()?.unwrap_or(false);
            if !ready{
                return Err(Rejected::new("checkpoint_unavailable","The lease is no longer ready for uploads.",409));
            }
            tx.execute("UPDATE incremental_checkpoint SET redeemed=1,upload_expires=? WHERE id=?",
                params![now+UPLOAD_SECONDS,current.id])?;
            Ok(json!({"id":current.id,"size":current.size,"sha256":current.digest,"path":current.path()}))
        })
    }

    pub fn complete(
        &mut self,
        request: &Value,
        retain_through: Option<f64>,
        now: f64,
    ) -> Result<Value, Rejected> {
        self.ledger.mutate(|tx|{
            let secret=request["ticket"].as_str().ok_or_else(ticket_unavailable)?;
            let previous=by_ticket(tx,secret,true,now)?;
            if !request["error"].is_null(){
                let reason=request["error"].as_str().filter(|value|(1..=240).contains(&value.chars().count()))
                    .filter(|_|previous.receipt.is_none())
                    .ok_or_else(||Rejected::new("invalid_checkpoint_result","Invalid upload failure.",400))?;
                tx.execute("UPDATE incremental_checkpoint SET token_hash=NULL,upload_expires=NULL,error=? WHERE id=?",
                    params![reason,previous.id])?;
            }else{
                let verified=request["snapshot"].as_str().filter(|value|valid_hash(value))
                    .filter(|_|request["sha256"]==previous.digest)
                    .ok_or_else(||Rejected::new("invalid_checkpoint_result","Upload did not verify the declared bytes.",400))?;
                if let Some(receipt)=&previous.receipt{
                    if receipt["snapshot"]!=verified{
                        return Err(Rejected::new("checkpoint_result_conflict","A different snapshot was already acknowledged.",409));
                    }
                    return Ok(previous.display());
                }
                let mut receipt=json!({"snapshot":verified,"path":previous.path(),"sha256":previous.digest,
                    "verified_at":now,"storage_managed":true});
                if let Some(retain_through)=retain_through{
                    receipt["retain_through"]=json!(retain_through);
                }
                tx.execute("UPDATE incremental_checkpoint SET receipt=?,error=NULL WHERE id=?",
                    params![receipt.to_string(),previous.id])?;
            }
            Ok(by_id(tx,&previous.id)?.display())
        })
    }
}
