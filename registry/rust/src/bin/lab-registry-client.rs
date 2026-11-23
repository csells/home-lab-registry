use std::{error::Error, fs::File, io, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand, ValueEnum};
use lab_registry::client::{Method, RegistryClient};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(about = "Read and maintain the home lab registry")]
struct Args {
    #[arg(
        long,
        env = "LAB_REGISTRY_CONFIG",
        default_value = "~/.config/home-lab/registry.json"
    )]
    config: PathBuf,
    #[command(subcommand)]
    operation: Operation,
}

fn valid_id(value: &str) -> Result<String, &'static str> {
    lab_registry::client::valid_id(value)
        .then(|| value.to_owned())
        .ok_or("Invalid service ID")
}

#[derive(Subcommand)]
enum Operation {
    List {
        #[arg(long)]
        capability: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        status: Option<String>,
        #[arg(long)]
        include_retired: bool,
    },
    Get {
        #[arg(value_parser = valid_id)]
        id: String,
    },
    History {
        #[arg(value_parser = valid_id)]
        id: String,
    },
    Put {
        #[arg(value_parser = valid_id)]
        id: String,
        file: PathBuf,
        #[arg(long)]
        revision: i64,
    },
    Retire {
        #[arg(value_parser = valid_id)]
        id: String,
        #[arg(long)]
        revision: i64,
    },
    Observe {
        #[arg(value_parser = valid_id)]
        id: String,
        #[arg(long, value_enum)]
        status: Health,
        #[arg(long)]
        evidence: String,
        #[arg(long, default_value_t = 300)]
        ttl: i64,
    },
}

#[derive(Clone, ValueEnum)]
enum Health {
    Healthy,
    Unhealthy,
    Unknown,
}

impl Health {
    fn name(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unhealthy => "unhealthy",
            Self::Unknown => "unknown",
        }
    }
}

fn execute(args: Args) -> Result<Value, Box<dyn Error>> {
    let client = RegistryClient::read_config(&args.config)?;
    let collection = "/v1/services";
    let route = |id: &str| format!("{collection}/{id}");

    Ok(match args.operation {
        Operation::List {
            capability,
            host,
            status,
            include_retired,
        } => {
            let mut query = form_urlencoded::Serializer::new(String::new());
            for (key, value) in [
                ("capability", capability),
                ("host", host),
                ("status", status),
            ] {
                if let Some(value) = value.filter(|value| !value.is_empty()) {
                    query.append_pair(key, &value);
                }
            }
            if include_retired {
                query.append_pair("include_retired", "true");
            }
            let query = query.finish();
            client.request(Method::Get, &format!("{collection}?{query}"), None, None)?
        }
        Operation::Get { id } => client.request(Method::Get, &route(&id), None, None)?,
        Operation::History { id } => {
            client.request(Method::Get, &format!("{}/history", route(&id)), None, None)?
        }
        Operation::Put { id, file, revision } => {
            let document = if file == std::path::Path::new("-") {
                serde_json::from_reader(io::stdin().lock())?
            } else {
                serde_json::from_reader(File::open(file)?)?
            };
            client.request(Method::Put, &route(&id), Some(&document), Some(revision))?
        }
        Operation::Retire { id, revision } => {
            let route = route(&id);
            let mut current = client.request(Method::Get, &route, None, None)?;
            if current["revision"].as_i64() != Some(revision) {
                return Err("Revision conflict; fetch latest record".into());
            }
            let spec = current
                .get_mut("spec")
                .and_then(Value::as_object_mut)
                .ok_or("Registry reply is missing its record specification")?;
            spec.insert("lifecycle".to_owned(), json!("retired"));
            client.request(Method::Put, &route, Some(&current["spec"]), Some(revision))?
        }
        Operation::Observe {
            id,
            status,
            evidence,
            ttl,
        } => client.request(
            Method::Post,
            &format!("{}/observe", route(&id)),
            Some(&json!({"status":status.name(),"evidence":evidence,"ttl_seconds":ttl})),
            None,
        )?,
    })
}

fn main() -> ExitCode {
    match execute(Args::parse()).and_then(|value| Ok(serde_json::to_string_pretty(&value)?)) {
        Ok(document) => {
            println!("{document}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
