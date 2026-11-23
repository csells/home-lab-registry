mod lifecycle;
pub use lifecycle::{LeasePhase, Lifecycle, ReclaimAction};
mod journal;
pub use journal::{Intent, LeaseJournal};
mod review;
pub use review::{GuestScope, lease_inventory, lease_review};
mod ledger;

pub use ledger::Ledger;

#[derive(Debug)]
pub struct Rejected {
    code: &'static str,
    reason: String,
    status: u16,
}

impl Rejected {
    pub fn new(code: &'static str, reason: impl Into<String>, status: u16) -> Self {
        Self {
            code,
            reason: reason.into(),
            status,
        }
    }

    pub const fn code(&self) -> &'static str {
        self.code
    }
    pub const fn status(&self) -> u16 {
        self.status
    }
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl std::error::Error for Rejected {}

impl From<rusqlite::Error> for Rejected {
    fn from(_: rusqlite::Error) -> Self {
        Self::new(
            "ledger_unavailable",
            "The allocator could not read or persist its ledger.",
            503,
        )
    }
}
