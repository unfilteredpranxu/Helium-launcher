//! G04 v1: borrowed CPU snapshots; no GPU handles or implied G02 compatibility.
//! MIT; Copyright (c) 2026 Horizon contributors.
use crate::section;
use std::sync::atomic::{AtomicU64, Ordering};

pub const VERSION: u32 = 1;
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Unique, non-reusable epoch. Replace on pack reload, bank regeneration, animation
/// update or device recreation. A snapshot borrows this owner, preventing mutation
/// while consumed. Never use addresses or ticks as resource identities.
/// ```compile_fail
/// use helium_metal_prototype::material::Epoch;
/// let mut owner = Epoch::new().unwrap();
/// let snapshot_owner = &owner;
/// owner.invalidate().unwrap();
/// let still_live = snapshot_owner.stamp();
/// ```
pub struct Epoch(u64);
impl Epoch {
    pub fn new() -> Result<Self, String> {
        NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map(Self)
            .map_err(|_| "resource epoch exhausted".into())
    }
    pub fn stamp(&self) -> Stamp {
        Stamp(self.0)
    }
    pub fn invalidate(&mut self) -> Result<(), String> {
        *self = Self::new()?;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rgba8Unorm,
    Rgba8Srgb,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provenance {
    ProductionGenerated,
    SyntheticFixture,
}
/// Layer-major, then mip-major, tightly packed RGBA. Padding layers are NOT sprites.
#[derive(Clone, Copy)]
pub struct Bank<'a> {
    pub pixels: &'a [u8],
    pub size: u32,
    pub layers: u32,
    pub allocated_layers: u32,
    pub levels: u32,
    pub format: Format,
    pub stamp: Stamp,
}
#[derive(Clone, Copy)]
pub struct Lightmap<'a> {
    /// Rows are sky, columns block; linear sampling at texel centers. No regeneration.
    pub pixels: &'a [u8; 1024],
    pub provenance: Provenance,
    pub stamp: Stamp,
    /// Producer revision, advanced when actual generated pixels change.
    pub revision: u64,
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub max_dimension: u32,
    pub max_array_layers: u32,
    pub payload_bytes: usize,
}
pub struct Input<'a> {
    pub version: u32,
    pub epoch: &'a Epoch,
    pub split: u32,
    pub sprites: u32,
    /// Number of mip levels the consumer requires, including the base level.
    pub required_levels: u32,
    pub banks: &'a [Bank<'a>],
    pub lightmap: Lightmap<'a>,
    /// Explicit provenance attestation, as in G03. HDR/PBR and packs unsupported.
    pub source: section::Format,
    pub hdr_or_pbr: bool,
}
/// Immutable validated metadata. All texels remain owned by the producer.
pub struct Resources<'a> {
    input: Input<'a>,
    pub payload_bytes: usize,
}

pub fn chain_bytes(size: u32, levels: u32, layers: u32) -> Result<usize, String> {
    if !size.is_power_of_two() || levels == 0 || levels > size.ilog2() + 1 || layers == 0 {
        return Err("invalid square dimensions, mip count or layer count".into());
    }
    let mut bytes = 0usize;
    for level in 0..levels {
        let edge = (size >> level) as usize;
        bytes = bytes
            .checked_add(
                edge.checked_mul(edge)
                    .and_then(|n| n.checked_mul(4))
                    .ok_or("mip byte overflow")?,
            )
            .ok_or("chain byte overflow")?;
    }
    bytes
        .checked_mul(layers as usize)
        .filter(|n| *n <= isize::MAX as usize)
        .ok_or_else(|| "bank allocation size overflow".into())
}
impl<'a> Resources<'a> {
    pub fn new(input: Input<'a>, limits: Limits) -> Result<Self, String> {
        if input.version != VERSION
            || input.source != section::Format::BakedSectionSolid
            || input.hdr_or_pbr
        {
            return Err(
                "unsupported material version, geometry provenance or HDR/PBR semantics".into(),
            );
        }
        if input.split == 0 || input.split > 2048 || input.sprites == 0 || input.sprites > 32768 {
            return Err("invalid production sprite namespace or split".into());
        }
        let count = input.sprites.div_ceil(input.split) as usize;
        if count > 8 || input.banks.len() != count {
            return Err("missing/excess production banks (maximum eight)".into());
        }
        let first = input.banks[0];
        if input.required_levels == 0 || first.levels < input.required_levels {
            return Err("missing required mip levels".into());
        }
        let mut payload = 1024usize;
        for (index, bank) in input.banks.iter().enumerate() {
            let used = (input.sprites - index as u32 * input.split).min(input.split);
            if bank.stamp != input.epoch.stamp() {
                return Err(format!("stale bank {index}"));
            }
            if bank.format != Format::Rgba8Unorm {
                return Err(format!("bank {index}: requires linear RGBA8Unorm"));
            }
            if bank.layers != used
                || bank.allocated_layers < used
                || bank.allocated_layers > limits.max_array_layers
            {
                return Err(format!(
                    "bank {index}: invalid used/allocated layers or device policy"
                ));
            }
            if bank.size > limits.max_dimension
                || bank.size != first.size
                || bank.levels != first.levels
            {
                return Err(format!(
                    "bank {index}: inconsistent dimensions/mips or device policy"
                ));
            }
            let bytes = chain_bytes(bank.size, bank.levels, bank.allocated_layers)?;
            if bank.pixels.len() != bytes {
                return Err(format!(
                    "bank {index}: missing mip pixels or invalid byte length"
                ));
            }
            payload = payload
                .checked_add(bytes)
                .ok_or("material payload overflow")?;
        }
        if input.lightmap.stamp != input.epoch.stamp() {
            return Err("stale lightmap".into());
        }
        if input.lightmap.pixels.chunks_exact(4).any(|p| p[3] != 255) {
            return Err("uninitialized/nonopaque lightmap".into());
        }
        if payload > limits.payload_bytes {
            return Err("material payload budget exceeded".into());
        }
        Ok(Self {
            input,
            payload_bytes: payload,
        })
    }
    /// Exact production partition, not wrapping an invalid ID into a valid layer.
    pub fn sprite(&self, global: u16) -> Result<(usize, u32), String> {
        let id = u32::from(global);
        if id >= self.input.sprites {
            return Err(format!("missing global sprite {id}"));
        }
        let bank = id / self.input.split;
        Ok((bank as usize, id - bank * self.input.split))
    }
    pub fn bank(&self, index: usize) -> Option<Bank<'a>> {
        self.input.banks.get(index).copied()
    }
    pub fn check_lightmap(&self, stamp: Stamp, revision: u64) -> Result<(), String> {
        self.check_stamp(stamp)?;
        if revision != self.input.lightmap.revision {
            return Err("stale lightmap revision".into());
        }
        Ok(())
    }
    pub fn lightmap(&self) -> Lightmap<'a> {
        self.input.lightmap
    }
    /// Cached mesh/upload tickets must carry this stamp. Old tickets fail even if
    /// the allocator reuses storage. Device recreation requires a new Epoch.
    pub fn check_stamp(&self, stamp: Stamp) -> Result<(), String> {
        if stamp != self.input.epoch.stamp() {
            Err("stale material ticket".into())
        } else {
            Ok(())
        }
    }
    pub fn validate_section(
        &self,
        mesh: &section::Prepared<'_, '_>,
        stamp: Stamp,
    ) -> Result<(), String> {
        self.check_stamp(stamp)?;
        // Prepared exposes summary fields publicly; inspect the borrowed bytes,
        // not a caller-mutable high-water mark. Do this once per captured section.
        if let Some(mesh) = mesh.mesh() {
            for index in 0..mesh.vertices.len() / crate::terrain::STRIDE {
                self.sprite(mesh.vertex(index)?.layer & !crate::terrain::OPAQUE_LAYER)?;
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests;
