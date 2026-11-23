use lab_allocator::{LeasePhase, Lifecycle, ReclaimAction};
use serde_json::json;

#[test]
fn automatic_cleanup_waits_for_grace_holds_and_verified_export() {
    let mut lease = Lifecycle {
        phase: LeasePhase::Ready,
        reclaim_on_expiry: true,
        deadline: 100.0,
        owner_hold: None,
        reviewer_hold: None,
        checkpoint: None,
    };
    assert_eq!(lease.advance(110.0, 20.0), ReclaimAction::Wait);
    assert_eq!(lease.advance(120.0, 20.0), ReclaimAction::StopForExport);
    lease.reviewer_hold = Some("running");
    assert_eq!(lease.advance(121.0, 20.0), ReclaimAction::Wait);
    lease.reviewer_hold = Some("disk");
    assert_eq!(lease.advance(121.0, 20.0), ReclaimAction::StopForExport);
    lease.phase = LeasePhase::AwaitingExport;
    let proof = json!({"storage_managed":true,"restore_verified":true});
    lease.checkpoint = Some(&proof);
    assert_eq!(lease.advance(121.0, 20.0), ReclaimAction::Wait);
    lease.reviewer_hold = None;
    assert_eq!(lease.advance(121.0, 20.0), ReclaimAction::CleanupVerified);
    let receipt = json!({"storage_managed":true,"restore_verified":false});
    lease.checkpoint = Some(&receipt);
    assert_eq!(lease.advance(121.0, 20.0), ReclaimAction::ReverifyExport);
    lease.reclaim_on_expiry = false;
    assert_eq!(lease.advance(121.0, 20.0), ReclaimAction::Wait);
}

#[test]
fn release_requires_explicit_data_disposition_or_verified_export_and_renewal_never_shortens() {
    let mut lease = Lifecycle {
        phase: LeasePhase::Ready,
        reclaim_on_expiry: true,
        deadline: 100.0,
        owner_hold: None,
        reviewer_hold: None,
        checkpoint: None,
    };
    assert_eq!(lease.renew(10, 15.0).unwrap(), 100.0);
    assert_eq!(lease.renew(180, 15.0).unwrap(), 195.0);
    assert_eq!(
        lease
            .release(&json!({"preserve_checkpoint":false}))
            .unwrap_err()
            .code(),
        "final_export_required"
    );
    assert_eq!(
        lease.release(&json!({})).unwrap(),
        ReclaimAction::StopForExport
    );
    lease.owner_hold = Some("disk");
    assert_eq!(lease.release(&json!({})).unwrap_err().code(), "lease_held");
    lease.reclaim_on_expiry = false;
    assert_eq!(
        lease.release(&json!({})).unwrap_err().code(),
        "release_requires_disposition"
    );
    assert_eq!(
        lease
            .release(&json!({"discard_disposable_data":true}))
            .unwrap(),
        ReclaimAction::DiscardLegacyWorkspace
    );
    lease.phase = LeasePhase::Deleting;
    assert_eq!(lease.release(&json!({})).unwrap(), ReclaimAction::Inspect);
    assert_eq!(
        lease.renew(180, 15.0).unwrap_err().code(),
        "lease_not_renewable"
    );
}
