use super::*;
// Handcrafted RGBA fixtures; never labeled production-generated light.
fn exercise(change: impl FnOnce(&mut Input<'_>, &mut Limits)) -> Result<(), String> {
    let epoch = Epoch::new()?;
    let pixels = vec![17; 256 * 20]; // 2x2 + 1x1, 256 layers
    let tail = vec![29; 20];
    let banks = [
        Bank {
            pixels: &pixels,
            size: 2,
            layers: 256,
            allocated_layers: 256,
            levels: 2,
            format: Format::Rgba8Unorm,
            stamp: epoch.stamp(),
        },
        Bank {
            pixels: &tail,
            size: 2,
            layers: 1,
            allocated_layers: 1,
            levels: 2,
            format: Format::Rgba8Unorm,
            stamp: epoch.stamp(),
        },
    ];
    let light = [255; 1024];
    let mut input = Input {
        version: VERSION,
        epoch: &epoch,
        split: 256,
        sprites: 257,
        required_levels: 2,
        banks: &banks,
        lightmap: Lightmap {
            pixels: &light,
            provenance: Provenance::SyntheticFixture,
            stamp: epoch.stamp(),
            revision: 7,
        },
        source: section::Format::BakedSectionSolid,
        hdr_or_pbr: false,
    };
    let mut limits = Limits {
        max_dimension: 4096,
        max_array_layers: 256,
        payload_bytes: 6164,
    };
    change(&mut input, &mut limits);
    let resources = Resources::new(input, limits)?;
    assert_eq!(resources.sprite(0)?, (0, 0));
    assert_eq!(resources.sprite(255)?, (0, 255));
    assert_eq!(resources.sprite(256)?, (1, 0));
    assert!(resources.sprite(257).is_err());
    assert!(resources.sprite(0x8000).is_err());
    assert_eq!(resources.bank(1).unwrap().pixels.as_ptr(), tail.as_ptr());
    assert_eq!(resources.lightmap().pixels.as_ptr(), light.as_ptr());
    assert_eq!(
        resources.lightmap().provenance,
        Provenance::SyntheticFixture
    );
    assert_eq!(resources.payload_bytes, 6164);
    let vertices = [crate::terrain::TerrainVertex {
        pos: [0.0; 3],
        uv: [0; 2],
        layer: 257,
        light: 0,
        color: 0,
    }; 4];
    let bytes = crate::terrain::Mesh::from_vertices(&vertices, &[]);
    let mut indices = Vec::new();
    let mut section = section::prepare(
        section::Input {
            version: section::VERSION,
            format: section::Format::BakedSectionSolid,
            vertices: bytes.vertices,
            stride: 24,
            vertex_count: 4,
            quad_count: 1,
            origin: [0; 3],
        },
        section::Limits {
            max_buffer_bytes: 1024,
            geometry_bytes: 1024,
            texture_layers: 258,
        },
        &mut indices,
    )?;
    section.required_texture_layers = 0; // untrusted public summary cannot hide sprite 257
    assert!(resources.validate_section(&section, epoch.stamp()).is_err());
    assert!(resources.check_lightmap(epoch.stamp(), 6).is_err());
    resources.check_lightmap(epoch.stamp(), 7)?;
    assert!(resources.check_stamp(Epoch::new()?.stamp()).is_err());
    Ok(())
}
#[test]
fn cross_bank_borrowed_snapshot() {
    exercise(|_, _| {}).unwrap();
}
#[test]
fn budget_and_device_policy() {
    assert!(
        exercise(|_, l| l.payload_bytes -= 1)
            .unwrap_err()
            .contains("budget")
    );
    assert!(exercise(|_, l| l.max_array_layers = 255).is_err());
    assert!(exercise(|_, l| l.max_dimension = 1).is_err());
}
#[test]
fn missing_banks_and_provenance() {
    assert!(exercise(|i, _| i.sprites = 513).is_err());
    for source in [
        section::Format::PackedQuads,
        section::Format::Translucent,
        section::Format::TiledLod,
        section::Format::ShaderPack,
    ] {
        assert!(exercise(|i, _| i.source = source).is_err());
    }
    assert!(exercise(|i, _| i.hdr_or_pbr = true).is_err());
    assert!(exercise(|i, _| i.version += 1).is_err());
}
#[test]
fn stale_and_uninitialized_light() {
    assert!(
        exercise(|i, _| i.lightmap.stamp = Epoch::new().unwrap().stamp())
            .unwrap_err()
            .contains("stale")
    );
    static ZERO: [u8; 1024] = [0; 1024];
    assert!(
        exercise(|i, _| i.lightmap.pixels = &ZERO)
            .unwrap_err()
            .contains("lightmap")
    );
}
#[test]
fn arithmetic_and_mips() {
    assert!(
        exercise(|i, _| i.required_levels = 3)
            .unwrap_err()
            .contains("mip")
    );
    assert_eq!(chain_bytes(16, 5, 2).unwrap(), 2728);
    assert!(chain_bytes(1 << 31, 32, u32::MAX).is_err());
    assert!(chain_bytes(3, 1, 1).is_err());
    assert!(chain_bytes(2, 3, 1).is_err());
    assert!(chain_bytes(2, 0, 1).is_err());
}
#[test]
fn invalid_bank_metadata() {
    let epoch = Epoch::new().unwrap();
    let pixel = [255; 4];
    let light = [255; 1024];
    let original = Bank {
        pixels: &pixel,
        size: 1,
        layers: 1,
        allocated_layers: 1,
        levels: 1,
        format: Format::Rgba8Unorm,
        stamp: epoch.stamp(),
    };
    for bank in [
        Bank {
            format: Format::Rgba8Srgb,
            ..original
        },
        Bank {
            layers: 2,
            ..original
        },
        Bank {
            pixels: &[],
            ..original
        },
        Bank {
            stamp: Epoch::new().unwrap().stamp(),
            ..original
        },
    ] {
        let banks = [bank];
        assert!(
            Resources::new(
                Input {
                    version: VERSION,
                    epoch: &epoch,
                    split: 1,
                    sprites: 1,
                    required_levels: 1,
                    banks: &banks,
                    lightmap: Lightmap {
                        pixels: &light,
                        provenance: Provenance::SyntheticFixture,
                        stamp: epoch.stamp(),
                        revision: 0
                    },
                    source: section::Format::BakedSectionSolid,
                    hdr_or_pbr: false
                },
                Limits {
                    max_dimension: 1,
                    max_array_layers: 1,
                    payload_bytes: 1028
                }
            )
            .is_err()
        );
    }
}
#[test]
fn invalidation_never_reuses_identity() {
    let mut epoch = Epoch::new().unwrap();
    let old = epoch.stamp();
    epoch.invalidate().unwrap();
    assert_ne!(old, epoch.stamp());
    assert_ne!(epoch.stamp(), Epoch::new().unwrap().stamp());
}

#[test]
fn eight_production_banks_keep_full_ids_and_reject_a_ninth() {
    let epoch = Epoch::new().unwrap();
    let pixels = vec![255; 2048 * 4]; // handcrafted equal pixels, eight logical banks
    let light = [255; 1024];
    let banks = [Bank {
        pixels: &pixels,
        size: 1,
        layers: 2048,
        allocated_layers: 2048,
        levels: 1,
        format: Format::Rgba8Unorm,
        stamp: epoch.stamp(),
    }; 8];
    let input = |sprites| Input {
        version: VERSION,
        epoch: &epoch,
        split: 2048,
        sprites,
        required_levels: 1,
        banks: &banks,
        lightmap: Lightmap {
            pixels: &light,
            provenance: Provenance::SyntheticFixture,
            stamp: epoch.stamp(),
            revision: 0,
        },
        source: section::Format::BakedSectionSolid,
        hdr_or_pbr: false,
    };
    let limits = Limits {
        max_dimension: 1,
        max_array_layers: 2048,
        payload_bytes: 66560,
    };
    let resources = Resources::new(input(16384), limits).unwrap();
    assert_eq!(resources.sprite(2047).unwrap(), (0, 2047));
    assert_eq!(resources.sprite(2048).unwrap(), (1, 0));
    assert_eq!(resources.sprite(16383).unwrap(), (7, 2047));
    assert!(resources.sprite(16384).is_err());
    assert!(Resources::new(input(16385), limits).is_err());
}
