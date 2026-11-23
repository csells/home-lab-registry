use std::{io, path::PathBuf, process::ExitCode};

use lab_allocator::workspace::archive_workspace;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(path), None) = (args.next(), args.next()) else {
        eprintln!("Usage: lab-archive-workspace WORKSPACE");
        return ExitCode::FAILURE;
    };
    match archive_workspace(&PathBuf::from(path), io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Workspace archive failed: {error}");
            ExitCode::FAILURE
        }
    }
}
