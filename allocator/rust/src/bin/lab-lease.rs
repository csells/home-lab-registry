use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::{ArgGroup, Parser, Subcommand, ValueEnum};
use lab_allocator::{
    task_client::{
        TaskClient, TaskError, TaskSession, check_lease_id, ensure_output_available,
        store_task_token,
    },
    task_resume::{Resume, discover, resume},
};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(about="Acquire and recover a disposable home lab environment",
    group(ArgGroup::new("source").required(true).args(["endpoint","record","session"])))]
struct Args {
    #[arg(long)]
    endpoint: Option<String>,
    #[arg(long, requires = "registry_client")]
    record: Option<String>,
    #[arg(long, conflicts_with_all=["ca","token_file","registry_client","registry_config"])]
    session: Option<PathBuf>,
    #[arg(long)]
    registry_client: Option<PathBuf>,
    #[arg(long)]
    registry_config: Option<PathBuf>,
    #[arg(long)]
    ca: Option<PathBuf>,
    #[arg(long)]
    token_file: Option<PathBuf>,
    #[command(subcommand)]
    operation: Operation,
}

#[derive(Clone, ValueEnum)]
enum HoldMode {
    Running,
    Disk,
    Clear,
}

impl HoldMode {
    fn name(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Disk => "disk",
            Self::Clear => "clear",
        }
    }
}

fn parse_requirements(value: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(value)
}

#[derive(Subcommand)]
enum Operation {
    Capabilities,
    Contract,
    Leases,
    Acquire {
        #[arg(long)]
        task: String,
        #[arg(long)]
        request_key: String,
        #[arg(long)]
        seconds: u64,
        #[arg(long)]
        ssh_public_key: Option<PathBuf>,
        #[arg(long)]
        recipe: Option<String>,
        #[arg(long,value_parser=parse_requirements)]
        requirements: Option<Value>,
        #[arg(long, hide = true)]
        reclaim_on_expiry: bool,
    },
    Resume {
        predecessor: String,
        #[arg(long)]
        task: String,
        #[arg(long)]
        request_key: String,
        #[arg(long)]
        seconds: u64,
        #[arg(long)]
        identity: PathBuf,
        #[arg(long)]
        checkpoint_id: Option<String>,
        #[arg(long)]
        restore_helpers: Option<PathBuf>,
    },
    Inspect {
        lease: String,
    },
    Renew {
        lease: String,
        #[arg(long)]
        seconds: u64,
    },
    Release {
        lease: String,
        #[arg(long, conflicts_with = "preserve_checkpoint")]
        discard_disposable_data: bool,
        #[arg(long)]
        preserve_checkpoint: bool,
    },
    AssignReviewer {
        lease: String,
        #[arg(long)]
        reviewer: String,
    },
    RemoveReviewer {
        lease: String,
    },
    OverrideReviewer {
        lease: String,
        #[arg(long)]
        confirm_reviewer: String,
        #[arg(long)]
        reason: String,
    },
    Hold {
        lease: String,
        #[arg(long, value_enum, default_value = "disk")]
        mode: HoldMode,
        #[arg(long)]
        reason: Option<String>,
    },
    ReviewerHold {
        lease: String,
        #[arg(long, value_enum, default_value = "disk")]
        mode: HoldMode,
        #[arg(long)]
        reason: Option<String>,
    },
    RetainFailedExport {
        lease: String,
        #[arg(long)]
        reason: String,
    },
    DiscardFailedExport {
        lease: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        confirm_lease: String,
    },
    RetryExport {
        lease: String,
    },
    DelegateRenewal {
        lease: String,
        #[arg(long)]
        duration_seconds: u64,
        #[arg(long)]
        extension_seconds: u64,
        #[arg(long)]
        write_token: PathBuf,
    },
    DelegatedRenew {
        lease: String,
    },
    RevokeRenewal {
        lease: String,
    },
    Checkpoint {
        lease: String,
        #[arg(long)]
        archive: PathBuf,
        #[arg(long)]
        request_key: Option<String>,
    },
    Recover {
        lease: String,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        checkpoint_id: Option<String>,
    },
    Pin {
        lease: String,
        #[arg(long)]
        reason: String,
        #[arg(long)]
        checkpoint_id: Option<String>,
    },
    Unpin {
        lease: String,
        #[arg(long)]
        checkpoint_id: Option<String>,
    },
    ReserveCapacity {
        #[arg(long)]
        request_key: String,
        #[arg(long)]
        purpose: String,
        #[arg(long, default_value = "primary")]
        slot: String,
        #[arg(long, default_value_t = 0)]
        memory_mib: i64,
        #[arg(long, default_value_t = 0)]
        storage_mib: i64,
    },
    ListReservations,
    ReleaseReservation {
        reservation: String,
    },
    IssueTaskCredential {
        #[arg(long)]
        task_ref: String,
        #[arg(long)]
        seconds: u64,
        #[arg(long)]
        write_token: PathBuf,
    },
    RevokeTaskCredential {
        #[arg(long)]
        task_ref: String,
    },
}

fn error(reason: &str) -> TaskError {
    TaskError::client(reason)
}

fn route(id: &str, suffix: &str) -> Result<String, TaskError> {
    Ok(format!("/v1/leases/{}{suffix}", check_lease_id(id)?))
}

fn send(client: &TaskClient, path: &str, body: Value) -> Result<Value, TaskError> {
    client.request(path, Some(&body))
}

fn scoped(client: &TaskClient, lease: &str, suffix: &str, body: Value) -> Result<Value, TaskError> {
    send(client, &route(lease, suffix)?, body)
}

fn private_grant(
    client: &TaskClient,
    path: &str,
    body: Value,
    output: &Path,
) -> Result<Value, TaskError> {
    ensure_output_available(output)?;
    let mut grant = send(client, path, body)?;
    store_task_token(&mut grant, output)?;
    Ok(grant)
}

fn resolve(args: &Args) -> Result<(TaskClient, Option<PathBuf>), TaskError> {
    if let Some(path) = &args.session {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error("System time is invalid"))?
            .as_secs_f64();
        let session = TaskSession::load(path, now)?;
        let client = TaskClient::new(&session.endpoint, session.ca, session.token)?;
        return Ok((client, Some(session.identity)));
    }
    let ca = args
        .ca
        .clone()
        .ok_or_else(|| error("Supply --ca and --token-file, or a task --session"))?;
    let token = args
        .token_file
        .clone()
        .ok_or_else(|| error("Supply --ca and --token-file, or a task --session"))?;
    let endpoint = match &args.endpoint {
        Some(url) => url.clone(),
        None => discover(
            args.registry_client
                .as_deref()
                .ok_or_else(|| error("--record requires --registry-client"))?,
            args.registry_config.as_deref(),
            args.record
                .as_deref()
                .ok_or_else(|| error("Provide a lease discovery record"))?,
        )?,
    };
    Ok((TaskClient::new(&endpoint, ca, token)?, None))
}

fn execute(args: Args) -> Result<Value, TaskError> {
    let (client, session_key) = resolve(&args)?;
    let client = &client;
    match args.operation {
        Operation::Capabilities => client.request("/v1/capabilities", None),
        Operation::Contract => client.request("/v1/contract", None),
        Operation::Leases => client.request("/v1/leases", None),
        Operation::Acquire {
            task,
            request_key,
            seconds,
            ssh_public_key,
            recipe,
            requirements,
            reclaim_on_expiry,
        } => {
            let key = ssh_public_key
                .or(session_key)
                .ok_or_else(|| error("Provide a task SSH public key or launcher session"))?;
            let identity = fs::read_to_string(key)?;
            let mut body = json!({"task":task,"request_key":request_key,"seconds":seconds,
                "ssh_public_key":identity.split_whitespace().take(2).collect::<Vec<_>>().join(" ")});
            if let Some(recipe) = recipe {
                body["recipe"] = json!(recipe);
            }
            if let Some(requirements) = requirements {
                body["requirements"] = requirements;
            }
            if reclaim_on_expiry {
                body["reclaim_on_expiry"] = json!(true);
            }
            send(client, "/v1/leases", body)
        }
        Operation::Resume {
            predecessor,
            task,
            request_key,
            seconds,
            identity,
            checkpoint_id,
            restore_helpers,
        } => resume(
            client,
            Resume {
                predecessor: &predecessor,
                task: &task,
                request_key: &request_key,
                seconds,
                identity: &identity,
                checkpoint: checkpoint_id.as_deref(),
                helpers: restore_helpers.as_deref(),
            },
        ),
        Operation::Inspect { lease } => client.request(&route(&lease, "")?, None),
        Operation::Renew { lease, seconds } => {
            scoped(client, &lease, "/renew", json!({"seconds":seconds}))
        }
        Operation::Release {
            lease,
            discard_disposable_data,
            ..
        } => scoped(
            client,
            &lease,
            "/release",
            json!({"discard_disposable_data":discard_disposable_data,"preserve_checkpoint":!discard_disposable_data}),
        ),
        Operation::AssignReviewer { lease, reviewer } => {
            scoped(client, &lease, "/reviewer", json!({"reviewer":reviewer}))
        }
        Operation::RemoveReviewer { lease } => {
            scoped(client, &lease, "/reviewer", json!({"reviewer":null}))
        }
        Operation::OverrideReviewer {
            lease,
            confirm_reviewer,
            reason,
        } => scoped(
            client,
            &lease,
            "/override-reviewer",
            json!({"confirm_reviewer":confirm_reviewer,"reason":reason}),
        ),
        Operation::Hold {
            lease,
            mode,
            reason,
        } => scoped(
            client,
            &lease,
            "/hold",
            json!({"mode":mode.name(),"reason":reason}),
        ),
        Operation::ReviewerHold {
            lease,
            mode,
            reason,
        } => scoped(
            client,
            &lease,
            "/reviewer-hold",
            json!({"mode":mode.name(),"reason":reason}),
        ),
        Operation::RetainFailedExport { lease, reason } => scoped(
            client,
            &lease,
            "/export-disposition",
            json!({"mode":"retain","reason":reason,"confirm_lease":null}),
        ),
        Operation::DiscardFailedExport {
            lease,
            reason,
            confirm_lease,
        } => scoped(
            client,
            &lease,
            "/export-disposition",
            json!({"mode":"discard","reason":reason,"confirm_lease":confirm_lease}),
        ),
        Operation::RetryExport { lease } => scoped(client, &lease, "/retry-export", json!({})),
        Operation::DelegateRenewal {
            lease,
            duration_seconds,
            extension_seconds,
            write_token,
        } => private_grant(
            client,
            &route(&lease, "/delegate-renewal")?,
            json!({"duration_seconds":duration_seconds,"extension_seconds":extension_seconds}),
            &write_token,
        ),
        Operation::DelegatedRenew { lease } => send(
            client,
            &format!("/v1/delegated-renewal/{}", check_lease_id(&lease)?),
            json!({}),
        ),
        Operation::RevokeRenewal { lease } => {
            scoped(client, &lease, "/delegate-renewal", json!({"revoke":true}))
        }
        Operation::Checkpoint {
            lease,
            archive,
            request_key,
        } => client.upload(
            &route(&lease, "/checkpoint-upload")?,
            &archive,
            request_key.as_deref(),
        ),
        Operation::Recover {
            lease,
            output,
            checkpoint_id,
        } => {
            ensure_output_available(&output)?;
            let grant = scoped(
                client,
                &lease,
                "/checkpoint-recovery",
                json!({"checkpoint_id":checkpoint_id}),
            )?;
            client.download(&grant, &output)
        }
        Operation::Pin {
            lease,
            reason,
            checkpoint_id,
        } => scoped(
            client,
            &lease,
            "/checkpoint-pin",
            json!({"pinned":true,"reason":reason,"checkpoint_id":checkpoint_id}),
        ),
        Operation::Unpin {
            lease,
            checkpoint_id,
        } => scoped(
            client,
            &lease,
            "/checkpoint-pin",
            json!({"pinned":false,"reason":null,"checkpoint_id":checkpoint_id}),
        ),
        Operation::ReserveCapacity {
            request_key,
            purpose,
            slot,
            memory_mib,
            storage_mib,
        } => send(
            client,
            "/v1/capacity-reservations",
            json!({"request_key":request_key,"purpose":purpose,"memory_mib":memory_mib,"storage_mib":storage_mib,"slot_id":slot}),
        ),
        Operation::ListReservations => client.request("/v1/capacity-reservations", None),
        Operation::ReleaseReservation { reservation } => send(
            client,
            &format!(
                "/v1/capacity-reservations/{}/release",
                check_lease_id(&reservation)?
            ),
            json!({}),
        ),
        Operation::IssueTaskCredential {
            task_ref,
            seconds,
            write_token,
        } => private_grant(
            client,
            "/v1/task-credentials",
            json!({"task_ref":task_ref,"seconds":seconds}),
            &write_token,
        ),
        Operation::RevokeTaskCredential { task_ref } => send(
            client,
            "/v1/task-credentials",
            json!({"task_ref":task_ref,"revoke":true}),
        ),
    }
}

fn main() -> ExitCode {
    match execute(Args::parse()) {
        Ok(document) => {
            println!("{document}");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("{}", problem.response);
            ExitCode::from(problem.exit_code())
        }
    }
}
