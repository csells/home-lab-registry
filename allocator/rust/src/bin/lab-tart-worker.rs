use std::{path::PathBuf, process::ExitCode};

use clap::Parser;

#[derive(Parser)]
#[command(about = "Serve one scoped Tart recipe through the local worker tunnel.")]
struct Command {
    configuration: PathBuf,
    #[arg(long)]
    preflight: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let command = Command::parse();
    // Set the worker's process-wide file creation mask before spawning work.
    unsafe { libc::umask(0o077) };
    match lab_allocator::tart_worker::run(&command.configuration, command.preflight).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Tart worker startup failed: {error}");
            ExitCode::FAILURE
        }
    }
}
