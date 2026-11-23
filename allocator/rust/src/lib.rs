mod process;
pub mod task_client;
pub mod task_resume;

pub use lab_workspace as workspace;

mod protocol;
mod tokens;

#[cfg(all(unix, feature = "server"))]
include!("server.rs");

#[derive(Debug)]
pub struct Rejected {
    code: String,
    reason: String,
    status: u16,
}

impl Rejected {
    pub fn new(code: impl Into<String>, reason: impl Into<String>, status: u16) -> Self {
        Self {
            code: code.into(),
            reason: reason.into(),
            status,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
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

pub use process::bounded as bounded_command;
