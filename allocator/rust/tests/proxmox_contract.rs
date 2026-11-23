use lab_allocator::proxmox::{GuestPin, TaskKind, task_matches};
use serde_json::json;

#[test]
fn only_the_exact_node_guest_task_type_and_token_can_claim_an_operation() {
    let task = "UPID:lab:00100:00200:11223344:qmstart:901:pipeline@pve!lease:";
    assert!(task_matches(
        task,
        "lab",
        "901",
        "pipeline@pve!lease",
        TaskKind::Start
    ));
    assert!(!task_matches(
        task,
        "other",
        "901",
        "pipeline@pve!lease",
        TaskKind::Start
    ));
    assert!(!task_matches(
        task,
        "lab",
        "900",
        "pipeline@pve!lease",
        TaskKind::Start
    ));
    assert!(!task_matches(
        task,
        "lab",
        "901",
        "operator@pam!lease",
        TaskKind::Start
    ));
    assert!(!task_matches(
        task,
        "lab",
        "901",
        "pipeline@pve!lease",
        TaskKind::Destroy
    ));
    assert!(!task_matches(
        "bad",
        "lab",
        "901",
        "pipeline@pve!lease",
        TaskKind::Start
    ));
}

#[test]
fn changed_or_locked_guests_refuse_power_or_storage_operations() {
    let pinned = GuestPin::new(
        "lease-slot",
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "store:vm-901-disk-0",
    );
    let mut reply = json!({"name":"lease-slot","smbios1":"manufacturer=Example,uuid=aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "scsi0":"store:vm-901-disk-0,size=8G","vmgenid":"bbbbbbbb-cccc-4ddd-8eee-ffffffffffff"});
    assert_eq!(
        pinned.generation(&reply).unwrap(),
        "bbbbbbbb-cccc-4ddd-8eee-ffffffffffff"
    );
    reply["lock"] = json!("backup");
    assert_eq!(pinned.generation(&reply).unwrap_err().code(), "slot_locked");
    reply["lock"] = json!(null);
    reply["scsi0"] = json!("store:vm-901-disk-99,size=8G");
    assert_eq!(
        pinned.generation(&reply).unwrap_err().code(),
        "slot_identity_changed"
    );
    reply["scsi0"] = json!("store:vm-901-disk-0,size=8G");
    reply["vmgenid"] = json!("00000000-0000-0000-0000-000000000000");
    assert_eq!(
        pinned.generation(&reply).unwrap_err().code(),
        "slot_identity_changed"
    );
}
