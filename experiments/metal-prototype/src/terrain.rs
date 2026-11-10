//! G02-A terrain contract, independent of the production Rust/wgpu workspace.
//! MIT; Copyright (c) 2026 Ludovic Marie and the Horizon contributors.
use crate::Image;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod metal;
pub mod validation;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use metal::TerrainRenderer;

pub const STRIDE: usize = 24;
pub const OPAQUE_LAYER: u16 = 0x8000;
pub const UV_STEP: f32 = 1.0 / 32768.0;

/// Matches mesher.rs TerrainVertex. Color's high byte is emission/fluid metadata,
/// NOT coverage alpha. Sprite alpha controls cutout; baked RGB already includes AO.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainVertex {
    pub pos: [f32; 3],
    pub uv: [u16; 2],
    pub layer: u16,
    pub light: u16,
    pub color: u32,
}
const _: () = {
    assert!(size_of::<TerrainVertex>() == STRIDE);
    assert!(align_of::<TerrainVertex>() == 4);
    assert!(std::mem::offset_of!(TerrainVertex, uv) == 12);
    assert!(std::mem::offset_of!(TerrainVertex, layer) == 16);
    assert!(std::mem::offset_of!(TerrainVertex, light) == 18);
    assert!(std::mem::offset_of!(TerrainVertex, color) == 20);
};

/// No geometry allocation/conversion. Production may pass its Pod vertex bytes
/// directly (e.g. bytemuck::cast_slice(section.solid)); no type punning is required.
pub struct Mesh<'a> {
    pub vertices: &'a [u8],
    pub indices: &'a [u32],
}
impl<'a> Mesh<'a> {
    pub fn from_vertices(vertices: &'a [TerrainVertex], indices: &'a [u32]) -> Self {
        // SAFETY: the const assertions establish no padding. Every field is
        // initialized and accepts all bit patterns; this immutable borrow cannot
        // outlive the source slice. Vec/slice allocation limits bound multiplication.
        let bytes = unsafe {
            std::slice::from_raw_parts(vertices.as_ptr().cast(), std::mem::size_of_val(vertices))
        };
        Self {
            vertices: bytes,
            indices,
        }
    }
    pub fn vertex(&self, index: usize) -> Result<TerrainVertex, String> {
        let start = index.checked_mul(STRIDE).ok_or("vertex offset overflow")?;
        let end = start.checked_add(STRIDE).ok_or("vertex end overflow")?;
        let b = self
            .vertices
            .get(start..end)
            .ok_or("vertex outside byte stream")?;
        // Read scalars, never form an aligned reference into arbitrary caller bytes.
        let word = |at| u16::from_le_bytes([b[at], b[at + 1]]);
        Ok(TerrainVertex {
            pos: std::array::from_fn(|i| {
                f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())
            }),
            uv: [word(12), word(14)],
            layer: word(16),
            light: word(18),
            color: u32::from_le_bytes(b[20..24].try_into().unwrap()),
        })
    }
    pub fn validate(&self, layers: usize) -> Result<(), String> {
        if !cfg!(target_endian = "little") {
            return Err("little-endian terrain contract required".into());
        }
        if self.vertices.is_empty()
            || !self.vertices.len().is_multiple_of(STRIDE)
            || self.indices.is_empty()
            || !self.indices.len().is_multiple_of(3)
        {
            return Err("expected 24-byte vertices and nonempty indexed triangles".into());
        }
        for i in 0..self.vertices.len() / STRIDE {
            let v = self.vertex(i)?;
            if !v.pos.iter().all(|p| p.is_finite())
                || usize::from(v.layer & !OPAQUE_LAYER) >= layers
            {
                return Err("non-finite position or missing sprite layer".into());
            }
        }
        for tri in self.indices.chunks_exact(3) {
            let a = self.vertex(tri[0] as usize)?;
            if self.vertex(tri[1] as usize)?.layer != a.layer
                || self.vertex(tri[2] as usize)?.layer != a.layer
            {
                return Err("triangle must have one flat sprite layer/flag".into());
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct Atlas<'a> {
    pub width: usize,
    pub height: usize,
    pub layers: usize,
    /// Tightly packed RGBA8Unorm, row-major, layer-major. No sRGB conversion.
    pub rgba: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Reversed,
    Forward,
}
#[derive(Clone, Copy)]
pub struct View {
    pub clip_from_relative: [[f32; 4]; 4],
    /// Section origin minus camera, computed in double precision by a future caller.
    pub origin: [f32; 3],
}
impl View {
    pub fn identity() -> Self {
        Self {
            clip_from_relative: [
                [1., 0., 0., 0.],
                [0., 1., 0., 0.],
                [0., 0., 1., 0.],
                [0., 0., 0., 1.],
            ],
            origin: [0.; 3],
        }
    }
}

/// Values reported by this MTLDevice, not a marketing-model table.
#[derive(Clone, Debug)]
pub struct Capabilities {
    pub apple_families: u16,
    pub common_families: u8,
    pub mac_families: u8,
    pub max_buffer_bytes: usize,
    pub unified_memory: Option<bool>,
    pub recommended_working_set_bytes: Option<u64>,
    pub allocated_bytes_at_creation: Option<usize>,
    /// Actual supportsTextureSampleCount responses, bit N means sample count N.
    pub sample_counts: u16,
    pub argument_buffer_tier: Option<usize>,
}
impl Capabilities {
    pub(crate) fn baseline(&self) -> bool {
        // Known Apple4..Apple10 or Common1..Common3. Zero can also mean an OS-
        // guarded query was not issued, so it is not a claim about future hardware.
        self.apple_families & 0x07f0 != 0 || self.common_families & 0x0e != 0
    }
}

/// Experiment policy, NOT a device's RAM, jetsam limit, or queried texture maximum.
/// A Common1/Apple4 baseline is required; requests stay conservatively below its
/// texture limits. Metal has no general maxTextureDimension2D getter.
pub const TEXTURE_SIDE_CEILING: usize = 4096;
pub const TEXTURE_LAYER_CEILING: usize = 256;
#[derive(Clone, Copy)]
pub struct Budget {
    pub payload_bytes: usize,
    pub target_pixels: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            payload_bytes: 32 * 1024 * 1024,
            target_pixels: 1024 * 1024,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Options {
    pub width: usize,
    pub height: usize,
    pub depth: Depth,
    pub budget: Budget,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            width: crate::SIDE,
            height: crate::SIDE,
            depth: Depth::Reversed,
            budget: Budget::default(),
        }
    }
}

pub(crate) fn product(values: &[usize]) -> Result<usize, String> {
    values.iter().try_fold(1usize, |a, &b| {
        a.checked_mul(b).ok_or("resource size overflow".into())
    })
}
fn aligned_row(width: usize) -> Result<usize, String> {
    product(&[width, 4])?
        .checked_add(255)
        .map(|n| n & !255)
        .ok_or("row alignment overflow".into())
}
pub struct ResourceLayout {
    pub index_bytes: usize,
    pub atlas_row: usize,
    pub atlas_slice: usize,
    pub atlas_staging: usize,
    pub staging_bytes: usize,
    pub readback_row: usize,
    pub readback_bytes: usize,
    pub payload_bytes: usize,
}
pub fn validate(
    mesh: &Mesh<'_>,
    atlas: Atlas<'_>,
    lightmap: &[u8],
    view: View,
    options: Options,
    caps: &Capabilities,
) -> Result<ResourceLayout, String> {
    if !caps.baseline() || caps.sample_counts & (1 << 1) == 0 {
        return Err("unrecognized Metal baseline; no inferred capabilities".into());
    }
    if atlas.width == 0
        || atlas.height == 0
        || atlas.layers == 0
        || atlas.layers > TEXTURE_LAYER_CEILING
        || [atlas.width, atlas.height, options.width, options.height]
            .iter()
            .any(|&n| n == 0 || n > TEXTURE_SIDE_CEILING)
    {
        return Err("texture/target extent exceeds conservative probe policy".into());
    }
    let texture_bytes = product(&[atlas.width, atlas.height, atlas.layers, 4])?;
    if atlas.rgba.len() != texture_bytes
        || lightmap.len() != 16 * 16 * 4
        || lightmap.chunks_exact(4).any(|p| p[3] != 255)
    {
        return Err("texture bytes or 16x16 lightmap length mismatch".into());
    }
    if !view
        .clip_from_relative
        .iter()
        .flatten()
        .chain(view.origin.iter())
        .all(|v| v.is_finite())
    {
        return Err("non-finite camera/origin".into());
    }
    let pixels = product(&[options.width, options.height])?;
    if pixels > options.budget.target_pixels {
        return Err("target pixel budget exceeded".into());
    }
    let atlas_row = aligned_row(atlas.width)?;
    let atlas_slice = product(&[atlas_row, atlas.height])?;
    let atlas_staging = product(&[atlas_slice, atlas.layers])?;
    let staging_bytes = atlas_staging
        .checked_add(256 * 16)
        .ok_or("staging overflow")?;
    let readback_row = aligned_row(options.width)?;
    let readback_bytes = product(&[readback_row, options.height])?;
    let index_bytes = product(&[mesh.indices.len(), 4])?;
    for size in [
        mesh.vertices.len(),
        index_bytes,
        staging_bytes,
        readback_bytes,
        80,
    ] {
        if size > caps.max_buffer_bytes || size > isize::MAX as usize {
            return Err("MTLDevice buffer limit exceeded".into());
        }
    }
    let payload_bytes = [
        mesh.vertices.len(),
        index_bytes,
        staging_bytes,
        readback_bytes,
        texture_bytes,
        16 * 16 * 4,
        product(&[pixels, 8])?,
        80,
        staging_bytes,
        product(&[pixels, 4])?,
    ]
    .into_iter()
    .try_fold(0usize, |a, b| {
        a.checked_add(b).ok_or("resident payload overflow")
    })?;
    if payload_bytes > options.budget.payload_bytes {
        return Err("resident payload budget exceeded".into());
    }
    mesh.validate(atlas.layers)?;
    Ok(ResourceLayout {
        index_bytes,
        atlas_row,
        atlas_slice,
        atlas_staging,
        staging_bytes,
        readback_row,
        readback_bytes,
        payload_bytes,
    })
}

/// The new oracle runs independently; the original G01 smoke/API is unchanged.
pub fn smoke_test() -> Result<Image, String> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    return validation::run_gpu();
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    Err("direct Metal requires macOS or iOS".into())
}

/// # Safety
/// Allocation must cover height*row_pitch, with width*4 initialized bytes at each
/// row start, after GPU completion. Sizes/stride were checked by validate().
/// Padding may be uninitialized and is never viewed as initialized Rust bytes.
#[cfg(any(target_os = "macos", target_os = "ios", test))]
pub(crate) unsafe fn copy_readback(
    pointer: *const u8,
    width: usize,
    height: usize,
    row_pitch: usize,
) -> Vec<u8> {
    let row_bytes = width * 4;
    let mut result = Vec::with_capacity(row_bytes * height);
    for row in 0..height {
        // SAFETY: row's initialized prefix fits the validated allocation/stride.
        let bytes = unsafe { std::slice::from_raw_parts(pointer.add(row * row_pitch), row_bytes) };
        result.extend_from_slice(bytes);
    }
    result
}
