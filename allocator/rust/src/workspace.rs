use std::{
    collections::VecDeque,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, symlink},
    path::{Component, Path, PathBuf},
};

use flate2::{Compression, read::MultiGzDecoder, write::GzEncoder};
use ring::digest::{Context, SHA256};
use serde_json::{Value, json};

use crate::tokens::hex;

pub const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
pub const MAX_EXPANDED: u64 = 512 * 1024 * 1024;
pub const MAX_MEMBERS: usize = 20_000;
pub const APPLE_WORKSPACE: &str = "/Volumes/My Shared Files/workspace/project";
const RECEIPT: &str = ".lease-restore-receipt";

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn escaped() -> io::Error {
    invalid("Archive entry or link points outside the workspace.")
}

fn budget() -> io::Error {
    invalid("Expanded checkpoint exceeds the workspace budget.")
}

fn private_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

fn private_file(path: &Path, mode: u32) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
}

#[derive(Clone)]
enum Step {
    Name(OsString),
    Parent,
}

fn steps(path: &Path) -> io::Result<VecDeque<Step>> {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(Ok(Step::Name(name.to_owned()))),
            Component::ParentDir => Some(Ok(Step::Parent)),
            Component::CurDir => None,
            Component::RootDir | Component::Prefix(_) => Some(Err(escaped())),
        })
        .collect()
}

// Normalize only within the workspace. Resolve existing symlinks using their
// relative parents, so links to future paths are checked against the same root.
fn contained(root: &Path, path: &Path, follow_leaf: bool) -> io::Result<PathBuf> {
    let mut pending = steps(path)?;
    let mut resolved = PathBuf::new();
    let mut links = 0;
    while let Some(next) = pending.pop_front() {
        match next {
            Step::Parent => {
                if !resolved.pop() {
                    return Err(escaped());
                }
            }
            Step::Name(name) => {
                let candidate = root.join(&resolved).join(&name);
                if follow_leaf || !pending.is_empty() {
                    match fs::symlink_metadata(&candidate) {
                        Ok(info) if info.is_symlink() => {
                            links += 1;
                            if links > 40 {
                                return Err(escaped());
                            }
                            let mut target = steps(&fs::read_link(&candidate)?)?;
                            target.append(&mut pending);
                            pending = target;
                            continue;
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                resolved.push(name);
            }
        }
    }
    Ok(root.join(resolved))
}

pub fn digest_to(
    mut input: impl Read,
    mut output: impl Write,
    limit: u64,
) -> io::Result<(String, u64)> {
    let mut hash = Context::new(&SHA256);
    let mut buffer = [0_u8; 65_536];
    let mut bytes = 0;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > limit {
            return Err(invalid("Recovery archive exceeds the supported size."));
        }
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
    }
    Ok((
        hash.finish()
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        bytes,
    ))
}

pub fn extract_workspace(source: impl Read, target: &Path) -> io::Result<()> {
    verify_workspace_with_limit(source, target, MAX_EXPANDED)
}

pub fn verify_workspace_with_limit(source: impl Read, target: &Path, limit: u64) -> io::Result<()> {
    let mut unpacked = tempfile::tempfile_in(target.parent().ok_or_else(escaped)?)?;
    let copied = io::copy(
        &mut MultiGzDecoder::new(source).take(limit.saturating_add(1)),
        &mut unpacked,
    )?;
    if copied > limit {
        return Err(budget());
    }
    unpacked.rewind()?;
    extract_tar(unpacked, target, limit)
}

fn extract_tar(source: impl Read, target: &Path, limit: u64) -> io::Result<()> {
    let root = target.canonicalize()?;
    if !root.is_dir() || target.symlink_metadata()?.is_symlink() {
        return Err(invalid("The declared workspace parent is unavailable."));
    }
    let mut archive = tar::Archive::new(source);
    let mut bytes = 0_u64;
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        bytes = bytes.checked_add(entry.size()).ok_or_else(budget)?;
        if index >= MAX_MEMBERS || bytes > limit {
            return Err(budget());
        }
        let path = entry.path()?.into_owned();
        if path == Path::new(RECEIPT) {
            continue;
        }
        let leaf = contained(&root, &path, false)?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            if leaf.symlink_metadata().is_ok_and(|info| info.is_symlink()) {
                return Err(invalid("Archive directory is redirected."));
            }
            private_directory(&leaf)?;
            continue;
        }
        if leaf == root {
            return Err(escaped());
        }
        let parent = leaf.parent().ok_or_else(escaped)?;
        private_directory(parent)?;
        if kind.is_file() || kind.is_gnu_sparse() || kind.is_contiguous() {
            let mode = entry.header().mode()?;
            let execute = if mode & 0o100 != 0 { mode & 0o111 } else { 0 };
            let mut file = private_file(&leaf, 0o600 | execute)?;
            io::copy(&mut entry, &mut file)?;
        } else if kind.is_symlink() {
            let link = entry.link_name()?.ok_or_else(escaped)?;
            let mut relative = parent
                .strip_prefix(&root)
                .map_err(|_| escaped())?
                .to_owned();
            if link.is_absolute() {
                return Err(escaped());
            }
            relative.push(&*link);
            contained(&root, &relative, true)?;
            symlink(&*link, leaf)?;
        } else if kind.is_hard_link() {
            let link = entry.link_name()?.ok_or_else(escaped)?;
            let previous = contained(&root, &link, false)?;
            let metadata = previous.symlink_metadata()?;
            if !metadata.is_file() || metadata.is_symlink() {
                return Err(invalid("Archive hard link has no earlier regular source."));
            }
            fs::hard_link(previous, leaf)?;
        } else {
            return Err(invalid("Unsupported file type in checkpoint."));
        }
    }
    private_file(&root.join(RECEIPT), 0o600)?;
    Ok(())
}

pub fn restore(
    source: impl Read,
    digest: &str,
    lease: &str,
    declared: Option<&Path>,
) -> io::Result<&'static str> {
    if !hex(digest, 64) || !hex(lease, 32) {
        return Err(invalid("Invalid restore identity."));
    }
    let workspace = match declared {
        Some(path) if path == Path::new(APPLE_WORKSPACE) => path.to_owned(),
        Some(_) => return Err(invalid("Invalid declared workspace.")),
        None => PathBuf::from(
            std::env::var_os("HOME").ok_or_else(|| invalid("Task workspace home is missing."))?,
        )
        .join("workspace"),
    };
    let parent = workspace.parent().ok_or_else(escaped)?;
    if !parent.is_dir() || parent.symlink_metadata()?.is_symlink() {
        return Err(invalid("The declared workspace parent is unavailable."));
    }
    let temporary = tempfile::tempdir_in(parent)?;
    let mut verified = tempfile::tempfile_in(temporary.path())?;
    if digest_to(source, &mut verified, MAX_ARCHIVE)?.0 != digest {
        return Err(invalid(
            "Downloaded checkpoint failed verification in the guest.",
        ));
    }
    let target = temporary.path().join("workspace");
    private_directory(&target)?;
    let identity = json!({"sha256":digest,"lease":lease});
    match workspace.symlink_metadata() {
        Ok(metadata) if metadata.is_symlink() => {
            return Err(invalid("Existing workspace is a symbolic link."));
        }
        Ok(metadata) if metadata.is_dir() => {
            let marker = workspace.join(RECEIPT);
            if marker
                .symlink_metadata()
                .is_ok_and(|info| info.is_file() && !info.is_symlink() && info.len() < 256)
            {
                let value: Value = serde_json::from_reader(File::open(marker)?)?;
                if value == identity {
                    return Ok("previously-restored");
                }
            }
            if fs::read_dir(&workspace)?.next().transpose()?.is_some() {
                return Err(invalid("An existing workspace already contains data."));
            }
        }
        Ok(_) => return Err(invalid("An existing workspace is not a directory.")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    verified.rewind()?;
    extract_workspace(verified, &target)?;
    let mut marker = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(target.join(RECEIPT))?;
    serde_json::to_writer(&mut marker, &identity)?;
    marker.sync_all()?;
    fs::rename(&target, &workspace)?;
    Ok("restored")
}

struct LimitedWriter<W> {
    inner: W,
    remaining: u64,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.len() as u64 > self.remaining {
            return Err(invalid(
                "Compressed workspace exceeds the checkpoint budget.",
            ));
        }
        let count = self.inner.write(data)?;
        self.remaining -= count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

pub fn archive_workspace(workspace: &Path, output: impl Write) -> io::Result<()> {
    if !workspace.is_absolute() || workspace.canonicalize()? != workspace || !workspace.is_dir() {
        return Err(invalid("Use the exact absolute task workspace."));
    }
    let device = workspace.metadata()?.dev();
    let gz = GzEncoder::new(
        LimitedWriter {
            inner: output,
            remaining: MAX_ARCHIVE,
        },
        Compression::default(),
    );
    let mut archive = tar::Builder::new(gz);
    archive.follow_symlinks(false);
    let mut pending = vec![workspace.to_owned()];
    let mut entries = 0;
    let mut bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        let mut children = fs::read_dir(&directory)?.collect::<Result<Vec<_>, _>>()?;
        children.sort_unstable_by_key(|entry| entry.file_name());
        let mut directories = Vec::new();
        for entry in children {
            entries += 1;
            let path = entry.path();
            let metadata = path.symlink_metadata()?;
            if entries > MAX_MEMBERS || metadata.dev() != device {
                return Err(invalid(
                    "Workspace exceeds its entry budget or crosses a filesystem.",
                ));
            }
            let relative = path.strip_prefix(workspace).map_err(|_| escaped())?;
            if metadata.is_symlink() {
                let link = fs::read_link(&path)?;
                let parent = relative.parent().ok_or_else(escaped)?;
                if link.is_absolute() {
                    return Err(escaped());
                }
                contained(workspace, &parent.join(link), true)?;
            } else if metadata.is_file() {
                bytes = bytes.checked_add(metadata.len()).ok_or_else(budget)?;
                if metadata.nlink() != 1 || bytes > MAX_EXPANDED {
                    return Err(invalid(
                        "Workspace contains a hard link or exceeds the byte budget.",
                    ));
                }
            } else if metadata.is_dir() {
                directories.push(path.clone());
            } else {
                return Err(invalid("Workspace contains an unsupported entry."));
            }
            archive.append_path_with_name(&path, relative)?;
        }
        pending.extend(directories.into_iter().rev());
    }
    archive.into_inner()?.finish()?.flush()
}
