//! Handcrafted inputs and simulated command events. No GPU runs in these tests.
use super::*;
use crate::upload::tests::{budget, caps, fixture};
use crate::{
    terrain::{self, validation},
    upload::{Identity, Plan, Update},
};
fn resident() -> (Record<u64>, Stamp<u64>, Id, Id) {
    let device = Id::new().unwrap();
    let owner = Id::new().unwrap();
    let mut upload = Record::prepared(
        Identity {
            generation: 7,
            revision: 3,
        },
        device,
        owner,
    )
    .unwrap();
    upload.advance(Phase::Prepared, Phase::Encoded).unwrap();
    upload.advance(Phase::Encoded, Phase::Submitted).unwrap();
    upload.finish(true).unwrap();
    let stamp = upload.residency().unwrap();
    (upload, stamp, device, owner)
}
#[test]
fn simulated_frame_completion_is_exact_and_one_use() {
    let (_, stamp, device, owner) = resident();
    let mut frame = Protocol::prepare(stamp, device, owner).unwrap();
    assert!(matches!(frame.observe(Some(true)), Poll::Idle));
    assert!(frame.submitted().is_err());
    frame.encoded().unwrap();
    frame.submitted().unwrap();
    assert!(frame.submitted().is_err());
    for _ in 0..3 {
        assert!(matches!(frame.observe(None), Poll::Pending));
    }
    let Poll::Completed(mut receipt) = frame.observe(Some(true)) else {
        panic!()
    };
    assert!(matches!(frame.observe(Some(true)), Poll::Idle));
    assert!(
        frame
            .consume(&mut receipt, stamp, Id::new().unwrap(), owner)
            .is_err()
    );
    assert!(
        frame
            .consume(&mut receipt, stamp, device, Id::new().unwrap())
            .is_err()
    );
    frame.consume(&mut receipt, stamp, device, owner).unwrap();
    assert!(frame.consume(&mut receipt, stamp, device, owner).is_err());
    // A newer upload, even to the same device/material/identity, is another operation.
    let mut upload = Record::prepared(stamp.identity, device, owner).unwrap();
    assert!(upload.residency().is_err());
    upload.advance(Phase::Prepared, Phase::Encoded).unwrap();
    upload.advance(Phase::Encoded, Phase::Submitted).unwrap();
    upload.finish(true).unwrap();
    assert!(frame.check(upload.residency().unwrap(), device).is_err());
    assert!(Protocol::prepare(stamp, Id::new().unwrap(), owner).is_err());
}
#[test]
fn simulated_revocation_waits_for_terminal_event_and_failure_cannot_complete() {
    for reason in ["release", "drop", "producer revision", "device recreation"] {
        let (_, stamp, device, owner) = resident();
        let mut p = Protocol::prepare(stamp, device, owner).unwrap();
        p.encoded().unwrap();
        p.submitted().unwrap();
        p.invalidate(reason.into());
        assert_eq!(p.phase(), Phase::Invalidated);
        for _ in 0..3 {
            assert!(matches!(p.observe(None), Poll::Pending));
        }
        assert!(matches!(p.observe(Some(true)), Poll::Failed(_)));
        assert!(p.check(stamp, device).is_err());
    }
    let (_, stamp, device, owner) = resident();
    let mut p = Protocol::prepare(stamp, device, owner).unwrap();
    p.encoded().unwrap();
    p.submitted().unwrap();
    assert!(matches!(p.observe(Some(false)), Poll::Failed(_)));
    assert!(matches!(p.observe(Some(true)), Poll::Idle));
    let mut changed = stamp;
    changed.identity.revision += 1;
    assert!(p.check(changed, device).is_err());
}
#[test]
fn frame_layout_has_no_readback_and_preserves_g02_validation() {
    let (s, a) = fixture();
    let chart = validation::fixture();
    let upload = Plan::prepare(&s, &a, None, Update::Full, &caps(), budget()).unwrap();
    let d = upload.description();
    let o = terrain::Options::default();
    let v = validation::fixture_view();
    let layout = validate(&chart.mesh(), d, 60, 80, &chart.lightmap, v, o, &caps()).unwrap();
    assert_eq!(layout.readback_bytes, 0);
    assert_eq!(layout.atlas_staging, 0);
    assert_eq!(layout.staging_bytes, 4096);
    let expected = chart.mesh().vertices.len()
        + chart.indices.len() * 4
        + 80
        + 4096 * 2
        + 1024
        + 60
        + 80
        + 256 * 256 * 8;
    assert_eq!(layout.payload_bytes, expected);
    let old = terrain::resource_layout(
        &chart.mesh(),
        (2, 2, 3),
        60,
        false,
        &chart.lightmap,
        v,
        o,
        &caps(),
    )
    .unwrap();
    assert_eq!(old.payload_bytes + 80 - layout.payload_bytes, 256 * 256 * 8);
    let mut small = o;
    small.budget.payload_bytes = expected - 1;
    assert!(validate(&chart.mesh(), d, 60, 80, &chart.lightmap, v, small, &caps()).is_err());
    assert!(
        validate(
            &chart.mesh(),
            d,
            60,
            usize::MAX,
            &chart.lightmap,
            v,
            o,
            &caps()
        )
        .is_err()
    );
    let mut enormous = o;
    enormous.width = usize::MAX;
    assert!(
        validate(
            &chart.mesh(),
            d,
            60,
            80,
            &chart.lightmap,
            v,
            enormous,
            &caps()
        )
        .is_err()
    );
    assert!(terrain::product(&[usize::MAX, 4]).is_err());
    let mut bad = d;
    bad.bank = 1;
    assert!(validate(&chart.mesh(), bad, 60, 80, &chart.lightmap, v, o, &caps()).is_err());
    bad = d;
    bad.used = 0;
    assert!(validate(&chart.mesh(), bad, 60, 80, &chart.lightmap, v, o, &caps()).is_err());
    bad = d;
    bad.levels = 1;
    assert!(validate(&chart.mesh(), bad, 60, 80, &chart.lightmap, v, o, &caps()).is_err());
    bad = d;
    bad.format = crate::material::Format::Other;
    assert!(validate(&chart.mesh(), bad, 60, 80, &chart.lightmap, v, o, &caps()).is_err());
    assert!(validate(&chart.mesh(), d, 59, 80, &chart.lightmap, v, o, &caps()).is_err());
}
#[test]
fn invalid_mesh_lightmap_camera_and_device_fail_before_submission() {
    let (s, _) = fixture();
    use crate::upload::Source;
    let d = s.description();
    let mut chart = validation::fixture();
    let o = terrain::Options::default();
    let v = validation::fixture_view();
    chart.indices[0] = u32::MAX;
    assert!(validate(&chart.mesh(), d, 60, 80, &chart.lightmap, v, o, &caps()).is_err());
    let chart = validation::fixture();
    let mut mesh = chart.mesh();
    mesh.vertices = &mesh.vertices[..23];
    assert!(validate(&mesh, d, 60, 80, &chart.lightmap, v, o, &caps()).is_err());
    assert!(
        validate(
            &chart.mesh(),
            d,
            60,
            80,
            &chart.lightmap[..1023],
            v,
            o,
            &caps()
        )
        .is_err()
    );
    let mut lm = chart.lightmap.clone();
    lm[3] = 0;
    assert!(validate(&chart.mesh(), d, 60, 80, &lm, v, o, &caps()).is_err());
    let mut bad = v;
    bad.origin[0] = f32::NAN;
    assert!(validate(&chart.mesh(), d, 60, 80, &chart.lightmap, bad, o, &caps()).is_err());
    let mut c = caps();
    c.sample_counts = 0;
    assert!(validate(&chart.mesh(), d, 60, 80, &chart.lightmap, v, o, &c).is_err());
}
#[test]
fn simulated_frame_slot_keeps_owners_after_drop_and_rejects_conflicting_uploads() {
    use crate::upload::{Ownership, Slot};
    use std::sync::Arc;
    let slot = Slot::new();
    let permit = slot.acquire().unwrap();
    permit.account_frame(1024);
    assert_eq!(slot.retention().frame_payload, 1024);
    let source = Arc::new(vec![42u8; 60]);
    let weak = Arc::downgrade(&source);
    let lease = Arc::new(Ownership {
        source,
        previous: None,
        _permit: permit,
    });
    let command_keepalive = lease.clone();
    let (_, stamp, device, owner) = resident();
    let mut protocol = Protocol::prepare(stamp, device, owner).unwrap();
    protocol.encoded().unwrap();
    protocol.submitted().unwrap();
    assert!(slot.acquire().is_err());
    assert!(matches!(protocol.observe(None), Poll::Pending));
    protocol.invalidate("frame owner dropped".into());
    drop(lease);
    assert!(slot.busy());
    assert!(weak.upgrade().is_some());
    assert!(matches!(protocol.observe(None), Poll::Pending));
    assert!(matches!(protocol.observe(Some(true)), Poll::Failed(_)));
    // Simulates disposal of the completion block AFTER a terminal GPU event.
    drop(command_keepalive);
    assert_eq!(slot.retention().frame_payload, 0);
    assert!(!slot.busy());
    assert!(weak.upgrade().is_none());
    assert!(slot.acquire().is_ok());
}
#[test]
fn simulated_new_frame_cannot_consume_old_frame_or_new_material_revision() {
    let (_, stamp, device, owner) = resident();
    let mut old = Protocol::prepare(stamp, device, owner).unwrap();
    old.encoded().unwrap();
    old.submitted().unwrap();
    let Poll::Completed(mut receipt) = old.observe(Some(true)) else {
        panic!()
    };
    let mut next = Protocol::prepare(stamp, device, owner).unwrap();
    next.encoded().unwrap();
    next.submitted().unwrap();
    assert!(next.consume(&mut receipt, stamp, device, owner).is_err());
    let Poll::Completed(mut current) = next.observe(Some(true)) else {
        panic!()
    };
    assert!(next.consume(&mut receipt, stamp, device, owner).is_err());
    let mut changed = stamp;
    changed.identity.generation += 1;
    assert!(next.consume(&mut current, changed, device, owner).is_err());
    changed = stamp;
    changed.identity.revision += 1;
    assert!(next.consume(&mut current, changed, device, owner).is_err());
    next.consume(&mut current, stamp, device, owner).unwrap();
}
