//! CPU-only handoff gate. Tests simulate host events, never Metal completion.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
pub(crate) struct Gate {
    state: AtomicU8,
    cancel: Arc<AtomicBool>,
}
impl Gate {
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            state: AtomicU8::new(1),
            cancel,
        }
    }
    pub fn start(&self) -> bool {
        !self.cancelled()
            && self
                .state
                .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    /// Called only after the owned worker returns. Native worker success itself
    /// requires exact upload/draw/present command completion, not this CPU gate.
    pub fn finish(&self, success: bool) {
        let _ = self.state.compare_exchange(
            2,
            if success { 3 } else { 4 },
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    pub fn publish(&self) -> bool {
        !self.cancelled()
            && self
                .state
                .compare_exchange(3, 5, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    pub fn published(&self) -> bool {
        !self.cancelled() && self.state.load(Ordering::Acquire) == 5
    }
    pub fn running(&self) -> bool {
        self.state.load(Ordering::Acquire) == 2
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn gate() -> Gate {
        Gate::new(Arc::new(AtomicBool::new(false)))
    }
    #[test]
    fn simulated_submission_has_no_early_or_double_publication() {
        let g = gate();
        assert!(!g.publish());
        assert!(!g.published());
        assert!(g.start());
        assert!(!g.start());
        assert!(!g.publish());
        g.finish(true);
        assert!(!g.published());
        assert!(g.publish());
        assert!(g.published());
        assert!(!g.publish());
        assert!(!g.start());
        g.finish(false);
        assert!(g.published());
    }
    #[test]
    fn cancellation_keeps_pending_retirement_distinct() {
        let g = gate();
        assert!(g.start());
        g.cancel();
        assert!(g.running());
        assert!(!g.publish());
        g.finish(true);
        assert!(!g.running());
        assert!(!g.publish());
        assert!(!g.published());
    }
    #[test]
    fn failed_or_released_host_cannot_reauthorize() {
        let g = gate();
        g.cancel();
        assert!(!g.start());
        g.finish(true);
        assert!(!g.publish());
        let g = gate();
        assert!(g.start());
        g.finish(false);
        g.finish(true);
        assert!(!g.publish());
    }
    #[test]
    fn new_session_does_not_inherit_old_completion() {
        let old = gate();
        assert!(old.start());
        let new = gate();
        old.finish(true);
        assert!(!new.publish());
        assert!(old.publish());
        old.cancel();
        assert!(!old.published());
        assert!(new.start());
        assert!(!new.published());
    }
}
