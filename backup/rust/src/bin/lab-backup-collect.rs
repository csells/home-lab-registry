use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, ValueEnum};

#[derive(Clone, ValueEnum)]
enum Action {
    Clean,
}

#[derive(Parser)]
#[command(about = "Collect one fresh backup export through its pinned SSH identity.")]
struct Options {
    plan: String,
    action: Option<Action>,
    #[arg(long, default_value = "/opt/backrest")]
    base: PathBuf,
}

fn main() -> ExitCode {
    let options = Options::parse();
    unsafe { libc::umask(0o077) };
    match lab_backup::collect(&options.base, &options.plan, options.action.is_some()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Backup collection failed: {error}");
            ExitCode::FAILURE
        }
    }
}
