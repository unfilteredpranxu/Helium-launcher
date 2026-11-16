//! Handcrafted fixtures through actual source_chains, collect and fallback mixer.
use super::*;
use crate::gfx::{
    PreparedTextures,
    material_snapshot::{Phase, State},
};

fn fixture(budget: usize) -> (SpriteAnimator, State) {
    let size = 4;
    let split = 256;
    let layers = 513;
    let metadata = [
        r#"{"frametime":2,"interpolate":true}"#,
        r#"{"frametime":2}"#,
    ];
    let mut settings = vec![Settings::default(); layers];
    let mut data = vec![Vec::new(), Vec::new()];
    let mut sprites = Vec::new();
    for (index, layer) in [257, 258, 512].into_iter().enumerate() {
        settings[layer].native_size = 2;
        let frames = (0..3)
            .map(|frame| {
                (0..size * size)
                    .flat_map(|p| {
                        [
                            ((p + frame * 31) % 256) as u8,
                            (index * 70) as u8,
                            255,
                            if p % 3 == 0 { 0 } else { 128 },
                        ]
                    })
                    .collect()
            })
            .collect();
        let animation = crate::sprites::test_frames(layer as u32, frames, metadata[index % 2]);
        let pixels: Vec<_> = (0..animation.frame_count())
            .map(|f| animation.pixels(f))
            .collect();
        let (chains, edge) = source_chains(&pixels, size, &settings, layer as u32);
        let group = if layer < 512 { 0 } else { 1 };
        let offsets = chains
            .iter()
            .map(|chain| {
                let offset = (data[group].len() / 4) as u32;
                data[group].extend(chain);
                offset
            })
            .collect();
        sprites.push(Sprite {
            animation,
            shown: None,
            layer: (layer - (group + 1) * 256) as u32,
            frames: offsets,
            edge,
            group,
        });
    }
    let groups = (0..2)
        .map(|i| Group {
            offset: 0,
            levels: vec![],
            jobs: vec![],
            array: i + 1,
            buffer: i,
        })
        .collect();
    let pixels = vec![73; size as usize * size as usize * 4 * layers];
    let state = PreparedTextures::with_split(
        &pixels,
        size,
        layers as u32,
        &settings,
        (&[255; 16], 2),
        split,
    )
    .retain_material_snapshot(budget)
    .capture_materials(size, layers as u32, false);
    (
        SpriteAnimator {
            sprites,
            groups,
            writer: Writer::Cpu {
                frames: data,
                textures: vec![],
            },
            size,
            changed: vec![],
            is_animated: vec![],
        },
        state,
    )
}
#[test]
fn collected_jobs_refresh_exact_fallback_bytes_without_static_rebuilds() {
    let (mut animator, mut state) = fixture(1 << 20);
    let initial = state.snapshot().unwrap().clone();
    let initial_ticket = state.ticket().unwrap();
    let static_pointer = initial.banks()[0].pixels().as_ptr();
    let mut expected = initial
        .banks()
        .iter()
        .map(|b| b.pixels().to_vec())
        .collect::<Vec<_>>();
    for (tick, active) in [
        (0, None),
        (0, None),
        (1, None),
        (2, Some(vec![0; 9])),
        (2, None),
        (3, None),
        (6, None),
    ] {
        let count = animator.collect(tick, active.as_deref());
        if count == 0 {
            continue;
        }
        let frames = match &animator.writer {
            Writer::Cpu { frames, .. } => frames,
            _ => unreachable!(),
        };
        // Independent destination assembly using the actual production fallback.
        for group in &animator.groups {
            for bytes in group.jobs.as_chunks::<JOB_BYTES>().0 {
                let words = bytes.as_chunks::<4>().0;
                let job = std::array::from_fn(|i| u32::from_le_bytes(words[i]));
                let mut offset = job[0] as usize * 84;
                for level in 0..3 {
                    let mixed = mix_level(&frames[group.buffer], job, 4 >> level);
                    expected[group.array][offset..offset + mixed.len()].copy_from_slice(&mixed);
                    offset += mixed.len();
                }
            }
        }
        animator.refresh_observer(&mut state);
        let snapshot = state.current(state.snapshot().unwrap()).unwrap();
        for (bank, expected) in snapshot.banks().iter().zip(&expected) {
            assert_eq!(bank.pixels(), expected);
        }
        assert_eq!(snapshot.banks()[0].pixels().as_ptr(), static_pointer);
        assert!(state.current(&initial).is_err());
        assert!(state.check_ticket(initial_ticket).is_err());
        assert_eq!(state.phase(), Phase::Encoded);
        assert!(state.gpu_current().is_err());
        for group in &mut animator.groups {
            group.jobs.clear();
        }
    }
    assert_eq!(initial.banks()[1].pixels()[0], 73); // immutable old revision
    assert!(state.counters().updated_layers > 3);
    assert_eq!(state.counters().copied_bytes, 513 * 84 + 257 * 84); // init + two changed banks once
    assert!(state.counters().reused_bytes > 0);
    assert_eq!(state.counters().failed_refreshes, 0);
    state.queued();
    assert_eq!(state.phase(), Phase::QueuedForSubmission);
    state.submitted();
    assert_eq!(state.phase(), Phase::SubmittedUnconfirmed);
    assert!(state.gpu_current().is_err());
}
#[test]
fn millisecond_boundaries_match_independent_integer_reference() {
    let frames = [0, 1, 127, 0, 255, 0, 128, 255];
    for millis in [0, 1, 499, 500, 501, 999, 1000] {
        let mut out = [0; 4];
        mix_level_into(&frames, [0, 0, 1, millis, 1], 1, &mut out).unwrap();
        let expected = std::array::from_fn::<_, 4, _>(|c| {
            ((u32::from(frames[c]) * (1000 - millis) + u32::from(frames[c + 4]) * millis + 500)
                / 1000) as u8
        });
        assert_eq!(out, expected);
    }
    assert!(validate_mix(&frames, [0, u32::MAX, 0, 0, 1], 1).is_err());
    assert!(validate_mix(&frames, [0, 0, 1, 1001, 1], 1).is_err());
}
#[test]
fn pinned_snapshot_budget_and_workload_rejection_are_fail_closed() {
    let (mut animator, mut state) = fixture(1 << 20);
    let before = state.counters().retained_capacity;
    let retained = state.snapshot().unwrap().clone();
    state.set_refresh_limit(1);
    animator.collect(0, None);
    animator.refresh_observer(&mut state);
    assert!(state.snapshot().is_err());
    assert_eq!(state.counters().failed_budget_checks, 1);
    assert_eq!(state.counters().invalidations, 1);
    assert_eq!(state.counters().retained_capacity, before);
    drop(retained);
    assert_eq!(state.counters().retained_capacity, 0);

    let (mut animator, mut state) = fixture(before + 1); // fits initial, not pinned COW bank
    let retained = state.snapshot().unwrap().clone();
    animator.collect(0, None);
    animator.refresh_observer(&mut state);
    assert!(state.snapshot().is_err());
    assert_eq!(state.counters().failed_budget_checks, 1);
    assert!(state.counters().peak_capacity <= before + 1);
    assert_eq!(retained.banks()[1].pixels()[0], 73);
}
#[test]
fn no_history_allows_zero_copy_refresh_and_frame_retention_is_bounded() {
    let (mut animator, mut state) = fixture(1 << 20);
    let pointer = state.snapshot().unwrap().banks()[1].pixels().as_ptr();
    let initial_copies = state.counters().copied_bytes;
    animator.collect(0, None);
    animator.refresh_observer(&mut state);
    assert_eq!(
        state.snapshot().unwrap().banks()[1].pixels().as_ptr(),
        pointer
    );
    assert_eq!(state.counters().copied_bytes, initial_copies);
    assert_eq!(state.counters().temporary_peak_capacity, 0);
    state.retain_frames(vec![vec![0; 2 << 20]]);
    assert!(state.snapshot().is_err());
    assert_eq!(state.counters().retained_capacity, 0);
}
