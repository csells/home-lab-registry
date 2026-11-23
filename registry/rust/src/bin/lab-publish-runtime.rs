use std::{error::Error, fs::File, path::PathBuf, process::ExitCode};

use clap::{ArgGroup, Parser};
use lab_registry::deployment::{Publisher, RuntimeState};
use serde_json::Value;

#[derive(Parser)]
#[command(about = "Publish a scoped observation of a registered deployment",
          group(ArgGroup::new("source").required(true).args(["systemd", "launchd", "state"])))]
struct Args {
    config: PathBuf,
    #[arg(long)]
    systemd: bool,
    #[arg(long)]
    launchd: bool,
    #[arg(long, value_enum)]
    state: Option<RuntimeState>,
}

async fn execute(args: Args) -> Result<Value, Box<dyn Error>> {
    let publisher: Publisher = serde_json::from_reader(File::open(args.config)?)?;
    let state = if args.systemd {
        publisher.systemd().await?
    } else if args.launchd {
        publisher.launchd().await?
    } else {
        args.state.ok_or("Select a runtime observation source")?
    };
    publisher.publish(state)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match execute(Args::parse()).await {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("Could not publish runtime observation. {problem}");
            ExitCode::FAILURE
        }
    }
}
