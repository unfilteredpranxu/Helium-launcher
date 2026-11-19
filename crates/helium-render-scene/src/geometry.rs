// MIT; Copyright (c) 2026 Horizon contributors.
pub const STRIDE: usize = 24;
pub const OPAQUE_LAYER: u16 = 0x8000;

/// Matches mesher.rs TerrainVertex. Color's high byte is emission/fluid metadata,
/// NOT coverage alpha. Sprite alpha controls cutout; baked RGB already includes AO.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
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
        let bytes = bytemuck::cast_slice(vertices);
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
pub struct View {
    pub clip_from_relative: [[f32; 4]; 4],
    /// Section origin minus camera, computed in double precision from the frame camera.
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
