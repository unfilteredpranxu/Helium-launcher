//! G05 v1 single-bank upload boundary. CPU validation is not GPU completion.
//! MIT; Copyright (c) 2026 Horizon contributors.
use crate::{
    material,
    terrain::{self, Capabilities},
};
use std::{
    fmt::Debug,
    sync::atomic::{AtomicU64, Ordering},
};
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
mod slot;
#[cfg(any(target_os = "macos", target_os = "ios", test))]
pub(crate) use slot::{Ownership, Slot};
pub use slot::{Poll, Retention};
pub const VERSION: u32 = 1;
/// Additive nonblocking API; source descriptions and receipt identity stay v1.
pub const NONBLOCKING_VERSION: u32 = 1;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use crate::terrain::metal::material_upload::Material;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    LayerMajorMipMajor,
    Other,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity<G> {
    pub generation: G,
    pub revision: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Description<G> {
    pub version: u32,
    pub identity: Identity<G>,
    pub bank: u32,
    pub split: u32,
    pub sprites: u32,
    pub used: u32,
    pub allocated: u32,
    pub size: u32,
    pub levels: u32,
    pub format: material::Format,
    pub layout: Layout,
}
impl<G> Description<G> {
    pub fn slice(&self, global: u32) -> Result<u32, String> {
        if self.split == 0
            || self.split > 2048
            || self.used == 0
            || self.used > self.split
            || self.sprites == 0
            || self.sprites > 32768
            || self.bank >= self.sprites.div_ceil(self.split)
            || self.sprites.div_ceil(self.split) > 8
        {
            return Err("invalid bank namespace for sprite mapping".into());
        }
        let first = self
            .bank
            .checked_mul(self.split)
            .ok_or("sprite range overflow")?;
        let local = global.checked_sub(first).ok_or("sprite precedes bank")?;
        if global >= self.sprites || local >= self.used {
            return Err("sprite outside bank's used range".into());
        }
        Ok(local)
    }
}
/// Implementations must own their immutable bytes and check the CURRENT producer,
/// not a retained historical owner. Authority remains borrowed through completion.
pub trait Source {
    type Generation: Copy + Eq + Debug;
    type Authority: ?Sized;
    fn description(&self) -> Description<Self::Generation>;
    fn pixels(&self) -> &[u8];
    /// All retained source payload (possibly the entire production snapshot).
    fn retained_bytes(&self) -> usize;
    fn check_current(&self, authority: &Self::Authority) -> Result<(), String>;
}
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub payload_bytes: usize,
    pub staging_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Update {
    Full,
    Layers { first: u32, count: u32 },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyRegion {
    pub slice: usize,
    pub level: usize,
    pub width: usize,
    pub source: usize,
    pub staging: usize,
    pub row: usize,
    pub image: usize,
}
impl CopyRegion {
    pub fn check(&self, source_len: usize, staging_len: usize) -> Result<(), String> {
        let row_bytes = terrain::product(&[self.width, 4])?;
        if self.width == 0
            || self.row < row_bytes
            || !self.row.is_multiple_of(256)
            || !self.staging.is_multiple_of(256)
            || self.image != terrain::product(&[self.row, self.width])?
        {
            return Err("invalid row/image pitch or staging alignment".into());
        }
        let end =
            |offset: usize, length: usize| offset.checked_add(length).ok_or("copy range overflow");
        if end(self.source, terrain::product(&[row_bytes, self.width])?)? > source_len
            || end(self.staging, self.image)? > staging_len
        {
            return Err("copy outside source/staging allocation".into());
        }
        Ok(())
    }
}
pub struct Plan<G> {
    description: Description<G>,
    regions: Vec<CopyRegion>,
    staging_bytes: usize,
    texture_bytes: usize,
    upload_bytes: usize,
    peak_payload: usize,
    source_bytes: usize,
}
impl<G: Copy + Eq + Debug> Plan<G> {
    pub fn description(&self) -> Description<G> {
        self.description
    }
    pub fn staging_bytes(&self) -> usize {
        self.staging_bytes
    }
    pub fn texture_bytes(&self) -> usize {
        self.texture_bytes
    }
    pub fn upload_bytes(&self) -> usize {
        self.upload_bytes
    }
    pub fn source_bytes(&self) -> usize {
        self.source_bytes
    }
    pub fn peak_payload(&self) -> usize {
        self.peak_payload
    }
    pub fn regions(&self) -> &[CopyRegion] {
        &self.regions
    }
    pub fn prepare<S: Source<Generation = G>>(
        source: &S,
        authority: &S::Authority,
        previous: Option<&S>,
        update: Update,
        caps: &Capabilities,
        budget: Budget,
    ) -> Result<Self, String> {
        source.check_current(authority)?;
        let d = source.description();
        if d.version != VERSION
            || d.format != material::Format::Rgba8Unorm
            || d.layout != Layout::LayerMajorMipMajor
        {
            return Err("unsupported upload version, RGBA format or mip layout".into());
        }
        if !caps.baseline() || caps.sample_counts & 2 == 0 {
            return Err("unverified Metal capability baseline".into());
        }
        if d.size == 0
            || d.size as usize > terrain::TEXTURE_SIDE_CEILING
            || d.allocated == 0
            || d.allocated as usize > terrain::TEXTURE_LAYER_CEILING
        {
            return Err("bank exceeds unchanged G02 4096-side/256-slice policy".into());
        }
        if d.split == 0
            || d.split > 2048
            || d.sprites == 0
            || d.sprites > 32768
            || d.bank >= d.sprites.div_ceil(d.split)
            || d.sprites.div_ceil(d.split) > 8
        {
            return Err("invalid production bank namespace".into());
        }
        let first_global = d.bank.checked_mul(d.split).ok_or("bank range overflow")?;
        if d.used != (d.sprites - first_global).min(d.split) || d.allocated < d.used {
            return Err("used/allocated slice metadata mismatch".into());
        }
        if !d.size.is_power_of_two() || d.levels != d.size.ilog2() + 1 {
            return Err("incomplete production mip chain".into());
        }
        let stride = material::chain_bytes(d.size, d.levels, 1)?;
        let texture_bytes = material::chain_bytes(d.size, d.levels, d.allocated)?;
        let retained = source.retained_bytes();
        if source.pixels().len() != texture_bytes || retained < texture_bytes {
            return Err("missing bank pixels or retained ownership accounting".into());
        }
        let (first, count) = match update {
            Update::Full => (0, d.allocated),
            Update::Layers { first, count } => {
                if count == 0 || first.checked_add(count).ok_or("layer range overflow")? > d.used {
                    return Err("partial update outside used layers".into());
                }
                let old = previous.ok_or("partial update requires completed base residency")?;
                let mut old_d = old.description();
                old_d.identity.revision = d.identity.revision;
                if old_d != d || old.description().identity.revision >= d.identity.revision {
                    return Err("incompatible/stale partial update base".into());
                }
                let start = terrain::product(&[first as usize, stride])?;
                let end = terrain::product(&[(first + count) as usize, stride])?;
                // Last-frame dirty IDs alone cannot prove all skipped revisions.
                if old.pixels().len() != texture_bytes
                    || old.pixels()[..start] != source.pixels()[..start]
                    || old.pixels()[end..] != source.pixels()[end..]
                {
                    return Err(
                        "partial update omits changed layers/mips; full upload required".into(),
                    );
                }
                (first, count)
            }
        };
        let regions_len = terrain::product(&[count as usize, d.levels as usize])?;
        let metadata_bytes = terrain::product(&[regions_len, size_of::<CopyRegion>()])?;
        // Preflight sizes before allocating even the bounded region list.
        let mut staging = 0usize;
        for level in 0..d.levels {
            let width = (d.size >> level) as usize;
            let row = terrain::aligned_row(width)?;
            staging = staging
                .checked_add(terrain::product(&[row, width, count as usize])?)
                .ok_or("staging overflow")?;
        }
        if staging > budget.staging_bytes
            || staging > caps.max_buffer_bytes
            || staging > isize::MAX as usize
        {
            return Err("staging budget/device buffer limit exceeded".into());
        }
        let source_bytes = retained
            .checked_add(previous.map_or(0, Source::retained_bytes))
            .ok_or("retained source overflow")?;
        let peak_payload = [
            texture_bytes,
            staging,
            staging,
            metadata_bytes,
            source_bytes,
        ]
        .into_iter()
        .try_fold(0usize, |a, b| {
            a.checked_add(b).ok_or("upload payload overflow")
        })?;
        if peak_payload > budget.payload_bytes {
            return Err(
                "upload payload budget exceeded (includes historical source and staging copies)"
                    .into(),
            );
        }
        let mut regions = Vec::new();
        regions
            .try_reserve_exact(regions_len)
            .map_err(|_| "region allocation failed")?;
        if regions.capacity() != regions_len {
            return Err("unexpected region capacity".into());
        }
        let mut offset = 0usize;
        for slice in first..first + count {
            let mut source_offset = terrain::product(&[slice as usize, stride])?;
            for level in 0..d.levels {
                let width = (d.size >> level) as usize;
                let row = terrain::aligned_row(width)?;
                let image = terrain::product(&[row, width])?;
                let region = CopyRegion {
                    slice: slice as usize,
                    level: level as usize,
                    width,
                    source: source_offset,
                    staging: offset,
                    row,
                    image,
                };
                region.check(source.pixels().len(), staging)?;
                regions.push(region);
                offset = offset.checked_add(image).ok_or("staging offset overflow")?;
                source_offset = source_offset
                    .checked_add(terrain::product(&[width, width, 4])?)
                    .ok_or("source mip offset overflow")?;
            }
        }
        Ok(Self {
            description: d,
            source_bytes,
            regions,
            staging_bytes: staging,
            texture_bytes,
            upload_bytes: terrain::product(&[count as usize, stride])?,
            peak_payload,
        })
    }
    pub fn stage(&self, source: &[u8]) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.staging_bytes)
            .map_err(|_| "staging allocation failed")?;
        if bytes.capacity() != self.staging_bytes {
            return Err("unexpected staging capacity".into());
        }
        bytes.resize(self.staging_bytes, 0);
        for r in &self.regions {
            r.check(source.len(), bytes.len())?;
            for y in 0..r.width {
                let from = r.source + y * r.width * 4;
                let to = r.staging + y * r.row;
                bytes[to..to + r.width * 4].copy_from_slice(&source[from..from + r.width * 4]);
            }
        }
        Ok(bytes)
    }
}

#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
static NEXT: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
pub(crate) struct Id(u64);
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
impl Id {
    pub(crate) fn new() -> Result<Self, String> {
        NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .map(Self)
            .map_err(|_| "GPU identity exhausted".into())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Prepared,
    Encoded,
    Submitted,
    Pending,
    Completed,
    Failed,
    Invalidated,
}
/// Non-cloneable, one-use completion receipt. No public state transitions.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
pub struct Ticket<G> {
    identity: Identity<G>,
    device: Id,
    owner: Id,
    operation: Id,
    phase: Phase,
    consumed: bool,
}
impl<G: Copy> Ticket<G> {
    pub fn identity(&self) -> Identity<G> {
        self.identity
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
}
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
/// Private reusable proof of an exact completed upload, never a public success flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
pub(crate) struct Stamp<G> {
    pub identity: Identity<G>,
    pub device: Id,
    pub owner: Id,
    operation: Id,
}
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
pub(crate) struct Record<G> {
    ticket: Ticket<G>,
}
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
impl<G: Copy + Eq> Record<G> {
    pub(crate) fn residency(&self) -> Result<Stamp<G>, String> {
        if self.phase() != Phase::Completed {
            return Err("no successful completed residency".into());
        }
        Ok(Stamp {
            identity: self.ticket.identity,
            device: self.ticket.device,
            owner: self.ticket.owner,
            operation: self.ticket.operation,
        })
    }
    pub(crate) fn prepared(identity: Identity<G>, device: Id, owner: Id) -> Result<Self, String> {
        Ok(Self {
            ticket: Ticket {
                identity,
                device,
                owner,
                operation: Id::new()?,
                phase: Phase::Prepared,
                consumed: false,
            },
        })
    }
    pub(crate) fn advance(&mut self, from: Phase, to: Phase) -> Result<(), String> {
        if self.ticket.phase != from
            || !matches!(
                (from, to),
                (Phase::Prepared, Phase::Encoded) | (Phase::Encoded, Phase::Submitted)
            )
        {
            return Err("invalid upload transition".into());
        }
        self.ticket.phase = to;
        Ok(())
    }
    pub(crate) fn finish(
        &mut self,
        successful_metal_completion: bool,
    ) -> Result<Ticket<G>, String> {
        if !matches!(self.ticket.phase, Phase::Submitted | Phase::Pending) {
            return Err("upload was not submitted".into());
        }
        self.ticket.phase = if successful_metal_completion {
            Phase::Completed
        } else {
            Phase::Failed
        };
        if !successful_metal_completion {
            return Err("Metal command did not complete successfully".into());
        }
        Ok(Ticket {
            consumed: false,
            ..self.ticket
        })
    }
    pub(crate) fn observe(&mut self, terminal: Option<bool>) -> Result<Option<Ticket<G>>, String> {
        if !matches!(self.phase(), Phase::Submitted | Phase::Pending) {
            return Err("no submitted upload to observe".into());
        }
        match terminal {
            None => {
                self.ticket.phase = Phase::Pending;
                Ok(None)
            }
            Some(success) => self.finish(success).map(Some),
        }
    }
    pub(crate) fn invalidate(&mut self) {
        self.ticket.phase = Phase::Invalidated;
    }
    pub(crate) fn fail(&mut self) {
        self.ticket.phase = Phase::Failed;
    }
    pub(crate) fn phase(&self) -> Phase {
        self.ticket.phase
    }
    pub(crate) fn consume(
        &self,
        ticket: &mut Ticket<G>,
        identity: Identity<G>,
        device: Id,
        owner: Id,
    ) -> Result<(), String> {
        if ticket.consumed
            || ticket.phase != Phase::Completed
            || self.phase() != Phase::Completed
            || ticket.operation != self.ticket.operation
            || self.ticket.device != device
            || self.ticket.owner != owner
            || self.ticket.identity != identity
            || ticket.device != device
            || ticket.owner != owner
            || ticket.identity != identity
        {
            return Err("stale/consumed/wrong-owner upload receipt".into());
        }
        ticket.consumed = true;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub upload_bytes: u64,
    pub texture_payload: usize,
    pub staging_peak: usize,
    pub source_peak: usize,
    pub reused_resources: u64,
    pub pending: usize,
    pub failed_allocations: u64,
    pub completed_uploads: u64,
    pub submitted_operations: u64,
    pub failed_operations: u64,
    pub invalidations: u64,
    pub budget_rejections: u64,
    pub pending_polls: u64,
    pub staging_capacity: usize,
    pub source_retained: usize,
}
impl Counters {
    #[cfg_attr(
        not(any(target_os = "macos", target_os = "ios", test)),
        allow(dead_code)
    )]
    pub(crate) fn committed(&mut self, bytes: usize) {
        if self.submitted_operations != 0 {
            self.reused_resources = self.reused_resources.saturating_add(1);
        }
        self.submitted_operations = self.submitted_operations.saturating_add(1);
        self.upload_bytes = self.upload_bytes.saturating_add(bytes as u64);
    }
}
#[cfg(test)]
pub(crate) mod tests;

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod async_native_tests;
