//! CPU-only handoff gate. Tests simulate host events, never Metal completion.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
pub(crate) struct Gate {
    state: AtomicU8,
    cancel: Arc<AtomicBool>,
    claimed: AtomicBool,
    native_confirmed: AtomicBool,
}
impl Gate {
    pub fn new(cancel: Arc<AtomicBool>) -> Self {
        Self {
            state: AtomicU8::new(1),
            cancel,
            claimed: AtomicBool::new(false),
            native_confirmed: AtomicBool::new(false),
        }
    }
    pub fn start(&self) -> bool {
        !self.cancelled()
            && self
                .state
                .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    pub fn claim_display(&self) -> bool {
        self.running()
            && !self.cancelled()
            && self
                .claimed
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
    }
    /// Crate-private: ONLY the exact Presentation::poll successful terminal path
    /// calls this in production. No public receipt/status setter can confirm it.
    pub fn native_completed(&self) {
        if self.running() && !self.cancelled() && self.claimed.load(Ordering::Acquire) {
            self.native_confirmed.store(true, Ordering::Release);
        }
    }
    pub fn verification_ready(&self) -> bool {
        !self.cancelled() && self.native_confirmed.load(Ordering::Acquire)
    }
    pub fn finish(&self, success: bool) -> bool {
        let verified = success && self.verification_ready();
        self.state
            .compare_exchange(
                2,
                if verified { 3 } else { 4 },
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
            && verified
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
        assert!(g.claim_display());
        assert!(!g.claim_display());
        g.native_completed(); // SIMULATED terminal event, no GPU in this CPU test
        assert!(g.finish(true));
        assert!(!g.published());
        assert!(g.publish());
        assert!(g.published());
        assert!(!g.publish());
        assert!(!g.start());
        g.finish(false);
        assert!(g.published());
    }
    #[test]
    fn worker_ok_without_native_command_confirmation_cannot_publish() {
        let g = gate();
        assert!(g.start());
        assert!(g.claim_display());
        assert!(!g.finish(true));
        assert!(!g.publish());
        g.native_completed();
        assert!(!g.finish(true));
        assert!(!g.publish());
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
        assert!(old.claim_display());
        old.native_completed(); // simulated
        assert!(old.finish(true));
        assert!(!new.publish());
        assert!(old.publish());
        old.cancel();
        assert!(!old.published());
        assert!(new.start());
        assert!(!new.published());
    }
    #[test]
    fn injected_worker_failures_and_detachment_never_authorize_reveal() {
        // CPU host protocol only. These labels are injection locations, not
        // synthetic claims that a Metal command was submitted or completed.
        for stage in [
            "device",
            "upload",
            "bank-completion",
            "draw",
            "draw-completion",
            "drawable",
            "presentation",
            "present-completion",
            "host-revalidation",
        ] {
            let g = gate();
            assert!(g.start());
            assert!(g.claim_display());
            if stage == "host-revalidation" {
                g.native_completed();
            }
            g.cancel(); // injected failure or detached Swift view
            assert!(!g.finish(true), "{stage}");
            assert!(!g.publish(), "{stage}");
            assert!(!g.published(), "{stage}");
            let later = gate();
            g.native_completed(); // stale callback cannot authorize another gate
            assert!(later.start());
            assert!(!later.finish(true), "{stage}");
            assert!(!later.publish());
        }
    }
}
