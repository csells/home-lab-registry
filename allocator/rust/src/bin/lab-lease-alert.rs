use std::{
    fs::File,
    io::{Seek, Write},
    path::PathBuf,
    process::ExitCode,
    time::Duration,
};

use clap::Parser;
use lab_allocator::deliver_alert;
use serde::Deserialize;

#[derive(Parser)]
#[command(about = "Deliver one pending export failure through the configured alert command.")]
struct Command {
    configuration: PathBuf,
}

#[derive(Deserialize)]
struct Settings {
    endpoint: String,
    ca_file: PathBuf,
    alert_token_file: PathBuf,
    delivery_command: Vec<String>,
    lock_file: PathBuf,
}

fn run(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let settings: Settings = serde_json::from_reader(File::open(path)?)?;
    let Some(_guard) = lab_allocator::worker_lock(&settings.lock_file)? else {
        return Ok(());
    };
    let client = lab_allocator::task_client::TaskClient::new(
        &settings.endpoint,
        settings.ca_file,
        settings.alert_token_file,
    )?;
    let pending = client.request("/v1/export-alerts", None)?;
    if pending.get("alert").is_none_or(serde_json::Value::is_null) {
        return Ok(());
    }
    let envelope = deliver_alert(&pending["alert"])?;
    let mut bytes = tempfile::tempfile()?;
    serde_json::to_writer(&mut bytes, &envelope)?;
    writeln!(bytes)?;
    bytes.rewind()?;
    let [program, args @ ..] = settings.delivery_command.as_slice() else {
        return Err("An alert delivery command is required.".into());
    };
    let mut command = std::process::Command::new(program);
    command.args(args);
    lab_allocator::run_alert_recipient(&mut command, &bytes, Duration::from_secs(20))?;
    let receipt = client.request(
        "/v1/export-alerts",
        Some(&serde_json::json!({"alert_id":envelope["id"]})),
    )?;
    if receipt != serde_json::json!({"delivered":true}) {
        return Err("Controller did not acknowledge alert delivery.".into());
    }
    println!(
        "Export alert delivered: {}",
        envelope["id"].as_str().unwrap_or_default()
    );
    Ok(())
}

fn main() -> ExitCode {
    let options = Command::parse();
    unsafe { libc::umask(0o077) };
    match run(options.configuration) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Export alert needs retry: {error}");
            ExitCode::FAILURE
        }
    }
}
