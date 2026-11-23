use std::{fs::File, path::PathBuf, process::ExitCode};

use clap::Parser;
use serde_json::Value;

#[derive(Parser)]
#[command(about = "Read only the Proxmox guest identities allowed by the forced-command policy.")]
struct Options {
    #[arg(long)]
    policy: PathBuf,
}

fn execute(options: Options) -> Result<Value, Box<dyn std::error::Error>> {
    let policy: Value = serde_json::from_reader(File::open(options.policy)?)?;
    let original = std::env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    let (id, kind) = lab_backup::guest_request(&original, &policy["guests"])?;
    lab_backup::observe(policy["node"].as_str().unwrap_or_default(), id, kind)
}

fn main() -> ExitCode {
    match execute(Options::parse()) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Guest observation refused: {error}");
            ExitCode::FAILURE
        }
    }
}
