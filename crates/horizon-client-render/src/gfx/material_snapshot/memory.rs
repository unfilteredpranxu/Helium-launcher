//! Shared accounting includes pinned historical banks, not just the latest view.
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
pub(super) struct Budget {
    limit: usize,
    live: AtomicUsize,
    peak: AtomicUsize,
}
pub(super) struct Charge {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Budget {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            live: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        })
    }
    pub fn charge(self: &Arc<Self>, bytes: usize) -> Result<Charge, &'static str> {
        let old = self
            .live
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_add(bytes).filter(|&n| n <= self.limit)
            })
            .map_err(|_| "refresh memory budget exceeded")?;
        self.peak.fetch_max(old + bytes, Ordering::Relaxed);
        Ok(Charge {
            budget: self.clone(),
            bytes,
        })
    }
    pub fn live(&self) -> usize {
        self.live.load(Ordering::Relaxed)
    }
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }
}
impl Charge {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.budget.live.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
pub(super) struct Payload {
    pub bytes: Vec<u8>,
    _charge: Charge,
}
impl Payload {
    pub fn zeroed(budget: &Arc<Budget>, len: usize) -> Result<Self, &'static str> {
        let mut charge = budget.charge(len)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| "refresh allocation failed")?;
        // Account actual allocator capacity before initialization/copying.
        let mut extra = budget.charge(bytes.capacity() - len)?;
        charge.bytes += extra.bytes;
        extra.bytes = 0;
        bytes.resize(len, 0);
        Ok(Self {
            bytes,
            _charge: charge,
        })
    }
}
