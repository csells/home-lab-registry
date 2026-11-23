use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Parser;
use serde_json::{Value, json};

const FILES: &[&str] = &[
    "etc/systemd/system/backrest.service",
    "opt/backrest/config/config.json",
    "opt/backrest/deployment/RECOVERY.md",
    "opt/backrest/deployment/ansible/central.yml",
    "opt/backrest/deployment/manifest.json",
    "opt/backrest/private/rclone.conf",
];

#[derive(Parser)]
#[command(about = "Verify the restored Backrest bootstrap inside an isolated restore root.")]
struct Options {
    restore_root: PathBuf,
}

fn invalid(summary: &'static str) -> io::Error {
    io::Error::other(summary)
}

fn index<'a>(items: &'a Value, failure: &'static str) -> io::Result<BTreeMap<&'a str, &'a Value>> {
    let items = items
        .as_array()
        .filter(|rows| !rows.is_empty())
        .ok_or_else(|| invalid(failure))?;
    let mut indexed = BTreeMap::new();
    for row in items {
        let id = row["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| invalid(failure))?;
        if indexed.insert(id, row).is_some() {
            return Err(invalid(failure));
        }
    }
    Ok(indexed)
}

fn verify(root: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    if root == Path::new("/") {
        return Err(invalid("Use an isolated restore directory.").into());
    }
    for file in FILES {
        let path = root.join(file);
        if path.canonicalize().ok().as_ref() != Some(&path)
            || path.metadata().is_err()
            || !path
                .metadata()
                .is_ok_and(|info| info.is_file() && info.len() > 0)
        {
            return Err(
                io::Error::other(format!("Missing or invalid recovery file: {file}")).into(),
            );
        }
    }
    let config: Value = serde_json::from_reader(File::open(root.join(FILES[1]))?)?;
    let repositories = index(
        &config["repos"],
        "Missing, duplicate or invalid repository declarations.",
    )?;
    let plans = index(
        &config["plans"],
        "Missing, duplicate or invalid plan declarations.",
    )?;
    if repositories
        .values()
        .any(|repo| repo["uri"].as_str().is_none_or(str::is_empty))
    {
        return Err(invalid("Missing repository location.").into());
    }
    if plans.values().any(|plan| {
        plan["repo"]
            .as_str()
            .is_none_or(|repo| !repositories.contains_key(repo))
    }) {
        return Err(invalid("A plan points to an unknown repository.").into());
    }
    let manifest: Value = serde_json::from_reader(File::open(root.join(FILES[4]))?)?;
    if manifest.as_object().is_none_or(serde_json::Map::is_empty) {
        return Err(invalid("Missing deployment manifest.").into());
    }
    if !fs::read_to_string(root.join(FILES[0]))?.contains("ExecStart=") {
        return Err(invalid("Missing Backrest service start command.").into());
    }
    Ok(json!({"bootstrap_files":FILES.len(),"repositories":repositories.len(),"plans":plans.len()}))
}

fn main() -> ExitCode {
    match verify(&Options::parse().restore_root) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(_) => {
            eprintln!("Controller restore verification failed.");
            ExitCode::FAILURE
        }
    }
}
