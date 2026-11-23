use lab_dashboard::network::{compare, guest_identity};
use serde_json::json;

#[test]
fn static_dhcp_offline_and_unknown_observations_have_distinct_dispositions() {
    let rows = vec![
        json!({"address":"192.0.2.11","mac":"aa:bb:cc:dd:ee:ff","assignment":"static"}),
        json!({"address":"192.0.2.11","mac":"aa:bb:cc:dd:ee:00","assignment":"static"}),
        json!({"address":"192.0.2.12","assignment":"static"}),
        json!({"address":"192.0.2.13","assignment":"dhcp"}),
        json!({"address":"192.0.2.14","assignment":"unknown"}),
        json!({"scope":"offline","assignment":"dhcp","mac":"aa:bb:cc:dd:ee:ff","profile_verified":true}),
        json!({"scope":"offline","address":"10.77.0.1","assignment":"static"}),
        json!({"address":"10.77.0.1","assignment":"static"}),
        json!({"machine":"unreachable","status":"unverified","reason":"offline"}),
    ];
    let authority = json!([{"ip":"192.0.2.11","mac":"AA:BB:CC:DD:EE:FF"}]);
    let result = compare(rows, &authority, &["192.0.2.0/24"]).unwrap();
    assert_eq!(
        result
            .iter()
            .map(|row| row["status"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "pass",
            "violation",
            "unverified",
            "pass",
            "unverified",
            "offline-config-verified",
            "not-applicable",
            "unverified"
        ]
    );
}

#[test]
fn exact_guest_binding_takes_precedence_over_old_audit_fields() {
    let spec = json!({"host":"host","resources":{"network_authority_audit":{"guest_id":140,"host":"host"}}});
    assert_eq!(
        guest_identity(&spec),
        (Some("machine-host".into()), Some("140".into()))
    );
    let spec = json!({"host":"host","resources":{
        "network_authority_audit":{"guest_id":140,"host":"host"},
        "runtime_binding":{"kind":"guest","machine_id":"machine-other","guest_id":"b"}}});
    assert_eq!(
        guest_identity(&spec),
        (Some("machine-other".into()), Some("b".into()))
    );
}

#[test]
fn saved_dhcp_and_pending_alias_edits_remain_visible_together() {
    use lab_dashboard::network::collector::{linux_links, saved_profiles};
    use std::fs;
    let dir = tempfile::tempdir().unwrap();
    assert!(saved_profiles(dir.path(), "aa:bb:cc:dd:ee:ff", false).is_err());
    let path = dir.path().join("etc/network");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("interfaces"), "iface eth0 inet dhcp\n").unwrap();
    fs::write(
        path.join("interfaces.new"),
        "iface eth0 inet static\n address 192.0.2.66/24\n",
    )
    .unwrap();
    let rows = saved_profiles(dir.path(), "aa:bb:cc:dd:ee:ff", false).unwrap();
    let checked = compare(rows, &json!([]), &["192.0.2.0/24"]).unwrap();
    assert_eq!(checked[0]["status"], "offline-config-verified");
    assert_eq!(checked[1]["status"], "violation");
    let links=linux_links(&json!([{"ifname":"eth0","addr_info":[{"family":"inet","local":"192.0.2.10","dynamic":true}]}]),
        &json!([{"ifname":"eth0","address":"aa:bb:cc:dd:ee:ff"}])).unwrap();
    assert_eq!(links[0]["assignment"], "dhcp");
    assert_eq!(links[0]["mac"], "aa:bb:cc:dd:ee:ff");
}
