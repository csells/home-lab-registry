use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use lab_allocator::restic_scope::{self, ScopeError};

#[derive(Parser)]
#[command(about = "Restrict storage-side Restic to the allocator repository and role.")]
struct Command {
    configuration: PathBuf,
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    arguments: Vec<String>,
}

fn main() -> ExitCode {
    let command = Command::parse();
    let original = std::env::var("SSH_ORIGINAL_COMMAND").ok();
    match restic_scope::run(
        &command.configuration,
        &command.arguments,
        original.as_deref(),
    ) {
        Ok(status) => ExitCode::from(u8::try_from(status).unwrap_or(1)),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(if matches!(error, ScopeError::Denied) {
                2
            } else {
                1
            })
        }
    }
}
