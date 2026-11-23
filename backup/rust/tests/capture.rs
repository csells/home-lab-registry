use std::{fs, io::Write};

use flate2::{Compression, write::GzEncoder};
use lab_backup::{guest_request, verified_address, verify_archive};
use serde_json::json;

#[test]
fn mac_pax_archive_verifies_in_place_and_requires_a_complete_gzip_trailer() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.tar.gz");
    let mut output = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    output
        .append_pax_extensions([("LIBARCHIVE.xattr.com.apple.provenance", &b"opaque"[..])])
        .unwrap();
    let mut header = tar::Header::new_ustar();
    header.set_size(4096);
    header.set_mode(0o600);
    let data: Vec<u8> = (0..4096)
        .map(|i| ((i * 221311 + 1087) % 223) as u8)
        .collect();
    output
        .append_data(&mut header, "config.json", data.as_slice())
        .unwrap();
    let bytes = output.into_inner().unwrap().finish().unwrap();
    fs::write(&path, &bytes).unwrap();
    verify_archive(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::write(&path, &bytes[..bytes.len() - 5]).unwrap();
    assert!(verify_archive(&path).is_err());
    fs::File::create(&path)
        .unwrap()
        .write_all(&b"invalid archive".repeat(100))
        .unwrap();
    assert!(verify_archive(&path).is_err());
}

#[test]
fn source_identity_must_agree_before_an_address_is_used() {
    let mut binding = json!({"vmid":123,"guest_kind":"qemu","uuid":"stable","mac":"bc:24:11:00:00:01",
        "allowed_networks":["192.0.2.0/24"]});
    let mut observed = json!({"vmid":123,"kind":"qemu","uuid":"stable","state":"running","observed_at":1000,
        "interfaces":[{"hardware-address":"BC:24:11:00:00:01","ip-addresses":[{"ip-address":"192.0.2.42"}]}]});
    assert_eq!(
        verified_address(&binding, &observed, 1001.)
            .unwrap()
            .to_string(),
        "192.0.2.42"
    );
    assert!(verified_address(&binding, &observed, 1061.).is_err());
    observed["uuid"] = json!("different");
    assert!(verified_address(&binding, &observed, 1001.).is_err());
    binding["guest_kind"] = json!("lxc");
    binding["volume_id"] = json!("pool:vm-123-disk-0");
    observed["kind"] = json!("lxc");
    observed["volume_id"] = json!("pool:vm-123-disk-0");
    assert!(verified_address(&binding, &observed, 1001.).is_ok());
    observed["interfaces"][0]["ip-addresses"][0]["ip-address"] = json!("198.51.100.42");
    assert!(verified_address(&binding, &observed, 1001.).is_err());
}

#[test]
fn observer_accepts_only_allowlisted_guest_ids_without_shell_arguments() {
    let policy = json!({"123":"qemu","45":"lxc"});
    assert_eq!(
        guest_request("discover 123", &policy).unwrap(),
        ("123", "qemu")
    );
    for request in [
        "discover 124",
        "discover 123; id",
        "discover ../123",
        "export",
        "discover 00123",
        "discover 123 extra",
    ] {
        assert!(guest_request(request, &policy).is_err(), "{request}");
    }
}
