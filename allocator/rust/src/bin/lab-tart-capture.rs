use std::{fs::File, io, path::PathBuf, process::ExitCode};

use clap::Parser;
use serde_json::Value;

#[derive(Parser)]
#[command(about = "Read the exact stopped Tart workspace using its capture role.")]
struct Command {
    configuration: PathBuf,
    slot: String,
    lease: String,
    generation: String,
}

fn main() -> ExitCode {
    let command = Command::parse();
    let result = || -> Result<(), Box<dyn std::error::Error>> {
        let config: Value = serde_json::from_reader(File::open(command.configuration)?)?;
        if command.slot != "apple-workspace" {
            return Err("Invalid Tart capture slot.".into());
        }
        lab_allocator::tart_worker::fetch_capture(
            &config,
            &command.lease,
            &command.generation,
            io::stdout().lock(),
        )?;
        Ok(())
    };
    match result() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Tart capture failed: {error}");
            ExitCode::FAILURE
        }
    }
}
