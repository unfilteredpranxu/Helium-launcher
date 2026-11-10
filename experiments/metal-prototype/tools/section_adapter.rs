//! Production-side G03-A adapter; compile with test_section_bridge.py, not the iOS app.
//! MIT; Copyright (c) 2026 Ludovic Marie and the Horizon contributors.
use helium_metal_prototype::section::{self, Format, Input, Limits, Prepared};
use horizon_client_render::mesher::{OPAQUE_LAYER, SectionMesh, TerrainVertex, UV_STEPS};

const _: () = {
    assert!(size_of::<TerrainVertex>() == 24 && align_of::<TerrainVertex>() == 4);
    assert!(std::mem::offset_of!(TerrainVertex, pos) == 0);
    assert!(std::mem::offset_of!(TerrainVertex, layer) == 16);
    assert!(std::mem::offset_of!(TerrainVertex, light) == 18);
    assert!(std::mem::offset_of!(TerrainVertex, color) == 20);
    assert!(UV_STEPS == 32768.0 && OPAQUE_LAYER == 0x8000);
};

pub struct Export<'v, 'i> {
    pub solid: Prepared<'v, 'i>,
    /// Redundant packed slots explicitly EXCLUDED, never decoded as vertices.
    pub excluded_packed_slots: usize,
}

/// Caller MUST identify an ordinary baked section, not LOD or shader-pack output:
/// SectionMesh itself does not carry that provenance. Any translucency rejects the
/// whole export, so this never silently drops a translucent part of a section.
/// The world-block origin is supplied externally, not inferred from local vertices.
pub fn expanded_solid<'v, 'i>(
    source: &'v SectionMesh,
    origin: [i32; 3],
    format: Format,
    limits: Limits,
    indices: &'i mut Vec<u32>,
) -> Result<Export<'v, 'i>, String> {
    if !source.translucent.is_empty() || (source.solid.is_empty() && !source.packed.is_empty()) {
        return Err("translucent or packed-only SectionMesh unsupported; nothing exported".into());
    }
    let quads = source.solid.len() / 4;
    if source.sides.quads() != 0 && source.sides.quads() as usize != quads {
        return Err("SectionMesh side metadata disagrees with expanded quad count".into());
    }
    let solid = section::prepare(
        Input {
            version: section::VERSION,
            format,
            vertices: bytemuck::cast_slice(&source.solid),
            stride: size_of::<TerrainVertex>(),
            vertex_count: source.solid.len(),
            quad_count: quads,
            origin,
        },
        limits,
        indices,
    )?;
    Ok(Export {
        solid,
        excluded_packed_slots: source.packed.len(),
    })
}

#[cfg(test)]
#[path = "section_adapter_tests.rs"]
mod tests;
