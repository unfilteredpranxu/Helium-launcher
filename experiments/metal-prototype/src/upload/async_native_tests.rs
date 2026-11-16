//! Real Metal tests. Handcrafted source pixels; sleeps/deadlines are TEST-ONLY
//! scheduling, never completion evidence. Every successful receipt comes from
//! poll_upload observing its exact command's real Completed/no-error status.
use super::{
    tests::{Authority, Fixture, budget, fixture},
    *,
};
use crate::terrain::{Options, TerrainRenderer, validation};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn retired(renderer: &TerrainRenderer) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while renderer.upload_retention().busy {
        assert!(
            Instant::now() < deadline,
            "completion keepalive/slot did not retire"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn completed(
    material: &mut Material<Fixture>,
    renderer: &TerrainRenderer,
    a: &Authority,
) -> Ticket<u64> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match material.poll_upload(renderer, a) {
            Poll::Completed(t) => return t,
            Poll::Pending => {
                assert!(Instant::now() < deadline, "GPU operation not terminal");
                std::thread::sleep(Duration::from_millis(1));
            }
            Poll::Failed(e) => panic!("native upload failed: {e}"),
            Poll::Idle => panic!("missing native pending operation"),
        }
    }
}
fn revoked(material: &mut Material<Fixture>, renderer: &TerrainRenderer, a: &Authority) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match material.poll_upload(renderer, a) {
            Poll::Failed(_) => return,
            Poll::Pending => {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
            _ => panic!("revoked command must not publish completion"),
        }
    }
}
#[test]
#[ignore = "requires real Metal; nonblocking submission, polling and terminal cleanup"]
fn native_one_slot_nonblocking_upload_and_cleanup() {
    let renderer = TerrainRenderer::new().expect("actual native Metal device required");
    retired(&renderer);
    let (source, mut a) = fixture();
    let expected = source.pixels().to_vec();
    let weak = Arc::downgrade(&source.pixels);
    let mut material = renderer.create_material(&source, &a, budget()).unwrap();
    assert!(matches!(material.poll_upload(&renderer, &a), Poll::Idle));
    assert!(
        material
            .begin_upload(
                &renderer,
                source.clone(),
                &a,
                Update::Full,
                Budget {
                    staging_bytes: 1,
                    ..budget()
                }
            )
            .is_err()
    );
    assert_eq!(material.counters().budget_rejections, 1);
    assert_eq!(material.counters().submitted_operations, 0);
    assert!(!renderer.upload_retention().busy);
    material
        .begin_upload(&renderer, source, &a, Update::Full, budget())
        .unwrap();
    // No first-poll timing assumption: the physical command might already be done,
    // but nothing publishes completion or frees this slot until observed/retired.
    assert_eq!(material.phase(), Phase::Submitted);
    assert_eq!(material.counters().pending, 1);
    assert_eq!(material.counters().completed_uploads, 0);
    assert!(renderer.upload_retention().busy);
    assert!(weak.upgrade().is_some());
    let (other, _) = fixture();
    assert!(
        material
            .begin_upload(&renderer, other.clone(), &a, Update::Full, budget())
            .is_err()
    );
    assert!(
        material
            .upload_blocking(&renderer, other.clone(), &a, Update::Full, budget())
            .is_err()
    );
    assert!(renderer.create_material(&other, &a, budget()).is_err());
    assert!(
        material
            .readback_for_validation(&renderer, &a, budget())
            .is_err()
    );
    let chart = validation::fixture();
    assert!(
        renderer
            .render_material(
                &material,
                &a,
                &chart.mesh(),
                &chart.lightmap,
                validation::fixture_view(),
                Options::default()
            )
            .is_err()
    );
    let mut ticket = completed(&mut material, &renderer, &a);
    assert_eq!(ticket.phase(), Phase::Completed);
    assert!(matches!(material.poll_upload(&renderer, &a), Poll::Idle));
    retired(&renderer);
    material.consume_ticket(&renderer, &a, &mut ticket).unwrap();
    assert!(material.consume_ticket(&renderer, &a, &mut ticket).is_err());
    assert_eq!(
        material
            .readback_for_validation(&renderer, &a, budget())
            .unwrap(),
        expected
    );
    let (mut next, _) = fixture();
    next.description.identity.revision = 2;
    Arc::make_mut(&mut next.pixels)[59] = 17;
    a.identity = next.description.identity;
    let expected = next.pixels().to_vec();
    material
        .begin_upload(
            &renderer,
            next,
            &a,
            Update::Layers { first: 2, count: 1 },
            budget(),
        )
        .unwrap();
    assert!(weak.upgrade().is_some()); // previous source owned through in-flight work
    let mut next_ticket = completed(&mut material, &renderer, &a);
    retired(&renderer);
    assert!(weak.upgrade().is_none());
    material
        .consume_ticket(&renderer, &a, &mut next_ticket)
        .unwrap();
    assert_eq!(
        material
            .readback_for_validation(&renderer, &a, budget())
            .unwrap(),
        expected
    );
    let counters = material.counters();
    assert_eq!(
        (
            counters.submitted_operations,
            counters.completed_uploads,
            counters.reused_resources,
            counters.upload_bytes
        ),
        (2, 2, 1, 80)
    );
    assert_eq!(counters.pending, 0);
    assert_eq!(counters.staging_capacity, 0);
    assert!(counters.pending_polls >= 2);
    eprintln!("G05-B real Metal upload counters: {counters:?}");

    // Actual submitted partial update, invalidated producer before observation.
    let (mut stale, _) = fixture();
    stale.description.identity.revision = 3;
    Arc::make_mut(&mut stale.pixels)[59] = 42;
    a.identity = stale.description.identity;
    material
        .begin_upload(
            &renderer,
            stale,
            &a,
            Update::Layers { first: 2, count: 1 },
            budget(),
        )
        .unwrap();
    a.identity.revision = 4;
    revoked(&mut material, &renderer, &a);
    retired(&renderer);
    assert_eq!(material.phase(), Phase::Invalidated);
    let (mut recovery, _) = fixture();
    recovery.description.identity = a.identity;
    assert!(
        material
            .begin_upload(
                &renderer,
                recovery.clone(),
                &a,
                Update::Layers { first: 2, count: 1 },
                budget()
            )
            .is_err()
    );
    let recovery_expected = recovery.pixels().to_vec();
    material
        .begin_upload(&renderer, recovery, &a, Update::Full, budget())
        .unwrap();
    completed(&mut material, &renderer, &a);
    retired(&renderer);

    assert_eq!(
        material
            .readback_for_validation(&renderer, &a, budget())
            .unwrap(),
        recovery_expected
    );
    assert_eq!(material.counters().submitted_operations, 4);
    assert_eq!(material.counters().completed_uploads, 3);
    assert_eq!(material.counters().reused_resources, 3); // includes the revoked write and full repair
    assert_eq!(material.counters().failed_operations, 1);

    // Release while submitted: never frees in-flight source/texture or resurrects.
    let (mut release_source, _) = fixture();
    release_source.description.identity.revision = 5;
    a.identity = release_source.description.identity;
    let release_weak = Arc::downgrade(&release_source.pixels);
    material
        .begin_upload(&renderer, release_source, &a, Update::Full, budget())
        .unwrap();
    material.release();
    assert_eq!(material.phase(), Phase::Invalidated);
    assert!(release_weak.upgrade().is_some());
    assert_eq!(material.counters().pending, 1);
    revoked(&mut material, &renderer, &a);
    retired(&renderer);
    assert!(release_weak.upgrade().is_none());
    let (mut impossible, _) = fixture();
    impossible.description.identity = a.identity;
    assert!(
        material
            .begin_upload(&renderer, impossible, &a, Update::Full, budget())
            .is_err()
    );

    // Drop without polling: callback owns CPU data and permit, Metal retains GPU
    // resources. No destructor waits; retirement can occur before/after this drop.
    let (source, a) = fixture();
    let weak = Arc::downgrade(&source.pixels);
    let mut orphan = renderer.create_material(&source, &a, budget()).unwrap();
    orphan
        .begin_upload(&renderer, source, &a, Update::Full, budget())
        .unwrap();
    drop(orphan);
    retired(&renderer);
    assert!(weak.upgrade().is_none());
    assert_eq!(renderer.upload_retention().sources, 0);

    // A new renderer cannot accept another owner's pending completion.
    let recreated = TerrainRenderer::new().unwrap();
    let (source, a) = fixture();
    let mut old = renderer.create_material(&source, &a, budget()).unwrap();
    old.begin_upload(&renderer, source, &a, Update::Full, budget())
        .unwrap();
    revoked(&mut old, &recreated, &a);
    retired(&renderer);
    assert_eq!(old.phase(), Phase::Invalidated);
    eprintln!(
        "G05-B pending conflicts, partial recovery, source revocation, release, drop and recreation passed"
    );
}
