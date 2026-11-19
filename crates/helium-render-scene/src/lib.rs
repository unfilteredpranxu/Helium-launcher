//! CPU terrain data shared between the game adapter and direct Metal.
//! MIT; Copyright (c) 2026 Horizon contributors.
#![forbid(unsafe_code)]

mod geometry;
mod materials;

pub use geometry::{Mesh, OPAQUE_LAYER, STRIDE, TerrainVertex, View};
pub use materials::{BankDescriptor, BankStorage, MaterialBank, MaterialSet, MaterialVersion};

use std::{num::NonZeroU64, sync::Arc};

pub type SectionKey = (i32, i32, i32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SectionVersion {
    pub epoch: NonZeroU64,
    pub generation: u64,
    pub key: SectionKey,
    pub revision: u64,
}

/// Owned expanded near-terrain data. Full vertex/index/sprite validation
/// happens before GPU admission through `Mesh::validate`.
pub struct Section {
    pub version: SectionVersion,
    pub origin: [i32; 3],
    pub bounds: [[f32; 3]; 2],
    pub omitted_translucent_vertices: usize,
    vertices: Vec<u8>,
    indices: Vec<u32>,
}

impl Section {
    pub fn new(
        version: SectionVersion,
        origin: [i32; 3],
        bounds: [[f32; 3]; 2],
        omitted_translucent_vertices: usize,
        vertices: Vec<u8>,
        indices: Vec<u32>,
        budget: usize,
    ) -> Result<Self, String> {
        let bytes = vertices
            .capacity()
            .checked_add(
                indices
                    .capacity()
                    .checked_mul(size_of::<u32>())
                    .ok_or("index capacity overflow")?,
            )
            .ok_or("geometry capacity overflow")?;
        if bytes > budget
            || !vertices.len().is_multiple_of(STRIDE)
            || !indices.len().is_multiple_of(3)
        {
            return Err("invalid section geometry layout or budget".into());
        }
        if version.generation == 0
            || version.revision == 0
            || (0..3).any(|axis| {
                !bounds[0][axis].is_finite()
                    || !bounds[1][axis].is_finite()
                    || bounds[0][axis] > bounds[1][axis]
            })
        {
            return Err("invalid section provenance or bounds".into());
        }
        Ok(Self {
            version,
            origin,
            bounds,
            omitted_translucent_vertices,
            vertices,
            indices,
        })
    }

    pub fn mesh(&self) -> Mesh<'_> {
        Mesh {
            vertices: &self.vertices,
            indices: &self.indices,
        }
    }

    pub fn retained_bytes(&self) -> usize {
        self.vertices.capacity() + self.indices.capacity() * size_of::<u32>()
    }
}

#[derive(Clone, Copy)]
pub struct Camera {
    pub position: [f64; 3],
    pub clip_from_relative: [[f32; 4]; 4],
}
impl Camera {
    pub fn section_view(self, origin: [i32; 3]) -> Result<View, String> {
        let origin = std::array::from_fn(|i| (f64::from(origin[i]) - self.position[i]) as f32);
        if !origin
            .iter()
            .chain(self.clip_from_relative.iter().flatten())
            .all(|n| n.is_finite())
        {
            return Err("invalid camera-relative transform".into());
        }
        Ok(View {
            clip_from_relative: self.clip_from_relative,
            origin,
        })
    }
}

pub struct Lightmap {
    pub revision: u64,
    pub rgba: [u8; 1024],
}

/// Each frame is a complete selection. Missing sections can retire from the
/// renderer cache. Producers can coalesce intermediate updates into a new frame.
pub struct Frame<S> {
    pub epoch: NonZeroU64,
    pub camera: Camera,
    pub lightmap: Lightmap,
    pub materials: Arc<MaterialSet>,
    pub sections: Vec<S>,
}

impl<S> Frame<S> {
    pub fn accepts(&self, section: &Section) -> bool {
        section.version.epoch == self.epoch
            && section.version.generation == self.materials.version().generation
    }
}

#[cfg(test)]
mod tests;
