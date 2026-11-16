use super::*;
use std::sync::Arc;
// Handcrafted fixtures, not Minecraft data or fabricated GPU results.
#[derive(Clone)]
pub(crate) struct Fixture {
    pub(crate) description: Description<u64>,
    pub(crate) pixels: Arc<Vec<u8>>,
}
pub(crate) struct Authority {
    pub(crate) identity: Identity<u64>,
    pub(crate) live: bool,
}
impl Source for Fixture {
    type Generation = u64;
    type Authority = Authority;
    fn description(&self) -> Description<u64> {
        self.description
    }
    fn pixels(&self) -> &[u8] {
        &self.pixels
    }
    fn retained_bytes(&self) -> usize {
        self.pixels.capacity()
    }
    fn check_current(&self, a: &Authority) -> Result<(), String> {
        if !a.live || a.identity != self.description.identity {
            Err("stale/released handcrafted authority".into())
        } else {
            Ok(())
        }
    }
}
pub(crate) fn fixture() -> (Fixture, Authority) {
    let identity = Identity {
        generation: 1,
        revision: 1,
    };
    let pixels = (0..3)
        .flat_map(|layer| {
            (0..5).flat_map(move |texel| {
                [
                    layer * 71,
                    texel * 33,
                    255,
                    [0, 127, 128, 255, 255][texel as usize],
                ]
            })
        })
        .collect();
    (
        Fixture {
            description: Description {
                version: VERSION,
                identity,
                bank: 0,
                split: 4,
                sprites: 3,
                used: 3,
                allocated: 3,
                size: 2,
                levels: 2,
                format: material::Format::Rgba8Unorm,
                layout: Layout::LayerMajorMipMajor,
            },
            pixels: Arc::new(pixels),
        },
        Authority {
            identity,
            live: true,
        },
    )
}
pub(crate) fn caps() -> Capabilities {
    Capabilities {
        apple_families: 1 << 4,
        common_families: 0,
        mac_families: 0,
        max_buffer_bytes: 1 << 20,
        unified_memory: None,
        recommended_working_set_bytes: None,
        allocated_bytes_at_creation: None,
        sample_counts: 2,
        argument_buffer_tier: None,
    }
}
pub(crate) fn budget() -> Budget {
    Budget {
        payload_bytes: 1 << 20,
        staging_bytes: 1 << 20,
    }
}
#[test]
fn full_plan_preserves_mips_slices_alpha_and_initializes_padding() {
    let (source, a) = fixture();
    let plan = Plan::prepare(&source, &a, None, Update::Full, &caps(), budget()).unwrap();
    assert_eq!(plan.regions().len(), 6);
    assert_eq!(plan.texture_bytes, 60);
    assert_eq!(plan.staging_bytes, 2304);
    assert_eq!(plan.upload_bytes, 60);
    let bytes = plan.stage(source.pixels()).unwrap();
    let mut recovered = vec![0; 60];
    for r in plan.regions() {
        for y in 0..r.width {
            recovered[r.source + y * r.width * 4..r.source + (y + 1) * r.width * 4]
                .copy_from_slice(
                    &bytes[r.staging + y * r.row..r.staging + y * r.row + r.width * 4],
                );
            assert!(
                bytes[r.staging + y * r.row + r.width * 4..r.staging + (y + 1) * r.row]
                    .iter()
                    .all(|&b| b == 0)
            );
        }
    }
    assert_eq!(recovered, source.pixels());
    assert_eq!(plan.regions()[5].source, 56);
    assert_eq!(plan.regions()[5].slice, 2);
}
#[test]
fn identities_and_partial_final_bank_never_wrap() {
    let (mut source, a) = fixture();
    source.description.bank = 1;
    source.description.sprites = 7;
    assert_eq!(source.description.slice(4).unwrap(), 0);
    assert_eq!(source.description.slice(6).unwrap(), 2);
    assert!(source.description.slice(3).is_err());
    assert!(source.description.slice(7).is_err());
    Plan::prepare(&source, &a, None, Update::Full, &caps(), budget()).unwrap();
    source.description.allocated = 4;
    Arc::make_mut(&mut source.pixels).resize(80, 0);
    let plan = Plan::prepare(&source, &a, None, Update::Full, &caps(), budget()).unwrap();
    assert_eq!(plan.regions().last().unwrap().slice, 3); // padding uploaded, never an identity
    assert!(source.description.slice(7).is_err());
}
#[test]
fn partial_upload_requires_unchanged_omitted_bytes_and_completed_base() {
    let (old, mut a) = fixture();
    let mut source = old.clone();
    source.description.identity.revision = 2;
    a.identity = source.description.identity;
    Arc::make_mut(&mut source.pixels)[59] = 17;
    let plan = Plan::prepare(
        &source,
        &a,
        Some(&old),
        Update::Layers { first: 2, count: 1 },
        &caps(),
        budget(),
    )
    .unwrap();
    assert_eq!(plan.upload_bytes, 20);
    assert_eq!(plan.staging_bytes, 768);
    assert_eq!(plan.regions()[0].source, 40);
    assert!(
        Plan::prepare(
            &source,
            &a,
            None,
            Update::Layers { first: 2, count: 1 },
            &caps(),
            budget()
        )
        .is_err()
    );
    assert!(
        Plan::prepare(
            &source,
            &a,
            Some(&old),
            Update::Layers { first: 0, count: 1 },
            &caps(),
            budget()
        )
        .unwrap_err_string()
        .contains("omits")
    );
}
// Result helper avoids imposing Debug on production Plan just for diagnostics.
trait ErrorString {
    fn unwrap_err_string(self) -> String;
}
impl<T> ErrorString for Result<T, String> {
    fn unwrap_err_string(self) -> String {
        self.err().expect("expected rejection")
    }
}
#[test]
fn malformed_descriptors_and_device_policy_fail_before_staging() {
    let (original, a) = fixture();
    for mutation in 0..13 {
        let mut s = original.clone();
        match mutation {
            0 => s.description.version = 2,
            1 => s.description.format = material::Format::Rgba8Srgb,
            2 => s.description.layout = Layout::Other,
            3 => s.description.size = 3,
            4 => s.description.levels = 1,
            5 => s.description.allocated = 257,
            6 => s.description.used = 0,
            7 => s.description.bank = 8,
            8 => s.description.split = 0,
            9 => s.description.sprites = 0,
            10 => s.description.size = 0,
            11 => s.description.allocated = 0,
            _ => s.pixels = Arc::new(vec![]),
        }
        assert!(
            Plan::prepare(&s, &a, None, Update::Full, &caps(), budget()).is_err(),
            "mutation {mutation}"
        );
    }
    let mut c = caps();
    c.max_buffer_bytes = 2303;
    assert!(Plan::prepare(&original, &a, None, Update::Full, &c, budget()).is_err());
    c = caps();
    c.apple_families = 0;
    assert!(Plan::prepare(&original, &a, None, Update::Full, &c, budget()).is_err());
}
#[test]
fn budgets_stale_sources_and_released_authority_reject() {
    let (s, mut a) = fixture();
    let plan = Plan::prepare(&s, &a, None, Update::Full, &caps(), budget()).unwrap();
    for b in [
        Budget {
            payload_bytes: plan.peak_payload - 1,
            ..budget()
        },
        Budget {
            staging_bytes: 2303,
            ..budget()
        },
    ] {
        assert!(Plan::prepare(&s, &a, None, Update::Full, &caps(), b).is_err());
    }
    a.identity.revision += 1;
    assert!(Plan::prepare(&s, &a, None, Update::Full, &caps(), budget()).is_err());
    a.identity = s.description.identity;
    a.live = false;
    assert!(Plan::prepare(&s, &a, None, Update::Full, &caps(), budget()).is_err());
}
#[test]
fn pitches_ranges_and_arithmetic_are_checked() {
    let good = CopyRegion {
        slice: 0,
        level: 0,
        width: 2,
        source: 0,
        staging: 0,
        row: 256,
        image: 512,
    };
    good.check(16, 512).unwrap();
    for r in [
        CopyRegion { row: 4, ..good },
        CopyRegion { row: 257, ..good },
        CopyRegion { image: 0, ..good },
        CopyRegion {
            source: usize::MAX,
            ..good
        },
        CopyRegion {
            staging: usize::MAX - 255,
            ..good
        },
        CopyRegion {
            width: usize::MAX,
            ..good
        },
    ] {
        assert!(r.check(16, 512).is_err());
    }
    assert!(terrain::aligned_row(usize::MAX).is_err());
    let (s, a) = fixture();
    let mut old = s.clone();
    old.description.identity.revision = 0;
    assert!(
        Plan::prepare(
            &s,
            &a,
            Some(&old),
            Update::Layers {
                first: u32::MAX,
                count: 2
            },
            &caps(),
            budget()
        )
        .is_err()
    );
    assert!(
        Plan::prepare(
            &s,
            &a,
            Some(&old),
            Update::Layers { first: 0, count: 0 },
            &caps(),
            budget()
        )
        .is_err()
    );
}
#[test]
fn simulated_events_do_not_complete_before_success_or_twice() {
    // Protocol simulation ONLY, not hardware completion evidence.
    let identity = Identity {
        generation: 7,
        revision: 1,
    };
    let device = Id::new().unwrap();
    let owner = Id::new().unwrap();
    let mut r = Record::prepared(identity, device, owner).unwrap();
    assert!(r.finish(true).is_err());
    assert_eq!(r.phase(), Phase::Prepared);
    assert!(r.advance(Phase::Prepared, Phase::Submitted).is_err());
    r.advance(Phase::Prepared, Phase::Encoded).unwrap();
    assert!(r.finish(true).is_err());
    r.advance(Phase::Encoded, Phase::Submitted).unwrap();
    let mut ticket = r.finish(true).unwrap();
    assert!(r.finish(true).is_err());
    assert!(
        r.consume(&mut ticket, identity, Id::new().unwrap(), owner)
            .is_err()
    );
    assert!(
        r.consume(&mut ticket, identity, device, Id::new().unwrap())
            .is_err()
    );
    assert!(
        r.consume(
            &mut ticket,
            Identity {
                revision: 2,
                ..identity
            },
            device,
            owner
        )
        .is_err()
    );
    r.consume(&mut ticket, identity, device, owner).unwrap();
    assert!(r.consume(&mut ticket, identity, device, owner).is_err());
    r.invalidate();
    assert_eq!(r.phase(), Phase::Invalidated);
}
#[test]
fn simulated_failure_and_superseded_operation_reject_receipts() {
    let identity = Identity {
        generation: 1,
        revision: 1,
    };
    let device = Id::new().unwrap();
    let owner = Id::new().unwrap();
    let mut old = Record::prepared(identity, device, owner).unwrap();
    old.advance(Phase::Prepared, Phase::Encoded).unwrap();
    old.advance(Phase::Encoded, Phase::Submitted).unwrap();
    let mut receipt = old.finish(true).unwrap();
    let mut next = Record::prepared(identity, device, owner).unwrap();
    next.advance(Phase::Prepared, Phase::Encoded).unwrap();
    next.advance(Phase::Encoded, Phase::Submitted).unwrap();
    assert!(next.finish(false).is_err());
    assert_eq!(next.phase(), Phase::Failed);
    assert!(next.consume(&mut receipt, identity, device, owner).is_err());
    next.fail();
    assert!(next.finish(true).is_err());
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[test]
#[ignore = "requires a real native Metal device; readback and full G02 image oracle"]
fn native_material_upload_readback_updates_and_resident_render() {
    use crate::terrain::{Options, TerrainRenderer, validation};
    let renderer = TerrainRenderer::new().expect("native Metal required");
    let (source, mut a) = fixture();
    let expected = source.pixels().to_vec();
    let weak = Arc::downgrade(&source.pixels);
    let mut material = renderer.create_material(&source, &a, budget()).unwrap();
    assert_eq!(material.phase(), Phase::Prepared);
    let mut receipt = material
        .upload_blocking(&renderer, source, &a, Update::Full, budget())
        .unwrap();
    assert_eq!(
        material
            .readback_for_validation(&renderer, &a, budget())
            .unwrap(),
        expected
    );
    assert!(weak.upgrade().is_some());
    material
        .consume_ticket(&renderer, &a, &mut receipt)
        .unwrap();
    assert!(
        material
            .consume_ticket(&renderer, &a, &mut receipt)
            .is_err()
    );
    let (mut next, _) = fixture();
    next.description.identity.revision = 2;
    Arc::make_mut(&mut next.pixels)[59] = 17;
    a.identity = next.description.identity;
    let expected = next.pixels().to_vec();
    let mut ticket = material
        .upload_blocking(
            &renderer,
            next,
            &a,
            Update::Layers { first: 2, count: 1 },
            budget(),
        )
        .unwrap();
    assert!(weak.upgrade().is_none());
    assert_eq!(material.counters().reused_resources, 1);
    assert_eq!(material.counters().completed_uploads, 2);
    assert_eq!(material.counters().upload_bytes, 80);
    assert_eq!(material.counters().texture_payload, 60);
    assert_eq!(material.counters().staging_peak, 2304);
    assert_eq!(material.counters().pending, 0);
    assert_eq!(
        material
            .readback_for_validation(&renderer, &a, budget())
            .unwrap(),
        expected
    );
    let other = TerrainRenderer::new().unwrap();
    assert!(material.consume_ticket(&other, &a, &mut ticket).is_err());
    material.release();
    assert!(material.consume_ticket(&renderer, &a, &mut ticket).is_err());

    let chart = validation::fixture();
    let (mut source, mut a) = fixture();
    source.pixels = Arc::new(
        chart
            .sprites
            .chunks_exact(16)
            .flat_map(|layer| layer.iter().copied().chain(layer[..4].iter().copied()))
            .collect(),
    );
    let mut material = renderer.create_material(&source, &a, budget()).unwrap();
    material
        .upload_blocking(&renderer, source, &a, Update::Full, budget())
        .unwrap();
    let image = renderer
        .render_material(
            &material,
            &a,
            &chart.mesh(),
            &chart.lightmap,
            validation::fixture_view(),
            Options::default(),
        )
        .unwrap();
    validation::check_image(&image).unwrap();
    let again = renderer
        .render_material(
            &material,
            &a,
            &chart.mesh(),
            &chart.lightmap,
            validation::fixture_view(),
            Options::default(),
        )
        .unwrap();
    assert_eq!(image.rgba, again.rgba);
    a.live = false;
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
}

#[test]
fn immutable_plan_and_exact_256_slice_policy_boundary() {
    let (mut source, a) = fixture();
    let original = Plan::prepare(&source, &a, None, Update::Full, &caps(), budget()).unwrap();
    source.description.size = u32::MAX;
    assert_eq!(original.description().size, 2); // later reads cannot change GPU allocation dimensions
    assert!(original.stage(&[]).is_err());
    source.description.size = 1;
    source.description.levels = 1;
    source.description.used = 256;
    source.description.allocated = 256;
    source.description.split = 256;
    source.description.sprites = 256;
    source.pixels = Arc::new(vec![255; 1024]);
    let plan = Plan::prepare(&source, &a, None, Update::Full, &caps(), budget()).unwrap();
    assert_eq!(plan.regions().last().unwrap().slice, 255);
    assert_eq!(plan.description().slice(255).unwrap(), 255);
    source.description.used = 512;
    source.description.allocated = 512;
    source.description.split = 512;
    source.description.sprites = 512;
    assert!(
        Plan::prepare(&source, &a, None, Update::Full, &caps(), budget())
            .unwrap_err_string()
            .contains("G02")
    );
}
