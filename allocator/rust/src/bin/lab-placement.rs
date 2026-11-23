use clap::Parser;
use serde_json::Value;
use std::{
    fs::File,
    io,
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Parser)]
#[command(about = "Select a staging worker using fresh, trusted capacity observations.")]
struct Options {
    input: PathBuf,
}

fn main() -> Result<ExitCode, Box<dyn std::error::Error>> {
    let input = Options::parse().input;
    let data: Value = if input == std::path::Path::new("-") {
        serde_json::from_reader(io::stdin().lock())?
    } else {
        serde_json::from_reader(File::open(input)?)?
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
    let result =
        lab_allocator::operators::placement(&data["requirements"], &data["observations"], now)?;
    println!("{result}");
    Ok(ExitCode::from(if result["placement"].is_null() {
        2
    } else {
        0
    }))
}
