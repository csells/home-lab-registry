use clap::Parser;
use serde_json::Value;
use std::{fs::File, path::PathBuf};

#[derive(Parser)]
#[command(about = "Estimate the resource and owner impact of recipe maintenance.")]
struct Options {
    observation: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data: Value = serde_json::from_reader(File::open(Options::parse().observation)?)?;
    let result = lab_allocator::operators::maintenance(
        &data["current"],
        &data["desired"],
        &data["active_leases"],
        &data["spare"],
        data["replacement_slot"].as_bool(),
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
