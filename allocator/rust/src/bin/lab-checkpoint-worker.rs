use std::{path::PathBuf, process::ExitCode};

use clap::Parser;

#[derive(Parser)]
#[command(about = "Serve checkpoints after redeeming scoped transfer tickets.")]
struct Command {
    configuration: PathBuf,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> ExitCode {
    let options = Command::parse();
    // Captured workspace data is private to this worker.
    unsafe { libc::umask(0o077) };
    match lab_allocator::checkpoint_worker::run(&options.configuration).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Checkpoint worker could not start: {error}");
            ExitCode::FAILURE
        }
    }
}
