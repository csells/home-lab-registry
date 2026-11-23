use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    fs::{self, File, FileTimes, OpenOptions},
    io::{self, Read, Seek, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::{APPLE_WORKSPACE, MAX_ARCHIVE, decode_gzip, digest_to};
use flate2::{Compression, write::GzEncoder};
use serde_json::{Value, json};

fn hex(text: &str, size: usize) -> bool {
    text.len() == size
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub const MAX_EXPANDED: u64 = 512 * 1024 * 1024;
pub const MAX_MEMBERS: usize = 20_000;
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

pub fn extract_workspace(source: impl Read, target: &Path) -> io::Result<()> {
    verify_workspace_with_limit(source, target, MAX_EXPANDED)
}

pub fn verify_workspace_with_limit(source: impl Read, target: &Path, limit: u64) -> io::Result<()> {
    let mut unpacked = tempfile::tempfile_in(target.parent().ok_or_else(escaped)?)?;
    decode_gzip(source, &mut unpacked, limit)?;
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
    let mut directory_times = HashMap::new();
    let mut regular_paths = HashSet::new();
    for (index, entry) in archive.entries()?.enumerate() {
        let mut entry = entry?;
        bytes = bytes.checked_add(entry.size()).ok_or_else(budget)?;
        if index >= MAX_MEMBERS || bytes > limit {
            return Err(budget());
        }
        let path = entry.path()?.into_owned();
        if path.strip_prefix(".").unwrap_or(&path) == Path::new(RECEIPT) {
            continue;
        }
        let leaf = contained(&root, &path, false)?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            if leaf.symlink_metadata().is_ok_and(|info| info.is_symlink()) {
                return Err(invalid("Archive directory is redirected."));
            }
            private_directory(&leaf)?;
            directory_times.insert(leaf, archive_mtime(&mut entry)?);
            continue;
        }
        if leaf == root {
            return Err(escaped());
        }
        let parent = leaf.parent().ok_or_else(escaped)?;
        private_directory(parent)?;
        if kind.is_file() || kind.is_gnu_sparse() || kind.is_contiguous() {
            let mode = entry.header().mode()?;
            let safe = if mode & 0o100 != 0 { 0o755 } else { 0o644 };
            let permissions = fs::Permissions::from_mode(0o600 | (mode & safe));
            let modified = FileTimes::new().set_modified(archive_mtime(&mut entry)?);
            // A duplicate regular member replaces its directory entry, preserving
            // any earlier hard links to the original inode.
            let mut file = tempfile::NamedTempFile::new_in(parent)?;
            io::copy(&mut entry, file.as_file_mut())?;
            file.as_file().set_permissions(permissions)?;
            file.as_file().set_times(modified)?;
            if regular_paths.insert(leaf.clone()) {
                if let Err(error) = file.persist_noclobber(&leaf) {
                    // Darwin reports ENOTSUP separately from EOPNOTSUPP.
                    if error.error.kind() != io::ErrorKind::Unsupported
                        && error.error.raw_os_error() != Some(libc::ENOTSUP)
                    {
                        return Err(error.into());
                    }
                    // Tart's VirtioFS share supports hard links but cannot
                    // rename with RENAME_EXCL. Linking still refuses existing
                    // entries, and dropping the temporary file unlinks it.
                    fs::hard_link(error.file.path(), &leaf)?;
                }
            } else {
                file.persist(&leaf)?;
            }
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
    for (path, modified) in directory_times {
        File::open(path)?.set_times(FileTimes::new().set_modified(modified))?;
    }
    Ok(())
}

fn archive_mtime<R: Read>(entry: &mut tar::Entry<'_, R>) -> io::Result<SystemTime> {
    let bad_timestamp = || invalid("Unsupported checkpoint timestamp.");
    let from_epoch = |negative, duration| {
        if negative {
            UNIX_EPOCH.checked_sub(duration)
        } else {
            UNIX_EPOCH.checked_add(duration)
        }
        .ok_or_else(bad_timestamp)
    };
    if let Some(extensions) = entry.pax_extensions()? {
        let mut mtime = None;
        for extension in extensions {
            let extension = extension?;
            if extension.key_bytes() == b"mtime" {
                let text = extension.value().map_err(|_| bad_timestamp())?;
                let (negative, value) = text
                    .strip_prefix('-')
                    .map_or((false, text), |value| (true, value));
                let (whole, decimal) = value.split_once('.').unwrap_or((value, ""));
                let seconds = whole.parse().map_err(|_| bad_timestamp())?;
                if !decimal.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(bad_timestamp());
                }
                let nanos = decimal
                    .bytes()
                    .map(|byte| byte - b'0')
                    .chain(std::iter::repeat(0))
                    .take(9)
                    .fold(0_u32, |value, digit| value * 10 + u32::from(digit));
                mtime = Some(from_epoch(negative, Duration::new(seconds, nanos))?);
            }
        }
        if let Some(time) = mtime {
            return Ok(time);
        }
    }
    let raw = entry.header().as_old().mtime;
    if raw[0] == 0xff {
        let mut signed = [0xff; 16];
        signed[4..].copy_from_slice(&raw);
        let seconds = u64::try_from(i128::from_be_bytes(signed).unsigned_abs())
            .map_err(|_| bad_timestamp())?;
        return from_epoch(true, Duration::from_secs(seconds));
    }
    from_epoch(false, Duration::from_secs(entry.header().mtime()?))
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
    sync_restore_marker(&marker)?;
    fs::rename(&target, &workspace)?;
    Ok("restored")
}

#[cfg(target_os = "macos")]
fn sync_restore_marker(marker: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // macOS File::sync_all uses F_FULLFSYNC, which VirtioFS shared folders
    // refuse. The standard fsync works on the shared workspace.
    if unsafe { libc::fsync(marker.as_raw_fd()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn sync_restore_marker(marker: &File) -> io::Result<()> {
    marker.sync_all()
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
            let (sign, elapsed) = match metadata.modified()?.duration_since(UNIX_EPOCH) {
                Ok(elapsed) => ("", elapsed),
                Err(before_epoch) => ("-", before_epoch.duration()),
            };
            if !sign.is_empty() || elapsed.subsec_nanos() != 0 {
                // The standard tar header stores whole seconds. PAX preserves the exact time.
                let mtime = format!("{sign}{}.{:09}", elapsed.as_secs(), elapsed.subsec_nanos());
                archive.append_pax_extensions([("mtime", mtime.as_bytes())])?;
            }
            archive.append_path_with_name(&path, relative)?;
        }
        pending.extend(directories.into_iter().rev());
    }
    archive.into_inner()?.finish()?.flush()
}
