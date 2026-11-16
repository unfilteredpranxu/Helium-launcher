//! Optional observation of production uploads. No rendering or mip generation here.
//! MIT; Copyright (c) 2026 Horizon contributors.
use super::PreparedTextures;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
mod memory;
pub(crate) mod refresh;
use memory::{Budget, Charge, Payload};
pub use refresh::{Counters, Phase, Ticket};
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Generation(u64);
impl Generation {
    /// Numeric diagnostics only; this does not construct or authorize a generation.
    pub fn diagnostic_id(self) -> u64 {
        self.0
    }
}
impl Generation {
    fn new() -> Self {
        Self(
            NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
                .expect("material generation exhausted"),
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8Unorm,
}
/// Immutable CPU target content. State::current checks generation AND revision;
/// it makes no GPU completion claim. Arc clones retain historical data safely.
/// Each bank is layer-major, with full mip chains in descending size per layer.
pub struct Snapshot {
    generation: Generation,
    split: u32,
    sprites: u32,
    size: u32,
    banks: Vec<Bank>,
    revision: u64,
    _metadata: Charge,
    retained_bytes: usize,
}
#[derive(Clone)]
pub struct Bank {
    used: u32,
    allocated: u32,
    levels: u32,
    pixels: Arc<Payload>,
}
impl Bank {
    pub fn used(&self) -> u32 {
        self.used
    }
    pub fn allocated(&self) -> u32 {
        self.allocated
    }
    pub fn levels(&self) -> u32 {
        self.levels
    }
    pub fn pixels(&self) -> &[u8] {
        &self.pixels.bytes
    }
}
impl Snapshot {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn format(&self) -> PixelFormat {
        PixelFormat::Rgba8Unorm
    }
    pub fn split(&self) -> u32 {
        self.split
    }
    pub fn sprites(&self) -> u32 {
        self.sprites
    }
    pub fn size(&self) -> u32 {
        self.size
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn generation(&self) -> Generation {
        self.generation
    }
    pub fn banks(&self) -> &[Bank] {
        &self.banks
    }
}
/// Renderer-owned lifecycle. No texture retention unless explicitly requested.
/// Mutating this state requires exclusive renderer access; no frame-path locks.
pub struct State {
    generation: Generation,
    snapshot: Option<Arc<Snapshot>>,
    unavailable: &'static str,
    light_revision: u64,
    animation_revision: u64,
    light_ready: bool,
    budget: Option<Arc<Budget>>,
    refresh_limit: usize,
    frames: Option<refresh::Frames>,
    counters: Counters,
    phase: Phase,
}
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}
impl State {
    pub fn new() -> Self {
        Self {
            generation: Generation::new(),
            snapshot: None,
            unavailable: "snapshot retention disabled",
            light_revision: 0,
            animation_revision: 0,
            light_ready: false,
            budget: None,
            refresh_limit: 0,
            frames: None,
            counters: Counters::default(),
            phase: Phase::Unavailable,
        }
    }
    pub fn generation(&self) -> Generation {
        self.generation
    }
    pub fn snapshot(&self) -> Result<&Arc<Snapshot>, &'static str> {
        self.snapshot.as_ref().ok_or(self.unavailable)
    }
    /// A current CPU target view borrows the producer as well as the retained bytes.
    /// ```compile_fail
    /// use horizon_client_render::gfx::material_snapshot::{State, Snapshot};
    /// fn invalid(state: &mut State, old: &Snapshot) {
    ///     let current = state.current(old).unwrap();
    ///     state.release();
    ///     let _ = current.banks();
    /// }
    /// ```
    pub fn current<'a>(&'a self, snapshot: &'a Snapshot) -> Result<&'a Snapshot, &'static str> {
        if snapshot.generation != self.generation {
            return Err("obsolete renderer/resource generation");
        }
        self.snapshot()?;
        if snapshot.revision != self.animation_revision {
            return Err("superseded material revision");
        }
        Ok(snapshot)
    }
    pub fn animation_revision(&self) -> u64 {
        self.animation_revision
    }
    pub fn light_revision(&self) -> Result<u64, &'static str> {
        if self.light_ready {
            Ok(self.light_revision)
        } else {
            Err("lightmap not generated")
        }
    }
    pub(super) fn observe_lightmap(&mut self, old: &[u8; 1024], new: &[u8; 1024]) {
        if self.light_ready && old == new {
            return;
        }
        self.light_revision = self
            .light_revision
            .checked_add(1)
            .expect("light revision exhausted");
        self.light_ready = true;
    }
    pub(super) fn animated(&mut self, changed: &[u32]) {
        if !changed.is_empty() {
            self.animation_revision = self
                .animation_revision
                .checked_add(1)
                .expect("animation revision exhausted");
            if self.snapshot.is_some() {
                self.fail("CPU snapshot stale after animation upload; refresh required");
            }
        }
    }
    pub fn release(&mut self) {
        self.fail("snapshot released");
        self.phase = Phase::Released;
    }
}

pub(in crate::gfx) fn chain(size: u32, levels: u32) -> Result<usize, &'static str> {
    if !size.is_power_of_two() || levels != size.ilog2() + 1 {
        return Err("invalid dimensions or incomplete mip chain");
    }
    (0..levels).try_fold(0usize, |sum, level| {
        let edge = (size >> level) as usize;
        sum.checked_add(
            edge.checked_mul(edge)
                .and_then(|n| n.checked_mul(4))
                .ok_or("mip size overflow")?,
        )
        .ok_or("chain size overflow")
    })
}
impl PreparedTextures {
    /// Explicit opt-in at construction. Zero disables retention. Budget covers
    /// bank pixel capacities and bank records; ordinary temporary upload data is
    /// separate. Rejection disables observation, never changes the wgpu upload.
    pub fn retain_material_snapshot(mut self, budget: usize) -> Self {
        self.snapshot_budget = budget;
        self
    }

    /// Observe the current prepared layout without exposing mutable chains.
    /// Before Renderer::new this is a prospective layout, NOT a device ticket.
    /// Renderer::new calls this after its actual device-selected resplit. Each
    /// observation has a unique generation; only its owning renderer can validate it.
    pub fn capture_materials(&self, size: u32, layers: u32, gl: bool) -> State {
        let mut state = State::new();
        if self.snapshot_budget == 0 {
            return state;
        }
        if size != self.snapshot_size || layers != self.snapshot_layers {
            state.unavailable = "prepared source dimensions/count disagree with upload";
            return state;
        }
        let budget = Budget::new(self.snapshot_budget);
        state.budget = Some(budget.clone());
        state.refresh_limit = self.snapshot_budget;
        match self.capture_checked(size, layers, gl, state.generation, &budget) {
            Ok(snapshot) => {
                state.snapshot = Some(Arc::new(snapshot));
                state.phase = Phase::InitialCpu;
                state.counters.copied_bytes = state
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .banks
                    .iter()
                    .map(|b| b.pixels.bytes.len() as u64)
                    .sum();
            }
            Err(error) => {
                if error.contains("budget") {
                    state.counters.failed_budget_checks += 1;
                }
                state.unavailable = error;
            }
        }
        state
    }
    fn capture_checked(
        &self,
        size: u32,
        layers: u32,
        gl: bool,
        generation: Generation,
        budget: &Arc<Budget>,
    ) -> Result<Snapshot, &'static str> {
        if self.pbr.is_some() {
            return Err("PBR snapshot unsupported");
        }
        if layers == 0
            || layers > 32768
            || self.split == 0
            || self.split > super::SPRITE_SPLIT
            || self.sprites.len() != layers.div_ceil(self.split) as usize
            || self.sprites.len() > 8
        {
            return Err("invalid sprite count/split/banks");
        }
        let mut bytes = self
            .sprites
            .len()
            .checked_mul(size_of::<Bank>())
            .ok_or("metadata overflow")?;
        for (index, (pixels, levels)) in self.sprites.iter().enumerate() {
            let used = (layers - index as u32 * self.split).min(self.split);
            let stride = chain(size, *levels)?;
            if pixels.len()
                != stride
                    .checked_mul(used as usize)
                    .ok_or("bank size overflow")?
            {
                return Err("corrupt mip payload");
            }
            bytes = bytes
                .checked_add(
                    stride
                        .checked_mul(super::array_layers(used, gl) as usize)
                        .ok_or("padded bank overflow")?,
                )
                .ok_or("payload overflow")?;
        }
        if bytes > self.snapshot_budget || bytes > isize::MAX as usize {
            return Err("snapshot budget exceeded");
        }
        let metadata = budget.charge(self.sprites.len() * size_of::<Bank>())?;
        let mut banks = Vec::new();
        banks
            .try_reserve_exact(self.sprites.len())
            .map_err(|_| "bank metadata allocation failed")?;
        if banks.capacity() != self.sprites.len() {
            return Err("unexpected metadata capacity");
        }
        let mut retained = banks
            .capacity()
            .checked_mul(size_of::<Bank>())
            .ok_or("bank metadata overflow")?;
        for (index, (pixels, levels)) in self.sprites.iter().enumerate() {
            let used = (layers - index as u32 * self.split).min(self.split);
            let allocated = super::array_layers(used, gl);
            let length = chain(size, *levels)?
                .checked_mul(allocated as usize)
                .ok_or("bank size overflow")?;
            let mut copy = Payload::zeroed(budget, length)?;
            retained = retained
                .checked_add(copy.bytes.capacity())
                .ok_or("retained size overflow")?;
            copy.bytes[..pixels.len()].copy_from_slice(pixels);
            banks.push(Bank {
                used,
                allocated,
                levels: *levels,
                pixels: Arc::new(copy),
            });
        }
        Ok(Snapshot {
            generation,
            split: self.split,
            sprites: layers,
            size,
            banks,
            retained_bytes: retained,
            revision: 0,
            _metadata: metadata,
        })
    }
}
#[cfg(test)]
mod tests;
