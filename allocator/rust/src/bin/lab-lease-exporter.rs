use std::{path::PathBuf, process::ExitCode};

use clap::Parser;

#[derive(Parser)]
#[command(
    about = "Export through the scoped stopped-guest capture and encrypted storage interfaces."
)]
struct Command {
    configuration: PathBuf,
}

fn main() -> ExitCode {
    let options = Command::parse();
    // Keep temporary workspace contents private to this worker.
    unsafe { libc::umask(0o077) };
    match lab_allocator::run_exporter(&options.configuration) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("Final export needs retry: {error}");
            ExitCode::FAILURE
        }
    }
}
