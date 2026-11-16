//! G05-D SDR presentation contract. No command completion implies scanout.
pub const VERSION: u32 = 1;
#[cfg(any(target_os = "ios", target_os = "macos"))]
pub use crate::terrain::metal::presentation::Display;
/// Aspect-fit physical pixel rectangle, top-left origin. Black outside.
pub fn fit(
    source: (usize, usize),
    target: (usize, usize),
    live: usize,
    budget: usize,
) -> Result<[f64; 4], String> {
    if [source.0, source.1, target.0, target.1]
        .iter()
        .any(|&n| n == 0 || n > 4096)
    {
        return Err("invalid display dimensions".into());
    }
    // Charge all three possible Core Animation buffers, not only the acquired one.
    let bytes = target
        .0
        .checked_mul(target.1)
        .and_then(|n| n.checked_mul(12))
        .and_then(|n| n.checked_add(live))
        .ok_or("display payload overflow")?;
    if bytes > budget {
        return Err("display budget exceeded".into());
    }
    let scale = (target.0 as f64 / source.0 as f64).min(target.1 as f64 / source.1 as f64);
    let w = source.0 as f64 * scale;
    let h = source.1 as f64 * scale;
    Ok([(target.0 as f64 - w) / 2., (target.1 as f64 - h) / 2., w, h])
}
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod host;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aspect_orientation_and_budget() {
        assert_eq!(
            fit((256, 256), (512, 256), 0, 2_000_000).unwrap(),
            [128., 0., 256., 256.]
        );
        assert_eq!(
            fit((256, 256), (256, 512), 0, 2_000_000).unwrap(),
            [0., 128., 256., 256.]
        );
        for dims in [(0, 1), (4097, 1), (usize::MAX, usize::MAX)] {
            assert!(fit((256, 256), dims, 0, usize::MAX).is_err());
        }
        assert!(fit((256, 256), (256, 256), usize::MAX, usize::MAX).is_err());
        assert!(fit((256, 256), (256, 256), 0, 1).is_err());
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub submitted: u64,
    pub completed: u64,
    pub failed: u64,
    pub unavailable: u64,
    pub busy: u64,
    pub skipped: u64,
    pub polls: u64,
    pub invalidated: u64,
    pub current_payload: usize,
    pub peak_payload: usize,
}
#[derive(Default)]
pub(crate) struct Progress {
    pub counters: Counters,
    submitted: bool,
    terminal: bool,
    revoked: bool,
}
#[cfg_attr(
    not(any(target_os = "macos", target_os = "ios", test)),
    allow(dead_code)
)]
impl Progress {
    pub fn begin(&mut self, payload: usize) -> Result<(), String> {
        if self.submitted {
            self.counters.skipped += 1;
            return Err("drawable/frame already submitted".into());
        }
        self.submitted = true;
        self.counters.submitted += 1;
        self.counters.current_payload = payload;
        self.counters.peak_payload = self.counters.peak_payload.max(payload);
        Ok(())
    }
    pub fn observe(&mut self, terminal: Option<bool>, current: bool) -> Result<bool, String> {
        if !self.submitted || self.terminal {
            return Err("no unconsumed presentation command".into());
        }
        self.counters.polls += 1;
        if !current && !self.revoked {
            self.revoked = true;
            self.counters.invalidated += 1;
        }
        let Some(success) = terminal else {
            return Ok(false);
        };
        self.terminal = true;
        self.counters.current_payload = 0;
        if success && !self.revoked {
            self.counters.completed += 1;
            Ok(true)
        } else {
            self.counters.failed += 1;
            Err("presentation failed/revoked".into())
        }
    }
}
#[cfg(test)]
mod protocol_tests {
    use super::*;
    #[test]
    fn simulated_exact_command_states_and_one_use() {
        let mut p = Progress::default();
        assert!(p.observe(Some(true), true).is_err());
        p.begin(1024).unwrap();
        assert!(p.begin(1024).is_err());
        for _ in 0..3 {
            assert!(!p.observe(None, true).unwrap());
        }
        assert!(p.observe(Some(true), true).unwrap());
        assert!(p.observe(Some(true), true).is_err());
        assert_eq!(p.counters.completed, 1);
    }
    #[test]
    fn simulated_revocation_is_not_gpu_retirement() {
        for _reason in [
            "wrong device",
            "stale revision",
            "frame release",
            "layer release",
            "resize",
            "suspend",
        ] {
            let mut p = Progress::default();
            p.begin(8).unwrap();
            assert!(!p.observe(None, false).unwrap());
            assert_eq!(p.counters.current_payload, 8);
            assert!(!p.observe(None, true).unwrap());
            assert!(p.observe(Some(true), true).is_err());
            assert_eq!(p.counters.completed, 0);
            assert_eq!(p.counters.current_payload, 0);
        }
        let mut p = Progress::default();
        p.begin(8).unwrap();
        assert!(p.observe(Some(false), true).is_err());
        assert!(p.observe(Some(true), true).is_err());
    }
}
