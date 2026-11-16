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
            0 => s.description.version = u32::MAX,
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

#[test]
fn banked_namespace_uses_real_vertex_ids_without_padding_aliases() {
    use crate::{
        banks,
        terrain::{Mesh, OPAQUE_LAYER, TerrainVertex},
    };
    let (mut source, _) = fixture();
    source.description.version = BANKED_VERSION;
    source.description.split = 512;
    source.description.sprites = 1540;
    source.description.used = 512;
    source.description.allocated = 512;
    let mut ds = Vec::new();
    for bank in 0..4 {
        let mut d = source.description;
        d.bank = bank;
        d.used = if bank == 3 { 4 } else { 512 };
        d.allocated = d.used;
        ds.push(d);
    }
    let ids = [0, 511, 512, 1027, 1536, 1539];
    let vertices: Vec<_> = ids
        .iter()
        .flat_map(|&id| {
            [TerrainVertex {
                pos: [0.; 3],
                uv: [0; 2],
                layer: OPAQUE_LAYER | id,
                light: 0,
                color: 0x91ffffff,
            }; 3]
        })
        .collect();
    let indices: Vec<_> = (0..vertices.len() as u32).collect();
    let mesh = Mesh::from_vertices(&vertices, &indices);
    banks::check_mesh(&mesh, &ds).unwrap();
    assert_eq!(banks::required(&mesh, 512, 1540).unwrap(), [0, 1, 2, 3]);
    for missing in 0..4 {
        let subset: Vec<_> = ds
            .iter()
            .enumerate()
            .filter_map(|(i, d)| (i != missing).then_some(*d))
            .collect();
        assert!(banks::check_mesh(&mesh, &subset).is_err());
    }
    for change in 0..5 {
        let mut bad = ds.clone();
        match change {
            0 => bad[1].identity.revision += 1,
            1 => bad[1].identity.generation += 1,
            2 => bad[3].used = 5,
            3 => bad[0].levels = 1,
            _ => bad[0].split = 0,
        };
        assert!(banks::check_mesh(&mesh, &bad).is_err());
    }
    let mut bad = vertices.clone();
    bad[0].layer = 1540;
    assert!(banks::check_mesh(&Mesh::from_vertices(&bad, &indices), &ds).is_err());
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
#[test]
#[ignore = "real Metal 512-slice and cross-bank independent full-image oracle"]
fn native_512_banks_sample_high_slices_without_global_aliases() {
    use crate::{
        frame,
        terrain::{Mesh, OPAQUE_LAYER, Options, TerrainRenderer, TerrainVertex, View},
    };
    let r = TerrainRenderer::new().unwrap();
    let (base, mut a) = fixture();
    let mut sources = Vec::new();
    for bank in 0..3u32 {
        let mut s = base.clone();
        s.description.version = BANKED_VERSION;
        s.description.split = 512;
        s.description.sprites = 1027;
        s.description.bank = bank;
        s.description.used = if bank == 2 { 3 } else { 512 };
        s.description.allocated = if bank == 2 { 4 } else { 512 };
        s.description.size = 2;
        s.description.levels = 2;
        let mut pixels = Vec::with_capacity(s.description.allocated as usize * 20);
        for local in 0..s.description.allocated {
            let id = bank * 512 + local;
            let rgba = [
                (id % 251) as u8,
                ((id / 251) * 37) as u8,
                (255 - id % 251) as u8,
                if id == 1026 { 0 } else { 255 },
            ];
            for _ in 0..5 {
                pixels.extend_from_slice(&rgba);
            } // 2x2 then 1x1, exact full chain
        }
        s.pixels = Arc::new(pixels);
        sources.push(s);
    }
    let mut group = r.create_material_set(&sources, &a, budget()).unwrap();
    let ids = [0u16, 255, 256, 511, 512, 1026];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for (i, &id) in ids.iter().enumerate() {
        let x = -1. + i as f32 / 3.;
        let end = -1. + (i + 1) as f32 / 3.;
        let base = vertices.len() as u32;
        for pos in [[x, -1., 0.5], [end, -1., 0.5], [end, 1., 0.5], [x, 1., 0.5]] {
            vertices.push(TerrainVertex {
                pos,
                uv: [0; 2],
                layer: if i == 5 { id } else { OPAQUE_LAYER | id },
                light: 0,
                color: 0x91ffffff,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    // A farther bank-one backdrop must remain behind every foreground bank,
    // independent of triangle order, and show through the cutout stripe.
    let base = vertices.len() as u32;
    for pos in [
        [-1., -1., 0.25],
        [1., -1., 0.25],
        [1., 1., 0.25],
        [-1., 1., 0.25],
    ] {
        vertices.push(TerrainVertex {
            pos,
            uv: [0; 2],
            layer: OPAQUE_LAYER | 513,
            light: 0,
            color: 0x91ffffff,
        });
    }
    let mut ordered = vec![base, base + 1, base + 2, base, base + 2, base + 3];
    ordered.extend_from_slice(&indices);
    indices = ordered;
    let mut frames = r.create_frames().unwrap();
    let light = [255u8; 1024];
    let options = Options {
        width: 96,
        height: 16,
        ..Options::default()
    };
    let original_sources = sources.clone();
    for source in sources {
        let bank = source.description.bank;
        group
            .bank_mut(bank)
            .unwrap()
            .begin_upload(&r, source, &a, Update::Full, budget())
            .unwrap();
        loop {
            match group.bank_mut(bank).unwrap().poll_upload(&r, &a) {
                Poll::Completed(_) => break,
                Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(1)),
                _ => panic!("bank upload failed"),
            }
        }
        while r.upload_retention().busy {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if bank < 2 {
            assert!(
                frames
                    .begin(
                        &r,
                        &group,
                        &a,
                        &Mesh::from_vertices(&vertices, &indices),
                        &light,
                        View::identity(),
                        options
                    )
                    .is_err()
            );
        }
    }
    for opaque in [false, true] {
        if opaque {
            indices = indices.chunks_exact(3).rev().flatten().copied().collect();
            for v in &mut vertices[20..] {
                v.layer |= OPAQUE_LAYER;
            }
        }
        frames
            .begin(
                &r,
                &group,
                &a,
                &Mesh::from_vertices(&vertices, &indices),
                &light,
                View::identity(),
                options,
            )
            .unwrap();
        let mut receipt = loop {
            match frames.poll(&r, &group, &a) {
                frame::Poll::Completed(t) => break t,
                frame::Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(1)),
                _ => panic!("banked frame failed"),
            }
        };
        while r.upload_retention().busy {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let image = frames
            .take_completed(&r, &group, &a, &mut receipt)
            .unwrap()
            .readback_for_validation(1 << 20)
            .unwrap();
        for y in 0..16 {
            for x in 0..96 {
                let id = if x >= 80 && !opaque {
                    513
                } else {
                    ids[x / 16] as u32
                };
                let expected = {
                    [
                        (id % 251) as u8,
                        ((id / 251) * 37) as u8,
                        (255 - id % 251) as u8,
                        255,
                    ]
                };
                assert_eq!(
                    &image.rgba[(y * 96 + x) * 4..][..4],
                    &expected,
                    "pixel {x},{y}"
                );
            }
        }
    }
    // New animation revision: unchanged banks retain completed textures, changed
    // bank uploads only its first layer. Mixed revisions must reject while staged.
    a.identity.revision = 2;
    let mut changed = original_sources[1].clone();
    changed.description.identity = a.identity;
    Arc::make_mut(&mut changed.pixels)[0] = 99;
    let mut same = original_sources[0].clone();
    same.description.identity = a.identity;
    group
        .bank_mut(0)
        .unwrap()
        .adopt_unchanged(&r, same, &a, budget())
        .unwrap();
    assert!(
        frames
            .begin(
                &r,
                &group,
                &a,
                &Mesh::from_vertices(&vertices, &indices),
                &light,
                View::identity(),
                options
            )
            .is_err()
    );
    let mut same = original_sources[2].clone();
    same.description.identity = a.identity;
    group
        .bank_mut(2)
        .unwrap()
        .adopt_unchanged(&r, same, &a, budget())
        .unwrap();
    group
        .bank_mut(1)
        .unwrap()
        .begin_upload(
            &r,
            changed,
            &a,
            Update::Layers { first: 0, count: 1 },
            budget(),
        )
        .unwrap();
    loop {
        match group.bank_mut(1).unwrap().poll_upload(&r, &a) {
            Poll::Completed(_) => break,
            Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(1)),
            _ => panic!("partial bank update failed"),
        }
    }
    while r.upload_retention().busy {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(group.bank_counters(0).unwrap().completed_uploads, 1);
    assert_eq!(group.bank_mut(2).unwrap().counters().completed_uploads, 1);
    assert_eq!(group.bank_mut(1).unwrap().counters().upload_bytes, 10260);
    frames
        .begin(
            &r,
            &group,
            &a,
            &Mesh::from_vertices(&vertices, &indices),
            &light,
            View::identity(),
            options,
        )
        .unwrap();
    let mut receipt = loop {
        match frames.poll(&r, &group, &a) {
            frame::Poll::Completed(t) => break t,
            frame::Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(1)),
            _ => panic!("updated bank draw failed"),
        }
    };
    while r.upload_retention().busy {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let image = frames
        .take_completed(&r, &group, &a, &mut receipt)
        .unwrap()
        .readback_for_validation(1 << 20)
        .unwrap();
    assert_eq!(image.rgba[(8 * 96 + 72) * 4], 99);
    a.live = false;
    assert!(
        frames
            .begin(
                &r,
                &group,
                &a,
                &Mesh::from_vertices(&vertices, &indices),
                &light,
                View::identity(),
                options
            )
            .is_err()
    );
    a.live = true;
    group.bank_mut(1).unwrap().release();
    assert!(
        frames
            .begin(
                &r,
                &group,
                &a,
                &Mesh::from_vertices(&vertices, &indices),
                &light,
                View::identity(),
                options
            )
            .is_err()
    );
    eprintln!(
        "G06-B real GPU 512 arrays, slices 0/255/256/511, 3 banks, partial bank last-used, full image, opaque/cutout and released set passed"
    );
}
#[test]
fn v2_device_memory_mips_and_legacy_policy_are_checked() {
    let (mut s, a) = fixture();
    s.description.version = BANKED_VERSION;
    let mut c = caps();
    c.apple_families = 0;
    c.common_families = 2;
    assert!(Plan::prepare(&s, &a, None, Update::Full, &c, budget()).is_err());
    assert!(
        Plan::prepare(
            &s,
            &a,
            None,
            Update::Full,
            &caps(),
            Budget {
                payload_bytes: 1,
                staging_bytes: 1
            }
        )
        .is_err()
    );
    s.description.levels = 1;
    assert!(Plan::prepare(&s, &a, None, Update::Full, &caps(), budget()).is_err());
    let (s, _) = fixture();
    let mut d = s.description;
    d.version = BANKED_VERSION;
    d.split = 512;
    d.sprites = 1027;
    d.used = 512;
    d.allocated = 512;
    let mut ds = vec![d];
    for bad in [0, 513] {
        let mut b = d;
        b.used = bad;
        assert!(crate::banks::validate(&[b]).is_err());
    }
    for i in 1..9 {
        let mut b = d;
        b.bank = i;
        ds.push(b);
    }
    assert!(crate::banks::validate(&ds).is_err());
}
#[test]
fn unchanged_revision_rebind_does_not_issue_new_gpu_completion() {
    let device = Id::new().unwrap();
    let owner = Id::new().unwrap();
    let id = Identity {
        generation: 7,
        revision: 1,
    };
    let mut r = Record::prepared(id, device, owner).unwrap();
    assert!(r.rebind_identical(Identity { revision: 2, ..id }).is_err());
    r.advance(Phase::Prepared, Phase::Encoded).unwrap();
    r.advance(Phase::Encoded, Phase::Submitted).unwrap();
    let mut old = r.finish(true).unwrap();
    assert!(
        r.rebind_identical(Identity {
            generation: 8,
            revision: 2
        })
        .is_err()
    );
    r.rebind_identical(Identity { revision: 2, ..id }).unwrap();
    assert!(r.consume(&mut old, id, device, owner).is_err());
    assert!(r.observe(Some(true)).is_err());
    assert!(r.rebind_identical(Identity { revision: 1, ..id }).is_err());
}
#[test]
fn sparse_bank_sets_require_no_bank_zero_or_padding_alias() {
    use crate::{
        banks,
        terrain::{Mesh, TerrainVertex},
    };
    let (s, _) = fixture();
    let mut d = s.description;
    d.version = BANKED_VERSION;
    d.bank = 1;
    d.split = 512;
    d.sprites = 515;
    d.used = 3;
    d.allocated = 4;
    let mut v = [TerrainVertex {
        pos: [0.; 3],
        uv: [0; 2],
        layer: 0x8000 | 514,
        light: 0,
        color: 0x91ffffff,
    }; 3];
    banks::check_mesh(&Mesh::from_vertices(&v, &[0, 1, 2]), &[d]).unwrap();
    for x in &mut v {
        x.layer = 515;
    }
    assert!(banks::check_mesh(&Mesh::from_vertices(&v, &[0, 1, 2]), &[d]).is_err());
}
