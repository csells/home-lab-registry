use clap::Parser;
use serde_json::{Value, json};
use std::{
    fs::File,
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(about = "Check pinned unprivileged CUDA recipe and current GPU observations.")]
struct Options {
    input: PathBuf,
}

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let data: Value = serde_json::from_reader(File::open(Options::parse().input)?)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    match lab_allocator::operators::cuda_preflight(
        &data["recipe"],
        &data["template"],
        &data["observation"],
        now,
    ) {
        Ok(result) => {
            println!("{result}");
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            println!("{}", json!({"code":error.code(),"reason":error.reason()}));
            Ok(ExitCode::from(2))
        }
    }
}
