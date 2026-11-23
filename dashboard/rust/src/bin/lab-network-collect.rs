use std::{io, process::ExitCode};

use clap::Parser;

#[derive(Parser)]
#[command(about = "Collect read-only IPv4 assignment evidence from a host and its guests.")]
struct Options {
    #[arg(long)]
    guest_only: bool,
    #[arg(long)]
    tart: Vec<String>,
    #[arg(long, default_value = "/usr/local/bin/lab-network-collect")]
    guest_binary: String,
    #[arg(long, conflicts_with_all=["guest_only","tart"])]
    appliance: bool,
}

fn main() -> ExitCode {
    let options = Options::parse();
    let rows = if options.appliance {
        lab_dashboard::network::collector::remote_appliance(io::stdin().lock())
    } else {
        Ok(lab_dashboard::network::collector::collect(
            options.guest_only,
            &options.tart,
            &options.guest_binary,
        ))
    };
    match rows.and_then(|rows| serde_json::to_string(&rows).map_err(Into::into)) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("IPv4 collection failed.");
            ExitCode::FAILURE
        }
    }
}
