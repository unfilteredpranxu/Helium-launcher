use super::*;
// Handcrafted source pixels passed through actual production mip generation.
fn prepared(layers: u32, split: u32) -> PreparedTextures {
    let pixels: Vec<_> = (0..layers)
        .flat_map(|i| [i as u8, 27, 93, 255].repeat(4))
        .collect();
    PreparedTextures::with_split(&pixels, 2, layers, &[], (&[255; 16], 2), split)
        .retain_material_snapshot(1 << 20)
}
#[test]
fn post_mips_resplit_partial_banks_and_padding_are_exact() {
    let original = prepared(5, 4);
    let expected: Vec<_> = original
        .sprites
        .iter()
        .flat_map(|(v, _)| v.iter().copied())
        .collect();
    let split = super::super::layer_split(2, false, Some(4), 5).unwrap();
    let prepared = original.resplit(5, split);
    let state = prepared.capture_materials(2, 5, true);
    let snapshot = state.snapshot().unwrap();
    assert_eq!(snapshot.split(), 2);
    assert_eq!(snapshot.banks().len(), 3);
    assert_eq!(
        snapshot
            .banks()
            .iter()
            .map(|b| b.used())
            .collect::<Vec<_>>(),
        [2, 2, 1]
    );
    for (index, bank) in snapshot.banks().iter().enumerate() {
        assert_eq!(bank.levels(), 2);
        assert_eq!(bank.allocated(), 2);
        let used = bank.used() as usize * 20;
        assert_eq!(
            &bank.pixels()[..used],
            &expected[index * 40..index * 40 + used]
        );
        assert!(bank.pixels()[used..].iter().all(|&b| b == 0));
    }
    // The generated 1x1 mip follows each 2x2 layer, not all base images.
    assert_eq!(&snapshot.banks()[0].pixels()[16..20], &[0, 27, 93, 255]);
    assert_eq!(&snapshot.banks()[0].pixels()[20..24], &[1, 27, 93, 255]);
}
#[test]
fn disabled_and_budgeted_capture() {
    let disabled = prepared(1, 2).retain_material_snapshot(0);
    assert!(disabled.capture_materials(2, 1, false).snapshot().is_err());
    assert!(
        prepared(1, 2)
            .retain_material_snapshot(19)
            .capture_materials(2, 1, false)
            .snapshot()
            .is_err()
    );
    let state = prepared(1, 2).capture_materials(2, 1, false);
    assert_eq!(state.snapshot().unwrap().banks()[0].pixels().len(), 20);
}
#[test]
fn corrupted_metadata_and_overflow_fail_closed() {
    assert!(chain(1 << 31, 32).is_err());
    let pbr = prepared(1, 2).with_pbr(&[255; 16], &[255; 16], 2, 1);
    assert!(
        pbr.capture_materials(2, 1, false)
            .snapshot()
            .err()
            .unwrap()
            .contains("PBR")
    );
    for size in [0, 3, 1 << 31] {
        assert!(
            prepared(1, 2)
                .capture_materials(size, 1, false)
                .snapshot()
                .is_err()
        );
    }
    let mut p = prepared(1, 2);
    p.sprites[0].0.pop();
    assert!(p.capture_materials(2, 1, false).snapshot().is_err());
    let mut p = prepared(1, 2);
    p.sprites[0].1 = 1;
    assert!(p.capture_materials(2, 1, false).snapshot().is_err());
    assert!(
        prepared(1, 2)
            .capture_materials(2, 3, false)
            .snapshot()
            .is_err()
    );
}
#[test]
fn animation_invalidates_retained_upload_but_preserves_owned_bytes() {
    let mut state = prepared(3, 2).capture_materials(2, 3, false);
    let retained = state.snapshot().unwrap().clone();
    state.animated(&[]);
    assert_eq!(state.animation_revision(), 0);
    assert!(state.current(&retained).is_ok());
    state.animated(&[2]);
    assert_eq!(state.animation_revision(), 1); // actual animator reports global identities
    assert!(
        state
            .current(&retained)
            .err()
            .unwrap()
            .contains("animation")
    );
    assert_eq!(retained.banks()[1].pixels()[0], 2);
    assert_eq!(Arc::strong_count(&retained), 1);
    let weak = Arc::downgrade(&retained);
    drop(retained);
    assert!(weak.upgrade().is_none());
}
#[test]
fn reload_and_device_recreation_have_distinct_generations() {
    let old = prepared(1, 2).capture_materials(2, 1, false);
    let retained = old.snapshot().unwrap().clone();
    drop(old);
    let new = prepared(1, 2).capture_materials(2, 1, false);
    assert_ne!(new.generation(), retained.generation());
    assert!(new.current(&retained).is_err());
    assert_eq!(retained.banks()[0].pixels().len(), 20);
}
#[test]
fn light_revision_tracks_pixels_not_frames() {
    let mut state = State::new();
    assert!(state.light_revision().is_err());
    let zero = [0; 1024];
    let pixels = [255; 1024];
    state.observe_lightmap(&zero, &pixels);
    assert_eq!(state.light_revision(), Ok(1));
    state.observe_lightmap(&pixels, &pixels);
    assert_eq!(state.light_revision(), Ok(1));
    let mut changed = pixels;
    changed[4 * (3 * 16 + 2)] = 42;
    state.observe_lightmap(&pixels, &changed);
    assert_eq!(state.light_revision(), Ok(2));
}
