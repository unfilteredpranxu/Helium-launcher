//! Deterministic CPU protocol events ONLY; never GPU-completion evidence.
use super::*;
use crate::upload::{Budget, Id, Identity, Phase, Plan, Record, Update};
fn submitted() -> (Record<u64>, Id, Id) {
    let device = Id::new().unwrap();
    let owner = Id::new().unwrap();
    let mut record = Record::prepared(
        Identity {
            generation: 1,
            revision: 1,
        },
        device,
        owner,
    )
    .unwrap();
    record.advance(Phase::Prepared, Phase::Encoded).unwrap();
    record.advance(Phase::Encoded, Phase::Submitted).unwrap();
    (record, device, owner)
}
#[test]
fn submitted_pending_completion_and_repeated_poll_do_not_reissue_receipts() {
    let slot = Slot::new();
    let permit = slot.acquire().unwrap();
    let (mut r, device, owner) = submitted();
    assert!(slot.acquire().is_err());
    for _ in 0..3 {
        assert!(r.observe(None).unwrap().is_none());
        assert_eq!(r.phase(), Phase::Pending);
    }
    let mut receipt = r.observe(Some(true)).unwrap().unwrap();
    assert!(r.observe(Some(true)).is_err());
    assert!(r.observe(None).is_err());
    assert!(
        r.consume(
            &mut receipt,
            Identity {
                generation: 1,
                revision: 2
            },
            device,
            owner
        )
        .is_err()
    );
    let key = receipt.identity();
    assert!(
        r.consume(&mut receipt, key, Id::new().unwrap(), owner)
            .is_err()
    );
    let key = receipt.identity();
    r.consume(&mut receipt, key, device, owner).unwrap();
    assert!(r.consume(&mut receipt, key, device, owner).is_err());
    drop(permit);
    assert!(!slot.busy());
}
#[test]
fn failed_partial_requires_a_full_upload_for_recovery() {
    use crate::upload::tests::{budget, caps, fixture};
    let (mut r, _, _) = submitted();
    r.observe(None).unwrap();
    assert!(r.observe(Some(false)).is_err());
    assert_eq!(r.phase(), Phase::Failed);
    assert!(r.observe(Some(true)).is_err());
    let (s, a) = fixture();
    assert!(
        Plan::prepare(
            &s,
            &a,
            None,
            Update::Layers { first: 0, count: 1 },
            &caps(),
            budget()
        )
        .is_err()
    );
    assert!(Plan::prepare(&s, &a, None, Update::Full, &caps(), budget()).is_ok());
    let (mut retry, _, _) = submitted();
    assert!(retry.observe(Some(true)).unwrap().is_some());
}
#[test]
fn release_invalidation_and_recreation_cannot_become_completion() {
    let (mut r, device, owner) = submitted();
    r.invalidate();
    assert!(r.observe(None).is_err());
    assert!(r.observe(Some(true)).is_err());
    let (mut old, _, _) = submitted();
    let mut receipt = old.observe(Some(true)).unwrap().unwrap();
    let (mut next, _, _) = submitted();
    next.observe(Some(true)).unwrap();
    assert!(
        next.consume(
            &mut receipt,
            Identity {
                generation: 2,
                revision: 1
            },
            device,
            owner
        )
        .is_err()
    );
    assert!(
        next.consume(
            &mut receipt,
            Identity {
                generation: 1,
                revision: 1
            },
            device,
            owner
        )
        .is_err()
    );
}
#[test]
fn material_drop_leaves_source_and_historical_owners_in_terminal_keepalive() {
    let slot = Slot::new();
    let permit = slot.acquire().unwrap();
    permit.account(60, 768, 120);
    let source = Arc::new(vec![1u8; 60]);
    let previous = Arc::new(vec![2u8; 60]);
    let weak = Arc::downgrade(&source);
    let old = Arc::downgrade(&previous);
    let lease = Arc::new(Ownership {
        source,
        previous: Some(previous),
        _permit: permit,
    });
    assert_eq!(lease.source.len(), 60);
    assert_eq!(lease.previous.as_ref().unwrap().len(), 60);
    let completion_keepalive = lease.clone();
    drop(lease); // model dropping Material's pending Rust handles, NOT completion
    assert!(slot.busy());
    assert!(slot.acquire().is_err());
    assert!(weak.upgrade().is_some());
    assert!(old.upgrade().is_some());
    let r = slot.retention();
    assert_eq!((r.texture, r.staging, r.sources), (60, 768, 120));
    // Simulated completion-owner disposal, after a hypothetical terminal event.
    std::thread::spawn(move || drop(completion_keepalive))
        .join()
        .unwrap();
    assert!(!slot.busy());
    assert!(weak.upgrade().is_none());
    assert!(old.upgrade().is_none());
    assert_eq!(slot.retention().sources, 0);
}
#[test]
fn no_receipt_without_submission_and_budget_rejections_do_not_leak_permits() {
    use crate::upload::tests::{budget, caps, fixture};
    let slot = Slot::new();
    {
        let _permit = slot.acquire().unwrap();
        let (s, a) = fixture();
        assert!(
            Plan::prepare(
                &s,
                &a,
                None,
                Update::Full,
                &caps(),
                Budget {
                    staging_bytes: 1,
                    ..budget()
                }
            )
            .is_err()
        );
        let mut r = Record::prepared(
            Identity {
                generation: 1,
                revision: 1,
            },
            Id::new().unwrap(),
            Id::new().unwrap(),
        )
        .unwrap();
        assert!(r.observe(Some(true)).is_err());
        r.advance(Phase::Prepared, Phase::Encoded).unwrap();
        assert!(r.observe(Some(true)).is_err());
    }
    assert!(!slot.busy());
    assert!(slot.acquire().is_ok());
}
