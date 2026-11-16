//! Real Apple Metal tests. Handcrafted G02 fixture, original independent oracle.
//! Sleeps/timeouts are test harness scheduling ONLY, never completion evidence.
use super::*;
use crate::{
    terrain::{Depth, TerrainRenderer, validation},
    upload::{
        self, Update,
        tests::{Authority, Fixture, budget, fixture},
    },
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn retire(r: &TerrainRenderer) {
    let end = Instant::now() + Duration::from_secs(15);
    while r.upload_retention().busy {
        assert!(
            Instant::now() < end,
            "native completion keepalive did not retire"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(r.upload_retention().sources, 0);
}
fn upload(m: &mut upload::Material<Fixture>, r: &TerrainRenderer, a: &Authority) {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        match m.poll_upload(r, a) {
            upload::Poll::Completed(mut ticket) => {
                m.consume_ticket(r, a, &mut ticket).unwrap();
                retire(r);
                return;
            }
            upload::Poll::Pending => {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(1));
            }
            upload::Poll::Failed(e) => panic!("native upload: {e}"),
            _ => panic!("missing upload"),
        }
    }
}
fn terminal(
    f: &mut Frames<Fixture>,
    r: &TerrainRenderer,
    m: &upload::Material<Fixture>,
    a: &Authority,
    success: bool,
) -> Option<Receipt<u64>> {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        match f.poll(r, m, a) {
            Poll::Completed(t) => {
                assert!(success, "invalid frame completed");
                retire(r);
                return Some(t);
            }
            Poll::Failed(e) => {
                assert!(!success, "native draw: {e}");
                retire(r);
                return None;
            }
            Poll::Pending => {
                assert!(Instant::now() < end);
                std::thread::sleep(Duration::from_millis(1));
            }
            Poll::Idle => panic!("missing draw"),
        }
    }
}
#[test]
#[ignore = "requires real Apple Metal; async resident draw and full-image oracle"]
fn native_nonblocking_resident_frames_match_original_oracle_and_retire() {
    let r = TerrainRenderer::new().expect("real native Metal required");
    retire(&r);
    let recreated = TerrainRenderer::new().unwrap();
    let mut chart = validation::fixture();
    let (mut source, mut a) = fixture();
    let mut pixels = Vec::new();
    // Explicit handcrafted G02 pixels in a full two-level G05 chain. Base level
    // is unmodified. Point sampling selects mip zero in this magnified oracle.
    for layer in chart.sprites.chunks_exact(16) {
        pixels.extend_from_slice(layer);
        pixels.extend_from_slice(&layer[..4]);
    }
    source.pixels = Arc::new(pixels);
    let weak = Arc::downgrade(&source.pixels);
    let mut m = r.create_material(&source, &a, budget()).unwrap();
    let mut f = r.create_frames().unwrap();
    assert!(matches!(f.poll(&r, &m, &a), Poll::Idle));
    assert!(
        f.begin(
            &r,
            &m,
            &a,
            &chart.mesh(),
            &chart.lightmap,
            validation::fixture_view(),
            Options::default()
        )
        .is_err()
    );
    m.begin_upload(&r, source.clone(), &a, Update::Full, budget())
        .unwrap();
    assert!(
        f.begin(
            &r,
            &m,
            &a,
            &chart.mesh(),
            &chart.lightmap,
            validation::fixture_view(),
            Options::default()
        )
        .is_err()
    );
    upload(&mut m, &r, &a); // consumes the upload receipt ONCE; frames reuse exact residency
    let mut baseline = None;
    for (pass, depth) in [Depth::Reversed, Depth::Forward, Depth::Reversed]
        .into_iter()
        .enumerate()
    {
        if pass == 1 {
            chart.indices = chart
                .indices
                .chunks_exact(3)
                .rev()
                .flatten()
                .copied()
                .collect();
        }
        let options = Options {
            depth,
            ..Options::default()
        };
        let mut view = validation::fixture_view();
        // Same z'=w-z convention used by the existing G02 forward-depth oracle.
        if depth == Depth::Forward {
            view.clip_from_relative[2][2] = -1.;
            view.clip_from_relative[3][2] = 1.;
        }
        f.begin(&r, &m, &a, &chart.mesh(), &chart.lightmap, view, options)
            .unwrap();
        assert_eq!(f.phase(), Some(upload::Phase::Submitted));
        assert_eq!(
            f.counters().diagnostic_readback_bytes,
            if baseline.is_none() {
                0
            } else {
                256 * 256 * 4 * (f.counters().submitted - 1)
            }
        );
        assert!(r.upload_retention().busy);
        assert!(
            f.begin(&r, &m, &a, &chart.mesh(), &chart.lightmap, view, options)
                .is_err()
        );
        let mut another = r.create_frames::<Fixture>().unwrap();
        assert!(
            another
                .begin(&r, &m, &a, &chart.mesh(), &chart.lightmap, view, options)
                .is_err()
        );
        let mut next = source.clone();
        next.description.identity.revision += 1;
        assert!(
            m.begin_upload(&r, next, &a, Update::Full, budget())
                .is_err()
        );
        assert!(m.readback_for_validation(&r, &a, budget()).is_err());
        assert!(
            r.render_material(&m, &a, &chart.mesh(), &chart.lightmap, view, options)
                .is_err()
        );
        let mut receipt = terminal(&mut f, &r, &m, &a, true).unwrap();
        assert!(matches!(f.poll(&r, &m, &a), Poll::Idle));
        assert!(f.take_completed(&recreated, &m, &a, &mut receipt).is_err());
        {
            let mut output = f.take_completed(&r, &m, &a, &mut receipt).unwrap();
            assert_eq!(output.dimensions(), (256, 256));
            assert!(output.readback_for_validation(1).is_err());
            let image = output.readback_for_validation(32 * 1024 * 1024).unwrap();
            validation::check_image(&image).expect("original independent G02 full-image oracle");
            if let Some(expected) = &baseline {
                assert_eq!(&image.rgba, expected);
            } else {
                baseline = Some(image.rgba);
            }
        }
        assert!(f.take_completed(&r, &m, &a, &mut receipt).is_err());
    }
    assert_eq!(f.counters().reused_targets, 2);
    assert_eq!(f.counters().completed, 3);
    assert_eq!(m.counters().submitted_operations, 1); // no reupload per frame
    eprintln!(
        "G05-C real Metal three full-image oracle draws: {:?}",
        f.counters()
    );
    // A receipt from successful GPU work is still rejected after producer revocation.
    f.begin(
        &r,
        &m,
        &a,
        &chart.mesh(),
        &chart.lightmap,
        validation::fixture_view(),
        Options::default(),
    )
    .unwrap();
    let mut stale = terminal(&mut f, &r, &m, &a, true).unwrap();
    a.live = false;
    assert!(f.take_completed(&r, &m, &a, &mut stale).is_err());
    a.live = true;
    // Invalidation before terminal observation never publishes current output.
    for reason in [0, 1, 2] {
        let mut frames = r.create_frames().unwrap();
        frames
            .begin(
                &r,
                &m,
                &a,
                &chart.mesh(),
                &chart.lightmap,
                validation::fixture_view(),
                Options::default(),
            )
            .unwrap();
        match reason {
            0 => {
                a.identity.revision += 1;
            }
            1 => frames.release(),
            _ => {}
        }
        let observer = if reason == 2 { &recreated } else { &r };
        terminal(&mut frames, observer, &m, &a, false);
        assert_eq!(frames.counters().allocated_payload, 0);
        a.identity = source.description.identity;
    }
    // Releasing the material while a draw is submitted keeps source/texture alive
    // for the exact command, but can never produce an authorized output.
    let mut frames = r.create_frames().unwrap();
    frames
        .begin(
            &r,
            &m,
            &a,
            &chart.mesh(),
            &chart.lightmap,
            validation::fixture_view(),
            Options::default(),
        )
        .unwrap();
    m.release();
    assert!(weak.upgrade().is_some());
    terminal(&mut frames, &r, &m, &a, false);
    assert_eq!(frames.counters().allocated_payload, 0);
    f.release();
    drop(source);
    drop(m);
    assert!(weak.upgrade().is_none());
    // Drop without polling and drop renderer/material too: callback owns source +
    // permit, Metal owns all encoded native objects. No pending-timing assumption.
    let worker = TerrainRenderer::new().unwrap();
    let (s, a) = fixture();
    let weak = Arc::downgrade(&s.pixels);
    let mut mat = worker.create_material(&s, &a, budget()).unwrap();
    mat.begin_upload(&worker, s, &a, Update::Full, budget())
        .unwrap();
    upload(&mut mat, &worker, &a);
    let mut orphan = worker.create_frames().unwrap();
    orphan
        .begin(
            &worker,
            &mat,
            &a,
            &chart.mesh(),
            &chart.lightmap,
            validation::fixture_view(),
            Options::default(),
        )
        .unwrap();
    drop(orphan);
    drop(mat);
    drop(worker);
    retire(&r);
    assert!(weak.upgrade().is_none());
    assert_eq!(r.upload_retention().staging, 0);
    eprintln!(
        "G05-C source/owner revocation, release, recreation, one-use output, orphan cleanup: passed; zero outstanding ownership"
    );
}
