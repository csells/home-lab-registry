use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use lab_registry::restore_export;

#[derive(Parser)]
#[command(about = "Restore an exported registry and its history to a new database")]
struct Args {
    export: PathBuf,
    destination: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match restore_export(&args.export, &args.destination) {
        Ok(count) => {
            println!("Restored {count} services; health will be rechecked");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
