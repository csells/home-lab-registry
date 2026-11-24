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
    deployment::{DeploymentCatalog, DeploymentIdentity, RuntimeState, observe_launchd, observe_systemd},
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
    /// Verify a local launchd job and plist before registering or retiring the deployment.
    SyncLaunchd,
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
        action @ (Action::SyncSystemd | Action::SyncLaunchd) => {
            DeploymentIdentity::from_spec(&spec)?;
            if spec["lifecycle"] != "active" {
                return Err("Use the original active deployment manifest".into());
            }
            let binding = &spec["resources"]["runtime_binding"];
            if binding["kind"] != "native" {
                return Err("Runtime sync requires a native deployment binding".into());
            }
            let instance = binding["instance"].as_str().ok_or("Missing deployment instance")?;
            let state = match action {
                Action::SyncSystemd if binding["provider"] == "systemd" => {
                    observe_systemd(instance).await?
                }
                Action::SyncLaunchd if binding["provider"] == "launchd" => {
                    let (domain, label) = instance.rsplit_once('/')
                        .ok_or("Provide a launchd domain and label in the deployment instance")?;
                    let plist = binding["plist_path"].as_str().ok_or("Missing launchd plist path")?;
                    observe_launchd(domain, label, Some(Path::new(plist))).await?
                }
                _ => return Err("Runtime sync needs the matching native provider binding".into()),
            };
            match state {
                RuntimeState::Present => catalog.register(&args.record, &spec).await?,
                RuntimeState::Missing => catalog.retire(&args.record, &spec).await?,
                RuntimeState::Unverified => {
                    return Err("Could not verify the service on the deployment host".into());
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
