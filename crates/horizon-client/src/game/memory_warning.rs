//! What the game does about memory warnings (iOS's `didReceiveMemoryWarning`).
//!
//! iOS has no quota an app can set: Jetsam kills it once its footprint nears
//! the device's, so the only lever is to free what can be freed. A warning
//! always releases memory at once (the streaming workers' scratch, the
//! allocator's free pages); two warnings inside [`WARN_WINDOW`] with a
//! singleplayer world at a long render distance also lower the *effective*
//! render distance, and it comes back after [`RESTORE_AFTER`] without
//! warnings. The options are never written.

use std::time::{Duration, Instant};

/// The effective render distance while lowered.
pub(super) const LOWER_RADIUS: i32 = 6;
/// Lowering only helps above this render distance; at or below it a warning
/// just frees memory.
pub(super) const LOWER_ABOVE: i32 = 8;
/// A second warning inside this window lowers the render distance.
const WARN_WINDOW: Duration = Duration::from_secs(60);
/// Quiet time before the render distance is restored.
pub(super) const RESTORE_AFTER: Duration = Duration::from_secs(120);

/// The memory warnings' state: when the last one came, how many in a row, and
/// whether the render distance is lowered.
#[derive(Default)]
pub(super) struct Memory {
    last_warning: Option<Instant>,
    /// Warnings in the current window (1 after the first).
    recent: u8,
    lowered: bool,
}

impl Memory {
    /// Notes a warning; true once this is at least the second one inside
    /// [`WARN_WINDOW`]. Every warning counts, lowered or not, so the restore
    /// waits for a quiet while rather than the lowering warning alone.
    pub(super) fn note_warning(&mut self, now: Instant) -> bool {
        let in_window = self
            .last_warning
            .is_some_and(|last| now.duration_since(last) < WARN_WINDOW);
        self.last_warning = Some(now);
        if !in_window {
            self.recent = 1;
            return false;
        }
        self.recent = self.recent.saturating_add(1);
        self.recent >= 2
    }
    pub(super) fn is_lowered(&self) -> bool {
        self.lowered
    }
    pub(super) fn lower(&mut self) {
        self.lowered = true;
    }
    /// The render distance in use: the option, or [`LOWER_RADIUS`] while lowered.
    pub(super) fn effective(&self, render_distance: i32) -> i32 {
        if self.lowered {
            render_distance.min(LOWER_RADIUS)
        } else {
            render_distance
        }
    }
    pub(super) fn restore_due(&self, now: Instant) -> bool {
        self.lowered
            && self
                .last_warning
                .is_none_or(|last| now.duration_since(last) >= RESTORE_AFTER)
    }
    pub(super) fn restore(&mut self) {
        self.lowered = false;
        self.recent = 0;
    }
    /// Left the world: the counts and the lowering belong to it.
    pub(super) fn close_world(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_warning_does_not_lower() {
        let mut memory = Memory::default();
        let now = Instant::now();
        assert!(!memory.note_warning(now));
        assert!(!memory.is_lowered());
        assert!(!memory.restore_due(now));
    }

    #[test]
    fn second_warning_in_the_window_lowers() {
        let mut memory = Memory::default();
        let now = Instant::now();
        assert!(!memory.note_warning(now));
        assert!(memory.note_warning(now + Duration::from_secs(30)));
        memory.lower();
        assert_eq!(memory.effective(12), 6);
        // Below the lowered distance, the option stays.
        assert_eq!(memory.effective(4), 4);
    }

    #[test]
    fn warnings_far_apart_do_not_lower() {
        let mut memory = Memory::default();
        let now = Instant::now();
        assert!(!memory.note_warning(now));
        assert!(!memory.note_warning(now + WARN_WINDOW));
        assert!(!memory.is_lowered());
    }

    #[test]
    fn restored_after_a_quiet_while() {
        let mut memory = Memory::default();
        let now = Instant::now();
        memory.note_warning(now);
        assert!(memory.note_warning(now + Duration::from_secs(1)));
        memory.lower();
        assert!(!memory.restore_due(now + RESTORE_AFTER));
        assert!(memory.restore_due(now + RESTORE_AFTER + Duration::from_secs(1)));
        memory.restore();
        assert_eq!(memory.effective(12), 12);
        assert!(!memory.is_lowered());
    }

    #[test]
    fn a_warning_while_lowered_postpones_the_restore() {
        let mut memory = Memory::default();
        let now = Instant::now();
        memory.note_warning(now);
        memory.note_warning(now + Duration::from_secs(1));
        memory.lower();
        // The lowering ignores it, but the quiet while starts over.
        assert!(memory.note_warning(now + Duration::from_secs(100)));
        assert!(!memory.restore_due(now + RESTORE_AFTER + Duration::from_secs(1)));
        assert!(memory.restore_due(now + Duration::from_secs(100) + RESTORE_AFTER));
    }

    #[test]
    fn closing_the_world_forgets_everything() {
        let mut memory = Memory::default();
        let now = Instant::now();
        memory.note_warning(now);
        memory.note_warning(now + Duration::from_secs(1));
        memory.lower();
        memory.close_world();
        assert!(!memory.is_lowered());
        assert!(!memory.note_warning(now + Duration::from_secs(2)));
    }
}
