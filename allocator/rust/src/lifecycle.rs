use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Rejected;

const MAX_SECONDS: u64 = 365 * 86400;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeasePhase {
    Cloning,
    Resizing,
    Starting,
    Ready,
    Stopping,
    ExportStopping,
    AwaitingExport,
    Resetting,
    Deleting,
    Released,
}

impl LeasePhase {
    pub fn renewable(self) -> bool {
        matches!(
            self,
            Self::Cloning | Self::Resizing | Self::Starting | Self::Ready
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReclaimAction {
    Wait,
    Inspect,
    StopForExport,
    ReverifyExport,
    CleanupVerified,
    DiscardLegacyWorkspace,
}

pub struct Lifecycle<'a> {
    pub phase: LeasePhase,
    pub reclaim_on_expiry: bool,
    pub deadline: f64,
    pub owner_hold: Option<&'a str>,
    pub reviewer_hold: Option<&'a str>,
    pub checkpoint: Option<&'a Value>,
}

impl Lifecycle<'_> {
    fn held(&self) -> bool {
        [self.owner_hold, self.reviewer_hold]
            .into_iter()
            .flatten()
            .any(|hold| !hold.is_empty())
    }

    pub fn renew(&self, seconds: u64, now: f64) -> Result<f64, Rejected> {
        duration(seconds)?;
        if !self.phase.renewable() {
            return Err(Rejected::new(
                "lease_not_renewable",
                "Only starting or ready leases can renew.",
                409,
            ));
        }
        Ok(self.deadline.max(now + seconds as f64))
    }

    pub fn advance(&self, now: f64, grace: f64) -> ReclaimAction {
        if !self.reclaim_on_expiry {
            return ReclaimAction::Wait;
        }
        match self.phase {
            LeasePhase::Starting
            | LeasePhase::Cloning
            | LeasePhase::Resizing
            | LeasePhase::Resetting
            | LeasePhase::Deleting
            | LeasePhase::ExportStopping => ReclaimAction::Inspect,
            LeasePhase::Ready
                if now >= self.deadline + grace
                    && ![self.owner_hold, self.reviewer_hold].contains(&Some("running")) =>
            {
                ReclaimAction::StopForExport
            }
            LeasePhase::AwaitingExport if !self.held() => match self.checkpoint {
                Some(checkpoint)
                    if checkpoint["restore_verified"] == true
                        && checkpoint["storage_managed"] == true =>
                {
                    ReclaimAction::CleanupVerified
                }
                Some(_) => ReclaimAction::ReverifyExport,
                None => ReclaimAction::Wait,
            },
            _ => ReclaimAction::Wait,
        }
    }

    pub fn release(&self, request: &Value) -> Result<ReclaimAction, Rejected> {
        match self.phase {
            LeasePhase::Released
            | LeasePhase::Stopping
            | LeasePhase::ExportStopping
            | LeasePhase::AwaitingExport
            | LeasePhase::Resetting
            | LeasePhase::Deleting => return Ok(ReclaimAction::Inspect),
            LeasePhase::Ready => {}
            _ => {
                return Err(Rejected::new(
                    "lease_not_releasable",
                    "Inspect the start operation before requesting release.",
                    409,
                ));
            }
        }
        if self.reclaim_on_expiry {
            if request
                .get("preserve_checkpoint")
                .is_some_and(|value| value != true)
                || request["discard_disposable_data"] == true
            {
                return Err(Rejected::new(
                    "final_export_required",
                    "Enrolled leases require verified export before reset.",
                    409,
                ));
            }
            if self.held() {
                return Err(Rejected::new(
                    "lease_held",
                    "Clear the review hold before releasing the guest.",
                    409,
                ));
            }
            Ok(ReclaimAction::StopForExport)
        } else if request["discard_disposable_data"] == true {
            Ok(ReclaimAction::DiscardLegacyWorkspace)
        } else {
            Err(Rejected::new(
                "release_requires_disposition",
                "Confirm discard of the pilot workspace after saving any required data.",
                400,
            ))
        }
    }
}

pub(crate) fn duration(seconds: u64) -> Result<(), Rejected> {
    if (1..=MAX_SECONDS).contains(&seconds) {
        return Ok(());
    }
    Err(Rejected::new(
        "invalid_duration",
        "Provide a duration between one second and one year.",
        400,
    ))
}
