use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use serde_json::json;

#[derive(Parser)]
#[command(about = "Check LAN address authority against fresh router, host and appliance evidence.")]
struct Options {
    private_directory: PathBuf,
}

fn main() -> ExitCode {
    match lab_dashboard::network::run(&Options::parse().private_directory) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).unwrap_or_default()
            );
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("Read-only address audit failed.");
            println!(
                "{}",
                json!({"status":"error","rows":[],
                "message":"Audit failed; no clean result established. Check server access to router and registry."})
            );
            ExitCode::FAILURE
        }
    }
}
