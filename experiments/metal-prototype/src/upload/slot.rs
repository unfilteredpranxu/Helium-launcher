//! One process-wide native upload/draw permit, retained through terminal cleanup.
//! No GPU objects, waits, polling loops or receipt publication in this module.
use super::Ticket;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// One observation per call. Idle/repeated polling never reissues a receipt.
pub enum Poll<G> {
    Idle,
    Pending,
    Completed(Ticket<G>),
    Failed(String),
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Retention {
    pub busy: bool,
    pub texture: usize,
    pub staging: usize,
    /// Other GPU frame payload, excluding material texture/staging/source.
    pub frame_payload: usize,
    pub sources: usize,
}
pub(crate) struct Slot {
    busy: AtomicBool,
    texture: AtomicUsize,
    staging: AtomicUsize,
    frame_payload: AtomicUsize,
    sources: AtomicUsize,
}
pub(crate) struct Permit {
    slot: Arc<Slot>,
}
impl Slot {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            busy: AtomicBool::new(false),
            texture: AtomicUsize::new(0),
            staging: AtomicUsize::new(0),
            frame_payload: AtomicUsize::new(0),
            sources: AtomicUsize::new(0),
        })
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub fn global() -> Arc<Self> {
        static SLOT: std::sync::OnceLock<Arc<Slot>> = std::sync::OnceLock::new();
        SLOT.get_or_init(Self::new).clone()
    }
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }
    pub fn acquire(self: &Arc<Self>) -> Result<Permit, String> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "native GPU slot busy (pending upload/draw or terminal cleanup)")?;
        Ok(Permit { slot: self.clone() })
    }
    /// Diagnostic gauges, not a transactional cross-thread memory snapshot.
    pub fn retention(&self) -> Retention {
        Retention {
            busy: self.busy(),
            texture: self.texture.load(Ordering::Relaxed),
            staging: self.staging.load(Ordering::Relaxed),
            frame_payload: self.frame_payload.load(Ordering::Relaxed),
            sources: self.sources.load(Ordering::Relaxed),
        }
    }
}
impl Permit {
    #[cfg(any(target_os = "macos", target_os = "ios", test))]
    pub fn account_frame(&self, bytes: usize) {
        self.slot.frame_payload.store(bytes, Ordering::Relaxed);
    }
    pub fn account(&self, texture: usize, staging: usize, sources: usize) {
        self.slot.texture.store(texture, Ordering::Relaxed);
        self.slot.staging.store(staging, Ordering::Relaxed);
        self.slot.sources.store(sources, Ordering::Relaxed);
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.slot.texture.store(0, Ordering::Relaxed);
        self.slot.staging.store(0, Ordering::Relaxed);
        self.slot.frame_payload.store(0, Ordering::Relaxed);
        self.slot.sources.store(0, Ordering::Relaxed);
        self.slot.busy.store(false, Ordering::Release);
    }
}
/// Same owned CPU lease used by native completion keepalive and protocol tests.
/// A callback can own this only when S is Send + Sync + 'static.
pub(crate) struct Ownership<S> {
    pub source: Arc<S>,
    pub previous: Option<Arc<S>>,
    pub _permit: Permit,
}
#[cfg(test)]
mod tests;
