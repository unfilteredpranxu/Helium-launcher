use super::*;

/// CPU content is the target of encoded jobs, never evidence of completed GPU work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    InitialCpu,
    Encoded,
    QueuedForSubmission,
    SubmittedUnconfirmed,
    Released,
    Unavailable,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ticket {
    generation: Generation,
    revision: u64,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub updated_layers: u64,
    pub regenerated_bytes: u64,
    /// Snapshot initialization and bank COW copies; refresh writes counted above.
    pub copied_bytes: u64,
    pub reused_bytes: u64,
    pub invalidations: u64,
    pub superseded_revisions: u64,
    pub failed_refreshes: u64,
    pub failed_budget_checks: u64,
    pub retained_capacity: usize,
    /// High-water live charged capacity (including historical snapshots and frames).
    pub peak_capacity: usize,
    /// Maximum newly allocated COW capacity in one refresh (old versions coexist).
    pub temporary_peak_capacity: usize,
    pub source_frame_capacity: usize,
}
pub(crate) struct Frames {
    pub data: Vec<Vec<u8>>,
    _charge: Charge,
}
#[derive(Clone, Copy)]
pub(crate) struct Job<'a> {
    pub bank: usize,
    pub words: [u32; 5],
    pub frames: &'a [u8],
}
impl State {
    /// Workload limit for bytes written by one refresh; exceeding it releases
    /// observation and requires recapture. It never defers production animation.
    pub fn set_refresh_limit(&mut self, bytes: usize) {
        self.refresh_limit = bytes;
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn ticket(&self) -> Result<Ticket, &'static str> {
        self.snapshot()?;
        Ok(Ticket {
            generation: self.generation,
            revision: self.animation_revision,
        })
    }
    pub fn check_ticket(&self, ticket: Ticket) -> Result<(), &'static str> {
        if self.ticket()? != ticket {
            Err("obsolete material generation/revision")
        } else {
            Ok(())
        }
    }
    pub fn counters(&self) -> Counters {
        let mut counters = self.counters;
        if let Some(budget) = &self.budget {
            counters.retained_capacity = budget.live();
            counters.peak_capacity = budget.peak();
        }
        counters.source_frame_capacity = self.frames.as_ref().map_or(0, |f| f._charge.bytes());
        counters
    }
    /// No completion proof at this boundary, including the deferred submit path.
    pub fn gpu_current(&self) -> Result<(), &'static str> {
        Err("GPU completion not observed; CPU target content only")
    }
    pub(in crate::gfx) fn queued(&mut self) {
        if self.snapshot.is_some() {
            self.phase = Phase::QueuedForSubmission;
        }
    }
    pub(in crate::gfx) fn submitted(&mut self) {
        if self.snapshot.is_some() {
            self.phase = Phase::SubmittedUnconfirmed;
        }
    }
    pub(super) fn fail(&mut self, error: &'static str) {
        if self.snapshot.take().is_some() {
            self.counters.invalidations = self.counters.invalidations.saturating_add(1);
        }
        self.frames = None;
        self.unavailable = error;
        self.phase = Phase::Unavailable;
    }
    pub(in crate::gfx) fn retain_frames(&mut self, data: Vec<Vec<u8>>) {
        if self.snapshot.is_none() {
            return;
        }
        let capacity = data.iter().fold(
            data.capacity().checked_mul(size_of::<Vec<u8>>()),
            |sum, v| sum.and_then(|s| s.checked_add(v.capacity())),
        );
        let charge = capacity
            .ok_or("animation frame capacity overflow")
            .and_then(|n| self.budget.as_ref().unwrap().charge(n));
        match charge {
            Ok(charge) => {
                self.frames = Some(Frames {
                    data,
                    _charge: charge,
                })
            }
            Err(error) => {
                self.counters.failed_budget_checks =
                    self.counters.failed_budget_checks.saturating_add(1);
                self.fail(error);
            }
        }
    }
    pub(in crate::gfx) fn take_frames(&mut self) -> Option<Frames> {
        self.frames.take()
    }
    pub(in crate::gfx) fn restore_frames(&mut self, frames: Option<Frames>) {
        if self.snapshot.is_some() {
            self.frames = frames;
        }
    }
    pub(in crate::gfx) fn refresh<'a>(&mut self, jobs: impl Iterator<Item = Job<'a>> + Clone) {
        self.animation_revision = self
            .animation_revision
            .checked_add(1)
            .expect("animation revision exhausted");
        if self.snapshot.is_none() {
            return;
        }
        self.counters.superseded_revisions = self.counters.superseded_revisions.saturating_add(1);
        self.phase = Phase::Encoded;
        if let Err(error) = self.refresh_checked(jobs) {
            self.counters.failed_refreshes = self.counters.failed_refreshes.saturating_add(1);
            if error.contains("budget") {
                self.counters.failed_budget_checks =
                    self.counters.failed_budget_checks.saturating_add(1);
            }
            self.fail(error);
        }
    }
    fn refresh_checked<'a>(
        &mut self,
        jobs: impl Iterator<Item = Job<'a>> + Clone,
    ) -> Result<(), &'static str> {
        let original = self.snapshot.as_ref().unwrap();
        let stride = chain(original.size, original.banks[0].levels)?;
        let mut changed = [false; 8];
        let mut count = 0usize;
        // At most 16384 production sprites; a fixed bitset rejects duplicate jobs.
        let mut seen = [0u64; 512];
        for job in jobs.clone() {
            let bank = original
                .banks
                .get(job.bank)
                .ok_or("invalid animation bank")?;
            if job.words[0] >= bank.used {
                return Err("invalid animation slice/padding layer");
            }
            let global = job
                .bank
                .checked_mul(original.split as usize)
                .and_then(|n| n.checked_add(job.words[0] as usize))
                .ok_or("animation sprite overflow")?;
            if global >= original.sprites as usize {
                return Err("missing global animation sprite");
            }
            let bit = 1 << (global % 64);
            if seen[global / 64] & bit != 0 {
                return Err("duplicate animation layer");
            }
            seen[global / 64] |= bit;
            super::super::animate::validate_mix(job.frames, job.words, original.size)?;
            changed[job.bank] = true;
            count = count.checked_add(1).ok_or("animation job overflow")?;
        }
        if count == 0 {
            return Err("missing animation jobs");
        }
        let regenerated = count.checked_mul(stride).ok_or("refresh byte overflow")?;
        if regenerated > self.refresh_limit {
            return Err("refresh workload budget exceeded");
        }
        let budget = self.budget.as_ref().unwrap();
        let mut temporary = 0usize;
        // Clone metadata only when an external consumer pins this revision.
        if Arc::get_mut(self.snapshot.as_mut().unwrap()).is_none() {
            let source = self.snapshot.as_ref().unwrap();
            let charge = budget.charge(
                source
                    .banks
                    .len()
                    .checked_mul(size_of::<Bank>())
                    .ok_or("metadata size overflow")?,
            )?;
            let mut banks = Vec::new();
            banks
                .try_reserve_exact(source.banks.len())
                .map_err(|_| "refresh metadata allocation failed")?;
            if banks.capacity() != source.banks.len() {
                return Err("unexpected metadata capacity");
            }
            temporary = banks.capacity() * size_of::<Bank>();
            self.counters.temporary_peak_capacity =
                self.counters.temporary_peak_capacity.max(temporary);
            banks.extend(source.banks.iter().cloned());
            self.snapshot = Some(Arc::new(Snapshot {
                generation: source.generation,
                split: source.split,
                sprites: source.sprites,
                size: source.size,
                retained_bytes: source.retained_bytes,
                revision: self.animation_revision,
                banks,
                _metadata: charge,
            }));
        }
        let snapshot = Arc::get_mut(self.snapshot.as_mut().unwrap()).unwrap();
        for (index, bank) in snapshot.banks.iter_mut().enumerate() {
            if !changed[index] {
                continue;
            }
            if Arc::get_mut(&mut bank.pixels).is_none() {
                let mut copy = Payload::zeroed(budget, bank.pixels.bytes.len())?;
                temporary = temporary
                    .checked_add(copy.bytes.capacity())
                    .ok_or("temporary byte overflow")?;
                self.counters.temporary_peak_capacity =
                    self.counters.temporary_peak_capacity.max(temporary);
                copy.bytes.copy_from_slice(&bank.pixels.bytes);
                self.counters.copied_bytes = self
                    .counters
                    .copied_bytes
                    .saturating_add(copy.bytes.len() as u64);
                bank.pixels = Arc::new(copy);
            } else {
                self.counters.reused_bytes = self
                    .counters
                    .reused_bytes
                    .saturating_add(bank.pixels.bytes.capacity() as u64);
            }
        }
        for job in jobs {
            let bank = &mut snapshot.banks[job.bank];
            let mut offset = (job.words[0] as usize)
                .checked_mul(stride)
                .ok_or("layer offset overflow")?;
            let pixels = &mut Arc::get_mut(&mut bank.pixels).unwrap().bytes;
            for level in 0..bank.levels {
                let width = snapshot.size >> level;
                let len = (width as usize)
                    .checked_mul(width as usize)
                    .and_then(|n| n.checked_mul(4))
                    .ok_or("mip byte overflow")?;
                let end = offset.checked_add(len).ok_or("mip offset overflow")?;
                super::super::animate::mix_level_into(
                    job.frames,
                    job.words,
                    width,
                    pixels
                        .get_mut(offset..end)
                        .ok_or("missing destination mip")?,
                )?;
                offset = end;
            }
            self.counters.updated_layers = self.counters.updated_layers.saturating_add(1);
            self.counters.regenerated_bytes = self
                .counters
                .regenerated_bytes
                .saturating_add(stride as u64);
        }
        snapshot.revision = self.animation_revision;
        snapshot.retained_bytes = snapshot.banks.iter().try_fold(
            snapshot
                .banks
                .capacity()
                .checked_mul(size_of::<Bank>())
                .ok_or("snapshot metadata overflow")?,
            |bytes, bank| {
                bytes
                    .checked_add(bank.pixels.bytes.capacity())
                    .ok_or("snapshot capacity overflow")
            },
        )?;
        Ok(())
    }
}
