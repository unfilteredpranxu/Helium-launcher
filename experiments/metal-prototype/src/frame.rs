//! G05-C v1: one resident frame, genuine completion, explicit diagnostic readback.
//! MIT; Copyright (c) 2026 Horizon contributors.
use crate::{
    terrain::{self, Capabilities, Mesh, Options, View},
    upload::{Description, Id, Phase, Record, Stamp, Ticket},
};

pub const VERSION: u32 = 1;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use crate::terrain::metal::frames::{CompletedFrame, Frames};
/// Distinct, non-cloneable, one-use frame completion receipt (not an upload ticket).
pub struct Receipt<G>(pub(crate) Ticket<G>);
pub enum Poll<G> {
    Idle,
    Pending,
    Completed(Receipt<G>),
    Failed(String),
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub submitted: u64,
    pub pending: usize,
    pub completed: u64,
    pub failed: u64,
    pub invalidated: u64,
    pub busy_rejections: u64,
    pub pending_polls: u64,
    /// Accounted live payload, including retained source and resident texture.
    /// Not physical Metal allocator usage or process RSS.
    pub allocated_payload: usize,
    pub peak_payload: usize,
    pub reused_targets: u64,
    pub diagnostic_readback_bytes: u64,
}
/// No readback/CPU image allocation is included in normal frame preparation.
/// Conservative peak includes both CPU and Metal lightmap staging (4096 each).
#[allow(clippy::too_many_arguments)]
pub fn validate<G: Copy + Eq + std::fmt::Debug>(
    mesh: &Mesh<'_>,
    description: Description<G>,
    texture_bytes: usize,
    source_bytes: usize,
    lightmap: &[u8],
    view: View,
    options: Options,
    caps: &Capabilities,
) -> Result<terrain::ResourceLayout, String> {
    if description.bank != 0
        || description.format != crate::material::Format::Rgba8Unorm
        || description.layout != crate::upload::Layout::LayerMajorMipMajor
        || description.version != crate::upload::VERSION
    {
        return Err("unsupported resident shader/bank/format/layout".into());
    }
    // Revalidate the namespace even though native callers require a completed plan.
    description.slice(
        description
            .used
            .checked_sub(1)
            .ok_or("empty resident bank")?,
    )?;
    if description.used != description.sprites.min(description.split)
        || source_bytes < texture_bytes
        || description.used > description.allocated
        || description.allocated as usize > terrain::TEXTURE_LAYER_CEILING
        || description.size == 0
        || !description.size.is_power_of_two()
        || description.levels != description.size.ilog2() + 1
    {
        return Err("invalid resident dimensions/mips/slices".into());
    }
    let chain = (0..description.levels).try_fold(0usize, |sum, level| {
        let side = (description.size >> level) as usize;
        sum.checked_add(terrain::product(&[side, side, 4])?)
            .ok_or_else(|| "mip size overflow".to_string())
    })?;
    if terrain::product(&[chain, description.allocated as usize])? != texture_bytes {
        return Err("resident mip payload mismatch".into());
    }
    let mut layout = terrain::resource_layout_mode(
        mesh,
        (
            description.size as usize,
            description.size as usize,
            description.used as usize,
        ),
        texture_bytes,
        false,
        false,
        lightmap,
        view,
        options,
        caps,
    )?;
    layout.payload_bytes = layout
        .payload_bytes
        .checked_add(source_bytes)
        .ok_or("frame source payload overflow")?;
    if layout.payload_bytes > options.budget.payload_bytes {
        return Err("frame source/payload budget exceeded".into());
    }
    Ok(layout)
}

// This exact protocol is also exercised on Linux using explicitly simulated GPU
// events. Only the native backend calls observe from real command status/error.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
pub(crate) struct Protocol<G> {
    record: Record<G>,
    resident: Stamp<G>,
    revoked: Option<String>,
}
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
impl<G: Copy + Eq> Protocol<G> {
    pub fn prepare(resident: Stamp<G>, device: Id, owner: Id) -> Result<Self, String> {
        if resident.device != device {
            return Err("wrong frame device".into());
        }
        Ok(Self {
            record: Record::prepared(resident.identity, device, owner)?,
            resident,
            revoked: None,
        })
    }
    pub fn encoded(&mut self) -> Result<(), String> {
        self.record.advance(Phase::Prepared, Phase::Encoded)
    }
    pub fn submitted(&mut self) -> Result<(), String> {
        self.record.advance(Phase::Encoded, Phase::Submitted)
    }
    pub fn phase(&self) -> Phase {
        self.record.phase()
    }
    pub fn invalidate(&mut self, reason: String) {
        if self.revoked.is_none() {
            self.revoked = Some(reason);
            self.record.invalidate();
        }
    }
    pub fn check(&self, resident: Stamp<G>, device: Id) -> Result<(), String> {
        if self.revoked.is_some() || self.resident != resident || resident.device != device {
            return Err("released/stale/recreated frame or material residency".into());
        }
        Ok(())
    }
    pub fn observe(&mut self, terminal: Option<bool>) -> Poll<G> {
        if !matches!(
            self.phase(),
            Phase::Submitted | Phase::Pending | Phase::Invalidated
        ) {
            return Poll::Idle;
        }
        // Logical invalidation never declares a nonterminal command retired.
        if let Some(reason) = &self.revoked {
            return if terminal.is_none() {
                Poll::Pending
            } else {
                Poll::Failed(reason.clone())
            };
        }
        match self.record.observe(terminal) {
            Ok(Some(ticket)) => Poll::Completed(Receipt(ticket)),
            Ok(None) => Poll::Pending,
            Err(e) => Poll::Failed(e),
        }
    }
    pub fn consume(
        &self,
        receipt: &mut Receipt<G>,
        resident: Stamp<G>,
        device: Id,
        owner: Id,
    ) -> Result<(), String> {
        self.check(resident, device)?;
        self.record
            .consume(&mut receipt.0, resident.identity, device, owner)
    }
}
#[cfg(test)]
mod tests;

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod native_tests;
