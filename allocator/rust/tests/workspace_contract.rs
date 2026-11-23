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
