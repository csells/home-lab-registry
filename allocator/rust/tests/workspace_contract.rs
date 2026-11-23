use std::{
    fs,
    io::Write,
    os::unix::fs::symlink,
    process::{Command, Stdio},
};

use flate2::{Compression, write::GzEncoder};
use lab_allocator::workspace::{archive_workspace, extract_workspace, verify_workspace_with_limit};
use ring::digest::{SHA256, digest};
use tar::{Builder, EntryType, Header};

fn archive(file: &[u8], link: Option<&str>) -> Vec<u8> {
    let mut archive = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = Header::new_gnu();
    header.set_size(file.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "café-東京-🐍.bin", file)
        .unwrap();
    if let Some(target) = link {
        let mut link = Header::new_gnu();
        link.set_entry_type(EntryType::Symlink);
        link.set_size(0);
        link.set_mode(0o777);
        archive.append_link(&mut link, "work-link", target).unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap()
}

fn hex(data: &[u8]) -> String {
    digest(&SHA256, data)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn native_guest_restore_replays_without_overwriting_edits_and_refuses_wrong_digest() {
    let root = tempfile::tempdir().unwrap();
    let bytes = archive(b"hello", Some("café-東京-🐍.bin"));
    let id = "1".repeat(32);
    let attempt = |digest: &str| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lab-restore-workspace"))
            .args([digest, &id])
            .env("HOME", root.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
        child.wait_with_output().unwrap()
    };
    let bad = attempt(&"0".repeat(64));
    assert!(!bad.status.success());
    assert!(!root.path().join("workspace").exists());
    let first = attempt(&hex(&bytes));
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let file = root.path().join("workspace/café-東京-🐍.bin");
    assert_eq!(
        fs::read(root.path().join("workspace/work-link")).unwrap(),
        b"hello"
    );
    fs::write(&file, b"later edits").unwrap();
    let result = attempt(&hex(&bytes));
    assert_eq!(result.stdout, b"previously-restored\n");
    assert_eq!(fs::read(&file).unwrap(), b"later edits");
    fs::remove_file(root.path().join("workspace/.lease-restore-receipt")).unwrap();
    assert!(!attempt(&hex(&bytes)).status.success());
    assert_eq!(fs::read(file).unwrap(), b"later edits");
}

#[test]
fn links_and_archive_metadata_cannot_escape_the_workspace_or_budget() {
    let root = tempfile::tempdir().unwrap();
    let invalid = archive(b"hello", Some("../outside.txt"));
    assert!(extract_workspace(invalid.as_slice(), root.path()).is_err());
    assert!(!root.path().join("work-link").exists());
    let valid = archive(b"hello", None);
    assert!(verify_workspace_with_limit(valid.as_slice(), root.path(), 512).is_err());
    let truncated = &valid[..valid.len() - 5];
    assert!(extract_workspace(truncated, root.path()).is_err());
}

#[test]
fn workspace_round_trip_preserves_exact_filenames_data_and_internal_symlinks() {
    let source = tempfile::tempdir().unwrap();
    let root = source.path().canonicalize().unwrap();
    fs::create_dir(root.join("sub")).unwrap();
    fs::write(root.join("sub/café-東京-🐍.bin"), b"payload").unwrap();
    symlink("sub/café-東京-🐍.bin", root.join("link")).unwrap();
    let mut bytes = Vec::new();
    archive_workspace(&root, &mut bytes).unwrap();
    let destination = tempfile::tempdir().unwrap();
    extract_workspace(bytes.as_slice(), destination.path()).unwrap();
    assert_eq!(
        fs::read(destination.path().join("link")).unwrap(),
        b"payload"
    );
    fs::hard_link(root.join("sub/café-東京-🐍.bin"), root.join("hardlink")).unwrap();
    assert!(archive_workspace(&root, Vec::new()).is_err());
}

#[test]
fn macos_tar_padding_is_accepted_but_nonzero_garbage_is_rejected() {
    let mut input = archive(b"native BSD tar writes zero padding", None);
    input.resize(10240, 0);
    let valid = tempfile::tempdir().unwrap();
    lab_allocator::workspace::extract_workspace(input.as_slice(), valid.path()).unwrap();
    assert!(valid.path().read_dir().unwrap().next().is_some());
    input[10239] = 1;
    let invalid = tempfile::tempdir().unwrap();
    assert!(lab_allocator::workspace::extract_workspace(input.as_slice(), invalid.path()).is_err());
}

#[test]
fn reexport_regenerates_a_receipt_written_with_a_dot_prefix() {
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("workspace.txt"), b"restored data").unwrap();
    fs::write(
        source.path().join(".lease-restore-receipt"),
        b"receipt from the original lease",
    )
    .unwrap();
    let archive = Command::new("tar")
        .args(["-czf", "-", "-C"])
        .arg(source.path())
        .arg(".")
        .output()
        .unwrap();
    assert!(archive.status.success());

    let restored = tempfile::tempdir().unwrap();
    extract_workspace(archive.stdout.as_slice(), restored.path()).unwrap();
    assert_eq!(
        fs::read(restored.path().join("workspace.txt")).unwrap(),
        b"restored data"
    );
    assert_eq!(
        fs::read(restored.path().join(".lease-restore-receipt")).unwrap(),
        b""
    );
}

#[test]
fn restoring_a_workspace_keeps_safe_modes_and_timestamps() {
    use std::{
        os::unix::fs::PermissionsExt,
        time::{Duration, UNIX_EPOCH},
    };
    let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut dir = Header::new_gnu();
    dir.set_entry_type(EntryType::Directory);
    dir.set_size(0);
    dir.set_mode(0o777);
    dir.set_mtime(1_700_000_000);
    tar.append_data(&mut dir, "source", &[][..]).unwrap();
    let mut file = Header::new_gnu();
    file.set_size(3);
    file.set_mode(0o6775);
    file.set_mtime(1_700_000_010);
    tar.append_data(&mut file, "source/run", &b"run"[..])
        .unwrap();
    let bytes = tar.into_inner().unwrap().finish().unwrap();

    let destination = tempfile::tempdir().unwrap();
    extract_workspace(bytes.as_slice(), destination.path()).unwrap();
    let file = fs::metadata(destination.path().join("source/run")).unwrap();
    assert_eq!(file.permissions().mode() & 0o7777, 0o755);
    assert_eq!(
        file.modified().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_010)
    );
    let dir = fs::metadata(destination.path().join("source")).unwrap();
    assert_eq!(dir.permissions().mode() & 0o7777, 0o700);
    assert_eq!(
        dir.modified().unwrap(),
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    );
}

#[test]
fn a_later_regular_archive_entry_supersedes_the_first_without_editing_its_hard_links() {
    let mut tar = Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    let mut header = Header::new_gnu();
    header.set_size(3);
    header.set_mode(0o644);
    tar.append_data(&mut header, "entry", &b"old"[..]).unwrap();
    let mut link = Header::new_gnu();
    link.set_size(0);
    link.set_mode(0o644);
    link.set_entry_type(EntryType::Link);
    tar.append_link(&mut link, "copy", "entry").unwrap();
    tar.append_data(&mut header, "entry", &b"new"[..]).unwrap();
    let bytes = tar.into_inner().unwrap().finish().unwrap();

    let destination = tempfile::tempdir().unwrap();
    extract_workspace(bytes.as_slice(), destination.path()).unwrap();
    assert_eq!(fs::read(destination.path().join("entry")).unwrap(), b"new");
    assert_eq!(fs::read(destination.path().join("copy")).unwrap(), b"old");
}
