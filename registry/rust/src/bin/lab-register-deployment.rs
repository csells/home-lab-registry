use std::{
    error::Error,
    fs::File,
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, ValueEnum};
use lab_registry::{
    client::RegistryClient,
    deployment::{DeploymentCatalog, DeploymentIdentity, RuntimeState, observe_systemd},
};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(about = "Register or retire an exact deployment generation")]
struct Args {
    #[arg(long, conflicts_with = "registry_client")]
    config: Option<PathBuf>,
    #[arg(long)]
    registry_client: Option<PathBuf>,
    action: Action,
    record: String,
    manifest: PathBuf,
}

#[derive(Clone, ValueEnum)]
enum Action {
    Register,
    Retire,
    /// Verify a local unit after installation or removal and sync the exact catalog generation.
    SyncSystemd,
}

async fn execute(args: Args) -> Result<Value, Box<dyn Error>> {
    let catalog = match args.registry_client {
        Some(path) => DeploymentCatalog::External(path),
        None => {
            let config = args
                .config
                .or_else(|| std::env::var_os("LAB_REGISTRY_CONFIG").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("~/.config/home-lab/registry.json"));
            DeploymentCatalog::Direct(RegistryClient::read_config(&config)?)
        }
    };
    let spec = if args.manifest == Path::new("-") {
        serde_json::from_reader(io::stdin().lock())?
    } else {
        serde_json::from_reader(File::open(args.manifest)?)?
    };
    let result = match args.action {
        Action::Register => catalog.register(&args.record, &spec).await?,
        Action::Retire => catalog.retire(&args.record, &spec).await?,
        Action::SyncSystemd => {
            DeploymentIdentity::from_spec(&spec)?;
            if spec["lifecycle"] != "active" {
                return Err("Use the original active deployment manifest".into());
            }
            let binding = &spec["resources"]["runtime_binding"];
            if binding["kind"] != "native" || binding["provider"] != "systemd" {
                return Err("Systemd sync requires a native systemd deployment binding".into());
            }
            let unit = binding["instance"].as_str().ok_or("Missing systemd unit")?;
            match observe_systemd(unit).await? {
                RuntimeState::Present => catalog.register(&args.record, &spec).await?,
                RuntimeState::Missing => catalog.retire(&args.record, &spec).await?,
                RuntimeState::Unverified => {
                    return Err("Could not verify the unit on the deployment host".into());
                }
            }
        }
    };
    Ok(
        json!({"id":result["id"], "revision":result["revision"], "lifecycle":result["spec"]["lifecycle"]}),
    )
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match execute(Args::parse()).await {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("Deployment registration failed. {problem}");
            ExitCode::FAILURE
        }
    }
}
