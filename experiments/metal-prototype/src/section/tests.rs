use super::*;
use crate::terrain::{TerrainVertex, View};

// Constructed contract fixture, NOT output from Minecraft's mesher.
fn vertices() -> Vec<TerrainVertex> {
    (0..8)
        .map(|i| TerrainVertex {
            pos: [i as f32 + 0.125, -0.0625, 16.25],
            uv: [1 + i, 65535 - i],
            layer: if i < 4 { OPAQUE_LAYER | 2 } else { 7 },
            light: 0x1878 + i,
            color: 0x9140_ff80 + u32::from(i),
        })
        .collect()
}
fn input(vertices: &[TerrainVertex]) -> Input<'_> {
    Input {
        version: VERSION,
        format: Format::BakedSectionSolid,
        vertices: Mesh::from_vertices(vertices, &[]).vertices,
        stride: STRIDE,
        vertex_count: vertices.len(),
        quad_count: vertices.len() / 4,
        origin: [30_000_000, -64, -30_000_000],
    }
}
fn limits() -> Limits {
    Limits {
        max_buffer_bytes: 4096,
        geometry_bytes: 8192,
        texture_layers: 8,
    }
}

#[test]
fn borrows_exact_bytes_preserving_all_fields_order_and_diagonal() {
    let source = vertices();
    let before = Mesh::from_vertices(&source, &[]).vertices.to_vec();
    let mut scratch = Vec::with_capacity(32);
    let allocation = scratch.as_ptr();
    let prepared = prepare(input(&source), limits(), &mut scratch).unwrap();
    let mesh = prepared.mesh().unwrap();
    assert_eq!(mesh.vertices.as_ptr(), source.as_ptr().cast());
    assert_eq!(mesh.vertices, before);
    assert_eq!(mesh.indices.as_ptr(), allocation);
    assert_eq!(mesh.indices, [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    assert_eq!(prepared.required_texture_layers, 8);
    for (i, original) in source.iter().enumerate() {
        let got = mesh.vertex(i).unwrap();
        assert_eq!(got.pos.map(f32::to_bits), original.pos.map(f32::to_bits));
        assert_eq!(
            (got.uv, got.layer, got.light, got.color),
            (original.uv, original.layer, original.light, original.color)
        );
    }
    mesh.validate(8).unwrap();
    assert_eq!(Mesh::from_vertices(&source, &[]).vertices, before);
}

#[test]
fn keeps_world_origin_separate_and_subtracts_camera_before_rounding() {
    let v = vertices();
    let mut indices = Vec::new();
    let p = prepare(input(&v), limits(), &mut indices).unwrap();
    let mut clip = View::identity().clip_from_relative;
    clip[3][0] = 0.25;
    let view = p
        .view(clip, [29_999_999.875, -63.5, -30_000_000.25])
        .unwrap();
    assert_eq!(view.origin, [0.125, -0.5, 0.25]);
    assert_eq!(view.clip_from_relative, clip);
    for bad in [f64::NAN, f64::INFINITY, f64::MAX] {
        assert!(p.view(clip, [bad, 0., 0.]).is_err());
    }
    clip[1][2] = f32::NAN;
    assert!(p.view(clip, [0.; 3]).is_err());
}

#[test]
fn empty_sections_produce_no_draw_and_reuse_scratch_capacity() {
    let mut indices = vec![99; 12];
    let pointer = indices.as_ptr();
    let p = prepare(
        input(&[]),
        Limits {
            max_buffer_bytes: 0,
            geometry_bytes: 0,
            texture_layers: 0,
        },
        &mut indices,
    )
    .unwrap();
    assert!(p.mesh().is_none());
    assert_eq!((p.layout.geometry_bytes, p.required_texture_layers), (0, 0));
    assert!(indices.is_empty());
    assert_eq!(indices.as_ptr(), pointer);
}

#[test]
fn rejects_malformed_contract_counts_alignment_versions_and_other_formats() {
    let v = vertices();
    let valid = input(&v);
    let mut cases = vec![
        Input {
            version: VERSION + 1,
            ..valid
        },
        Input {
            stride: 16,
            ..valid
        },
        Input {
            vertex_count: 7,
            ..valid
        },
        Input {
            quad_count: 1,
            ..valid
        },
        Input {
            vertices: &valid.vertices[..valid.vertices.len() - 1],
            ..valid
        },
        Input {
            vertices: &valid.vertices[1..97],
            vertex_count: 4,
            quad_count: 1,
            ..valid
        },
    ];
    for format in [
        Format::PackedQuads,
        Format::Translucent,
        Format::TiledLod,
        Format::ShaderPack,
    ] {
        cases.push(Input { format, ..valid });
    }
    for bad in cases {
        let mut scratch = vec![42];
        assert!(prepare(bad, limits(), &mut scratch).is_err());
        assert_eq!(scratch, [42]);
    }
}

#[test]
fn rejects_nonfinite_positions_missing_layers_and_mixed_quad_flags() {
    for (field, value) in [(0, 0), (1, 8), (2, 2)] {
        let mut v = vertices();
        match field {
            0 => v[1].pos[2] = f32::NAN,
            _ => v[1].layer = value,
        }
        let mut scratch = vec![42];
        assert!(prepare(input(&v), limits(), &mut scratch).is_err());
        assert_eq!(scratch, [42]);
    }
}

#[test]
fn enforces_exact_budget_edges_and_checked_index_arithmetic() {
    let v = vertices();
    let sizes = layout(8).unwrap();
    assert_eq!(
        (
            sizes.vertex_bytes,
            sizes.index_count,
            sizes.index_bytes,
            sizes.geometry_bytes
        ),
        (192, 12, 48, 240)
    );
    let exact = Limits {
        max_buffer_bytes: 192,
        geometry_bytes: 240,
        texture_layers: 8,
    };
    let mut scratch = Vec::new();
    prepare(input(&v), exact, &mut scratch).unwrap();
    for limits in [
        Limits {
            max_buffer_bytes: 191,
            ..exact
        },
        Limits {
            geometry_bytes: 239,
            ..exact
        },
        Limits {
            texture_layers: 7,
            ..exact
        },
        Limits {
            texture_layers: usize::from(OPAQUE_LAYER) + 1,
            ..exact
        },
    ] {
        let before = scratch.clone();
        assert!(prepare(input(&v), limits, &mut scratch).is_err());
        assert_eq!(scratch, before);
    }
    assert!(layout(usize::MAX).is_err());
    assert!(layout(usize::MAX - 3).is_err());
    assert!(quad_indices(usize::MAX).is_err());
    assert!(quad_indices(u32::MAX as usize).is_err());
    if usize::BITS == 64 {
        let last = layout(u32::MAX as usize + 1).unwrap();
        assert_eq!(quad_indices(last.quad_count - 1).unwrap()[5], u32::MAX);
        assert!(layout(u32::MAX as usize + 5).is_err());
    }
}

#[test]
fn global_layer_ids_are_preserved_without_claiming_g02_array_support() {
    let mut v = vertices();
    for vertex in &mut v[..4] {
        vertex.layer = OPAQUE_LAYER | 1027;
    }
    let mut indices = Vec::new();
    let p = prepare(
        input(&v),
        Limits {
            texture_layers: 1028,
            ..limits()
        },
        &mut indices,
    )
    .unwrap();
    assert_eq!(p.required_texture_layers, 1028);
    let mesh = p.mesh().unwrap();
    assert_eq!(mesh.vertex(0).unwrap().layer, OPAQUE_LAYER | 1027);
    mesh.validate(1028).unwrap();
    // This is CPU geometry compatibility only: the unchanged G02 renderer's
    // full preflight separately rejects arrays above its conservative policy.
    assert!(p.required_texture_layers > crate::terrain::TEXTURE_LAYER_CEILING);
}
