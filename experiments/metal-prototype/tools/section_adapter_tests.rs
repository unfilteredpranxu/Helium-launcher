use super::*;

fn limits() -> Limits {
    Limits {
        max_buffer_bytes: 4096,
        geometry_bytes: 8192,
        texture_layers: 8,
    }
}
// Actual production types, deliberately hand-constructed: NOT Mesher::mesh output.
fn constructed() -> SectionMesh {
    let mut source = SectionMesh::default();
    for layer in [OPAQUE_LAYER | 2, 5] {
        source.solid.extend(
            [
                [0.125, -0.25, 0.],
                [1.25, -0.25, 0.],
                [1.25, 0.75, 0.],
                [0.125, 0.75, 0.],
            ]
            .into_iter()
            .enumerate()
            .map(|(i, pos)| {
                TerrainVertex::new(
                    pos,
                    [(i + 1) as f32 / 32768., 65535. / 32768.],
                    layer,
                    0x9140_ff80 + i as u32,
                    0x1878 + i as u16,
                )
            }),
        );
    }
    source
}

#[test]
fn actual_production_type_borrows_and_preserves_bytes_and_every_field() {
    let source = constructed();
    let original: &[u8] = bytemuck::cast_slice(&source.solid);
    let before = original.to_vec();
    let mut scratch = Vec::with_capacity(12);
    let export = expanded_solid(
        &source,
        [16, -64, -32],
        Format::BakedSectionSolid,
        limits(),
        &mut scratch,
    )
    .unwrap();
    let mesh = export.solid.mesh().unwrap();
    assert_eq!(mesh.vertices.as_ptr(), original.as_ptr());
    assert_eq!(mesh.vertices, before);
    assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    for (i, source) in source.solid.iter().enumerate() {
        let got = mesh.vertex(i).unwrap();
        assert_eq!(got.pos.map(f32::to_bits), source.pos.map(f32::to_bits));
        assert_eq!(got.uv.map(|v| f32::from(v) / 32768.), source.uv());
        assert_eq!(
            (got.layer, got.light, got.color),
            (source.layer, source.light, source.color)
        );
    }
    // Private production UV field is verified through literal byte offsets as well.
    assert_eq!(&mesh.vertices[12..16], &[1, 0, 255, 255]);
    assert_eq!(
        &mesh.vertices[16..24],
        &[2, 128, 120, 24, 128, 255, 64, 145]
    );
    mesh.validate(8).unwrap();
    assert_eq!(bytemuck::cast_slice::<_, u8>(&source.solid), before);
}

#[test]
fn preserves_real_production_sort_order_without_sorting_again() {
    let mut source = constructed();
    source.solid.rotate_left(4);
    assert_eq!(source.solid[0].layer, 5);
    source.sort_solid();
    assert_eq!(source.solid[0].layer, OPAQUE_LAYER | 2);
    let before = bytemuck::cast_slice::<_, u8>(&source.solid).to_vec();
    let mut scratch = Vec::new();
    let export = expanded_solid(
        &source,
        [0; 3],
        Format::BakedSectionSolid,
        limits(),
        &mut scratch,
    )
    .unwrap();
    assert_eq!(export.solid.mesh().unwrap().vertices, before);
    assert_eq!(source.sides.quads(), 2);
}

#[test]
fn excludes_redundant_packed_slots_but_rejects_packed_only_and_translucency() {
    let mut source = constructed();
    source.packed = vec![[u32::MAX; 6]];
    let mut scratch = Vec::new();
    let export = expanded_solid(
        &source,
        [0; 3],
        Format::BakedSectionSolid,
        limits(),
        &mut scratch,
    )
    .unwrap();
    assert_eq!(export.excluded_packed_slots, 1);
    assert_eq!(
        export.solid.mesh().unwrap().vertices.len(),
        source.solid.len() * 24
    );
    source.solid.clear();
    assert!(
        expanded_solid(
            &source,
            [0; 3],
            Format::BakedSectionSolid,
            limits(),
            &mut scratch
        )
        .is_err()
    );
    source = constructed();
    source.translucent.push(source.solid[0]);
    assert!(
        expanded_solid(
            &source,
            [0; 3],
            Format::BakedSectionSolid,
            limits(),
            &mut scratch
        )
        .is_err()
    );
}

#[test]
fn empty_malformed_metadata_and_external_provenance_are_explicit() {
    let mut source = SectionMesh::default();
    let mut scratch = Vec::new();
    assert!(
        expanded_solid(
            &source,
            [0; 3],
            Format::BakedSectionSolid,
            limits(),
            &mut scratch
        )
        .unwrap()
        .solid
        .mesh()
        .is_none()
    );
    source = constructed();
    for format in [
        Format::PackedQuads,
        Format::Translucent,
        Format::TiledLod,
        Format::ShaderPack,
    ] {
        assert!(expanded_solid(&source, [0; 3], format, limits(), &mut scratch).is_err());
    }
    source.sides = horizon_client_render::mesher::Sides::unsorted(3);
    assert!(
        expanded_solid(
            &source,
            [0; 3],
            Format::BakedSectionSolid,
            limits(),
            &mut scratch
        )
        .is_err()
    );
    source.sides = Default::default();
    source.solid.pop();
    assert!(
        expanded_solid(
            &source,
            [0; 3],
            Format::BakedSectionSolid,
            limits(),
            &mut scratch
        )
        .is_err()
    );
}

#[test]
#[ignore = "requires owner-provided prepared resources; never counted as CPU-fixture success"]
fn actual_mesher_output_with_adjacent_cubes_fractional_models_and_tint() {
    use horizon_client_render::{
        assets::Assets,
        biome::Biomes,
        blocks::BlockFacts,
        chunk::{Chunk, Light, MIN_Y},
        graphics::LeafCullingMode,
        mesher::{MeshTables, Mesher},
        models,
    };
    let root = std::path::PathBuf::from(
        std::env::var_os("HORIZON_MESH_TEST_RESOURCES")
            .expect("explicit resource directory required"),
    );
    let assets = Assets::load(&root).unwrap();
    let facts = BlockFacts::load(&root, assets.registry).unwrap();
    let render = assets.reload(&root).unwrap();
    let biomes = Biomes::load(&root).unwrap();
    let visibility = models::load_visibility(&root, assets.registry).unwrap();
    let tables = MeshTables::new(&assets, &render, &facts, visibility.clone()).unwrap();
    let mut mesher = Mesher {
        nether_shading: false,
        shader_pack_lighting: false,
        separate_ao: false,
        shader_pack: false,
        assets: &assets,
        render: &render,
        tables: &tables,
        biomes: &biomes,
        visibility,
        zoom: horizon_core::biome::BiomeZoom::new(0),
        blend_radius: 0,
        smooth_lighting: true,
        cutout_leaves: true,
        translucent_layers: &[],
        leaf_culling: LeafCullingMode::Off,
    };
    let mut chunk = Chunk::empty(0, 0);
    let state = |name| assets.registry.default_state(name).unwrap().0;
    chunk.set(6, MIN_Y + 4, 6, state("minecraft:stone"));
    chunk.set(7, MIN_Y + 4, 6, state("minecraft:stone"));
    let mesh = |chunk: &Chunk, mesher: &mut Mesher<'_>| {
        let mut region = [[None; 3]; 3];
        region[1][1] = Some(chunk);
        let mut section = mesher.mesh(&region, 0);
        section.sort_solid();
        section
    };
    assert_eq!(
        mesh(&chunk, &mut mesher).solid.len(),
        40,
        "two cubes hide their shared faces"
    );
    chunk.set(10, MIN_Y + 4, 6, state("minecraft:stone_slab"));
    chunk.set(6, MIN_Y + 4, 10, state("minecraft:grass_block"));
    let light = std::array::from_fn(|i| 0x80 | (i % 16) as u8);
    std::sync::Arc::make_mut(&mut chunk.sections[0]).light = Light::Full(Box::new(light));
    let source = mesh(&chunk, &mut mesher);
    assert!(
        source
            .solid
            .iter()
            .any(|v| v.pos.iter().any(|p| p.fract() != 0.))
    );
    let mut layers: Vec<_> = source
        .solid
        .iter()
        .map(|v| v.layer & !OPAQUE_LAYER)
        .collect();
    layers.sort_unstable();
    layers.dedup();
    assert!(layers.len() > 1);
    assert!(source.solid.windows(2).any(|v| v[0].light != v[1].light));
    assert!(source.solid.windows(2).any(|v| v[0].color != v[1].color));
    let mut scratch = Vec::new();
    let limits = Limits {
        max_buffer_bytes: 1 << 20,
        geometry_bytes: 2 << 20,
        texture_layers: render.texture_layers as usize,
    };
    let exported = expanded_solid(
        &source,
        [0, MIN_Y, 0],
        Format::BakedSectionSolid,
        limits,
        &mut scratch,
    )
    .unwrap();
    let bytes: &[u8] = bytemuck::cast_slice(&source.solid);
    assert_eq!(exported.solid.mesh().unwrap().vertices, bytes);
    assert_eq!(
        exported.solid.mesh().unwrap().vertices.as_ptr(),
        bytes.as_ptr()
    );
}
