//! G03-A: borrowed, versioned expanded-solid section input. No production/wgpu dependency.
//! MIT; Copyright (c) 2026 Ludovic Marie and the Horizon contributors.
use crate::terrain::{self, Mesh, OPAQUE_LAYER, STRIDE};

pub const VERSION: u32 = 1;

/// Caller attestation: bytes cannot reveal their mesher/UV/material provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    BakedSectionSolid,
    PackedQuads,
    Translucent,
    TiledLod,
    ShaderPack,
}

/// Immutable native little-endian production Pod bytes, not a disk/C ABI format.
/// Origin is in world blocks, NOT section indices; UVs use 32768 steps/unit.
/// The producer must identify normal section lighting (AO already baked in RGB).
#[derive(Clone, Copy)]
pub struct Input<'a> {
    pub version: u32,
    pub format: Format,
    pub vertices: &'a [u8],
    pub stride: usize,
    pub vertex_count: usize,
    pub quad_count: usize,
    pub origin: [i32; 3],
}

/// Caller policy, not a device RAM/jetsam estimate. Supply actual GPU buffer limits
/// before uploading; G02 separately budgets textures, targets, staging and readback.
/// texture_layers describes the source atlas's GLOBAL layer namespace. Successful
/// CPU bridging does not bypass G02's narrower single-array texture policy.
#[derive(Clone, Copy)]
pub struct Limits {
    pub max_buffer_bytes: usize,
    pub geometry_bytes: usize,
    pub texture_layers: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub quad_count: usize,
    pub vertex_bytes: usize,
    pub index_count: usize,
    pub index_bytes: usize,
    pub geometry_bytes: usize,
}

/// Size-only preflight, including huge-count rejection without allocating memory.
pub fn layout(vertex_count: usize) -> Result<Layout, String> {
    if !vertex_count.is_multiple_of(4) {
        return Err("section vertices must form complete four-corner quads".into());
    }
    if vertex_count > 0 && u32::try_from(vertex_count - 1).is_err() {
        return Err("section vertex index exceeds uint32".into());
    }
    let quad_count = vertex_count / 4;
    let vertex_bytes = vertex_count
        .checked_mul(STRIDE)
        .ok_or("vertex size overflow")?;
    let index_count = quad_count.checked_mul(6).ok_or("index count overflow")?;
    let index_bytes = index_count.checked_mul(4).ok_or("index size overflow")?;
    let geometry_bytes = vertex_bytes
        .checked_add(index_bytes)
        .ok_or("geometry size overflow")?;
    if [vertex_bytes, index_bytes]
        .into_iter()
        .any(|n| n > isize::MAX as usize)
    {
        return Err("section buffer exceeds Rust allocation bounds".into());
    }
    Ok(Layout {
        quad_count,
        vertex_bytes,
        index_count,
        index_bytes,
        geometry_bytes,
    })
}

fn quad_indices(quad: usize) -> Result<[u32; 6], String> {
    let base = quad
        .checked_mul(4)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or("quad index overflow")?;
    let end = base.checked_add(3).ok_or("quad corner overflow")?;
    Ok([base, base + 1, base + 2, base, base + 2, end])
}

/// Borrows source vertices and the caller's reusable index scratch separately.
/// Neither can be mutated/freed while this result is live. No GPU resources exist.
pub struct Prepared<'v, 'i> {
    vertices: &'v [u8],
    indices: &'i [u32],
    pub layout: Layout,
    pub origin: [i32; 3],
    pub required_texture_layers: usize,
}
impl Prepared<'_, '_> {
    /// Empty sections are valid CPU results, but must not create a G02 draw.
    pub fn mesh(&self) -> Option<Mesh<'_>> {
        (!self.vertices.is_empty()).then_some(Mesh {
            vertices: self.vertices,
            indices: self.indices,
        })
    }

    /// Subtract the camera in f64 BEFORE conversion; never rebase vertex bytes.
    /// The caller provides G02's column-major clip matrix and depth configuration.
    pub fn view(&self, clip: [[f32; 4]; 4], camera: [f64; 3]) -> Result<terrain::View, String> {
        let origin = std::array::from_fn(|k| (f64::from(self.origin[k]) - camera[k]) as f32);
        if !camera.iter().all(|v| v.is_finite())
            || !clip
                .iter()
                .flatten()
                .chain(origin.iter())
                .all(|v| v.is_finite())
        {
            return Err("non-finite section camera transform".into());
        }
        Ok(terrain::View {
            clip_from_relative: clip,
            origin,
        })
    }
}

/// Validates once per call, then fills caller-owned indices. Input errors and
/// reservation failure leave scratch contents untouched. Existing capacity is reused;
/// scratch is not a trusted index cache, so every successful call rewrites its pattern.
pub fn prepare<'v, 'i>(
    input: Input<'v>,
    limits: Limits,
    indices: &'i mut Vec<u32>,
) -> Result<Prepared<'v, 'i>, String> {
    if input.version != VERSION || input.format != Format::BakedSectionSolid {
        return Err("unsupported section contract version or geometry/material encoding".into());
    }
    if !cfg!(target_endian = "little") || input.stride != STRIDE {
        return Err("section contract requires little-endian 24-byte vertices".into());
    }
    let sizes = layout(input.vertex_count)?;
    if sizes.quad_count != input.quad_count || input.vertices.len() != sizes.vertex_bytes {
        return Err("section vertex/quad counts or byte length disagree".into());
    }
    if !input.vertices.is_empty() && !input.vertices.as_ptr().addr().is_multiple_of(4) {
        return Err("production section vertex bytes must have four-byte alignment".into());
    }
    if limits.texture_layers > usize::from(OPAQUE_LAYER)
        || sizes.vertex_bytes > limits.max_buffer_bytes
        || sizes.index_bytes > limits.max_buffer_bytes
        || sizes.geometry_bytes > limits.geometry_bytes
    {
        return Err(
            "section exceeds the source layer namespace or caller geometry/buffer budget".into(),
        );
    }
    let bytes = Mesh {
        vertices: input.vertices,
        indices: &[],
    };
    let mut required_texture_layers = 0;
    for q in 0..sizes.quad_count {
        let first = bytes.vertex(q * 4)?.layer;
        for corner in 0..4 {
            let v = bytes.vertex(q * 4 + corner)?;
            let layer = usize::from(v.layer & !OPAQUE_LAYER);
            if !v.pos.iter().all(|p| p.is_finite())
                || layer >= limits.texture_layers
                || v.layer != first
            {
                return Err(
                    "section has non-finite positions, missing layers or mixed quad layers/flags"
                        .into(),
                );
            }
            required_texture_layers = required_texture_layers.max(layer + 1);
        }
    }
    indices
        .try_reserve_exact(sizes.index_count.saturating_sub(indices.len()))
        .map_err(|_| "section index allocation failed")?;
    indices.clear();
    for q in 0..sizes.quad_count {
        // layout already proved every quad/corner is uint32-representable.
        indices.extend(quad_indices(q)?);
    }
    Ok(Prepared {
        vertices: input.vertices,
        indices,
        layout: sizes,
        origin: input.origin,
        required_texture_layers,
    })
}

#[cfg(test)]
mod tests;
