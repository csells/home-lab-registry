use std::{error::Error, io, path::Path, process::ExitCode};

use lab_workspace as workspace;

#[cfg(target_os = "linux")]
fn limits() -> io::Result<()> {
    for (resource, value) in [(libc::RLIMIT_AS, 192 * 1024 * 1024), (libc::RLIMIT_CPU, 60)] {
        let limit = libc::rlimit {
            rlim_cur: value,
            rlim_max: value,
        };
        // Resource limits apply only to this dedicated validation process.
        if unsafe { libc::setrlimit(resource, &limit) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
#[cfg(not(target_os = "linux"))]
fn limits() -> io::Result<()> {
    Ok(())
}

fn execute() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let stdin = io::stdin().lock();
    match args.as_slice() {
        [flag, target] if flag == "--validate" => {
            limits()?;
            workspace::extract_workspace(stdin, Path::new(target))?;
        }
        [digest, lease, ..] if args.len() <= 3 => {
            let declared = args.get(2).map(Path::new);
            println!("{}", workspace::restore(stdin, digest, lease, declared)?);
        }
        _ => return Err(
            "Usage: lab-restore-workspace SHA256 LEASE [WORKSPACE] | --validate EMPTY_DIRECTORY"
                .into(),
        ),
    }
    Ok(())
}

fn main() -> ExitCode {
    match execute() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Workspace restore failed: {error}");
            ExitCode::FAILURE
        }
    }
}
