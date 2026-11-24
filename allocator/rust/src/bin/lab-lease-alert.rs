use std::{
    collections::BTreeMap,
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
    #[serde(default)]
    delivery_command: Vec<String>,
    #[serde(default)]
    delivery_by_owner: BTreeMap<String, Vec<String>>,
    lock_file: PathBuf,
}

impl Settings {
    fn recipient(&self, owner: &str) -> &[String] {
        self.delivery_by_owner
            .get(owner)
            .unwrap_or(&self.delivery_command)
    }
}

fn run(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let settings: Settings = serde_json::from_reader(File::open(path)?)?;
    let Some(_guard) = lab_allocator::worker_lock(&settings.lock_file)? else {
        return Ok(());
    };
    let client = lab_allocator::task_client::TaskClient::new(
        &settings.endpoint,
        settings.ca_file.clone(),
        settings.alert_token_file.clone(),
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
    let owner = envelope["owner"].as_str().ok_or("Missing alert owner.")?;
    let [program, args @ ..] = settings.recipient(owner) else {
        return Err("No alert recipient is configured for this owner.".into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_an_exact_owner_uses_a_dedicated_recipient() {
        let settings: Settings = serde_json::from_value(json!({
            "endpoint":"https://controller.example.invalid",
            "ca_file":"/ca", "alert_token_file":"/token", "lock_file":"/lock",
            "delivery_command":["/inbox","operator"],
            "delivery_by_owner":{"build-42":["/inbox","build-42"],"disabled":[]}
        }))
        .unwrap();
        assert_eq!(settings.recipient("build-42"), ["/inbox", "build-42"]);
        assert_eq!(settings.recipient("build-421"), ["/inbox", "operator"]);
        assert!(settings.recipient("disabled").is_empty());
    }
}
