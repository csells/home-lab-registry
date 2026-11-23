use std::{
    fs::File,
    io::{self, Read},
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};

use wait_timeout::ChildExt;

pub fn bounded(
    command: &mut Command,
    input: Option<&File>,
    timeout: Duration,
    limit: u64,
) -> io::Result<Output> {
    let stdin = input
        .map(File::try_clone)
        .transpose()?
        .map_or_else(Stdio::null, Stdio::from);
    let mut child = command
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let read = |pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.take(limit + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > limit {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Command output exceeded its limit",
                ));
            }
            Ok(bytes)
        })
    };
    let stdout = read(Box::new(
        child.stdout.take().expect("piped output available"),
    ));
    let stderr = read(Box::new(
        child.stderr.take().expect("piped output available"),
    ));
    let status = match child.wait_timeout(timeout) {
        Ok(Some(status)) => status,
        pending => {
            let killed = child.kill();
            let waited = child.wait();
            if let Err(error) = pending {
                return Err(error);
            }
            if let Err(error) = killed
                && error.kind() != io::ErrorKind::InvalidInput
            {
                return Err(error);
            }
            waited?;
            return Err(io::Error::new(io::ErrorKind::TimedOut, "Command timed out"));
        }
    };
    let complete = |worker: thread::JoinHandle<io::Result<Vec<u8>>>| {
        worker
            .join()
            .map_err(|_| io::Error::other("Command output worker failed"))?
    };
    Ok(Output {
        status,
        stdout: complete(stdout)?,
        stderr: complete(stderr)?,
    })
}

pub(crate) fn shell_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
