use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Ledger, Rejected};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Intent {
    pub phase: String,
    pub upstream: Option<String>,
    pub prior_tasks: Option<Vec<String>>,
    pub generation: Option<String>,
}

pub struct LeaseJournal<'a, G> {
    ledger: &'a mut Ledger<G>,
}

fn lease_unknown() -> Rejected {
    Rejected::new("unknown_lease", "No lease has this identifier.", 404)
}

fn generation_changed() -> Rejected {
    Rejected::new(
        "slot_reconciliation_required",
        "Provider generation differs from the recorded lease. Hold for operator reconciliation.",
        409,
    )
}

fn wrong_phase() -> Rejected {
    Rejected::new(
        "operation_outcome_unknown",
        "The operation intent already exists or the lease phase changed.",
        409,
    )
}

fn get_intent(connection: &Connection, lease: &str, owner: &str) -> Result<Intent, Rejected> {
    let row = connection
        .query_row(
            "SELECT phase,upstream,prior_tasks,generation FROM lease WHERE id=? AND owner=?",
            params![lease, owner],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(lease_unknown)?;
    let prior_tasks = row
        .2
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| {
            Rejected::new(
                "ledger_reconciliation_required",
                "The lease contains invalid operation history.",
                409,
            )
        })?;
    Ok(Intent {
        phase: row.0,
        upstream: row.1,
        prior_tasks,
        generation: row.3,
    })
}

fn expected(intent: &Intent, phase: &str, generation: &str) -> Result<(), Rejected> {
    if intent.generation.as_deref() != Some(generation) {
        return Err(generation_changed());
    }
    if intent.phase != phase {
        return Err(wrong_phase());
    }
    Ok(())
}

impl<'a, G: FnMut() -> Result<String, Rejected>> LeaseJournal<'a, G> {
    pub fn new(ledger: &'a mut Ledger<G>) -> Self {
        Self { ledger }
    }

    pub fn get(&self, lease: &str, owner: &str) -> Result<Intent, Rejected> {
        get_intent(self.ledger.connection(), lease, owner)
    }

    pub fn record_intent(
        &mut self,
        lease: &str,
        owner: &str,
        from: &str,
        phase: &str,
        generation: &str,
        prior: &[String],
    ) -> Result<Intent, Rejected> {
        if !matches!(
            phase,
            "cloning"
                | "resizing"
                | "starting"
                | "stopping"
                | "export-stopping"
                | "resetting"
                | "deleting"
        ) {
            return Err(wrong_phase());
        }
        let tasks: BTreeSet<_> = prior.iter().collect();
        let prior = serde_json::to_string(&tasks).map_err(|_| wrong_phase())?;
        self.ledger.mutate(|tx| {
            let intent = get_intent(tx, lease, owner)?;
            expected(&intent, from, generation)?;
            if from == phase || intent.prior_tasks.is_some() && intent.upstream.is_none() {
                return Err(wrong_phase());
            }
            tx.execute(
                "UPDATE lease SET phase=?,upstream=NULL,address=NULL,prior_tasks=? WHERE id=?",
                params![phase, prior, lease],
            )?;
            get_intent(tx, lease, owner)
        })
    }

    pub fn record_task(
        &mut self,
        lease: &str,
        owner: &str,
        phase: &str,
        generation: &str,
        task: &str,
    ) -> Result<Intent, Rejected> {
        if task.is_empty() {
            return Err(wrong_phase());
        }
        self.ledger.mutate(|tx| {
            let intent = get_intent(tx, lease, owner)?;
            expected(&intent, phase, generation)?;
            if intent.prior_tasks.is_none()
                || intent
                    .upstream
                    .as_deref()
                    .is_some_and(|prior| prior != task)
            {
                return Err(wrong_phase());
            }
            tx.execute(
                "UPDATE lease SET upstream=? WHERE id=?",
                params![task, lease],
            )?;
            get_intent(tx, lease, owner)
        })
    }

    pub fn recover_task(
        &mut self,
        lease: &str,
        owner: &str,
        phase: &str,
        generation: &str,
        candidates: &[String],
    ) -> Result<Option<String>, Rejected> {
        let intent = self.get(lease, owner)?;
        expected(&intent, phase, generation)?;
        if intent.upstream.is_some() {
            return Ok(intent.upstream);
        }
        let Some(prior) = intent.prior_tasks else {
            return Ok(None);
        };
        let before: BTreeSet<_> = prior.iter().collect();
        let remaining: BTreeSet<_> = candidates
            .iter()
            .filter(|task| !before.contains(task))
            .collect();
        if remaining.len() != 1 {
            return Ok(None);
        }
        let task = remaining.into_iter().next().expect("one matching task");
        self.record_task(lease, owner, phase, generation, task)?;
        Ok(Some(task.clone()))
    }
}
