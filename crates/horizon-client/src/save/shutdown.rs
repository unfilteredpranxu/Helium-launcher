//! Bounded shutdown evidence, not a shutdown coordinator or a write-ahead journal.
//! MIT; Copyright (c) 2026 Horizon contributors.
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Actor {
    Region = 1,
    Player,
    Server,
    Streaming,
    Lod,
    Main,
    Native,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u64)]
pub enum Stage {
    Requested = 1,
    Queued,
    Started,
    Synced,
    Failed,
    Drain,
    JoinBegin,
    JoinEnd,
    Suspended,
    Exit,
    Deadline,
    LockReleased,
    WaitBegin,
    WaitEnd,
    Superseded,
}
#[derive(Clone, Debug)]
struct Event {
    ms: u64,
    storage: u64,
    actor: Actor,
    stage: Stage,
    records: u64,
    outstanding: u64,
    main: bool,
}
#[derive(Default)]
struct State {
    seq: u64,
    lost: u64,
    errors: u64,
    events: VecDeque<Event>,
    first_errors: Vec<Event>,
    joins: Vec<Event>,
}
pub struct Trace {
    start: Instant,
    boot: AtomicU64,
    main: OnceLock<std::thread::ThreadId>,
    state: Mutex<State>,
}
impl Trace {
    fn new(main: Option<std::thread::ThreadId>) -> Self {
        Self {
            start: Instant::now(),
            boot: AtomicU64::new(0),
            main: {
                let slot = OnceLock::new();
                if let Some(id) = main {
                    let _ = slot.set(id);
                }
                slot
            },
            state: Mutex::new(State::default()),
        }
    }
    pub fn record(
        &self,
        storage: u64,
        actor: Actor,
        stage: Stage,
        records: usize,
        outstanding: usize,
    ) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let e = Event {
            ms: self.start.elapsed().as_millis() as u64,
            storage,
            actor,
            stage,
            records: records as u64,
            outstanding: outstanding as u64,
            main: self.main.get() == Some(&std::thread::current().id()),
        };
        // Repeated status probes are not transitions. Queue/write events remain
        // visible; first errors have a separate bounded priority reservation.
        if matches!(
            stage,
            Stage::Drain | Stage::Deadline | Stage::Exit | Stage::Suspended | Stage::Requested
        ) && s.events.iter().rev().any(|v| {
            v.storage == storage
                && v.actor == actor
                && v.stage == stage
                && v.records == e.records
                && v.outstanding == e.outstanding
        }) {
            return;
        }
        s.seq = s.seq.saturating_add(1);
        if matches!(stage, Stage::JoinBegin | Stage::WaitBegin) && s.joins.len() < 16 {
            s.joins.push(e.clone());
        }
        if matches!(stage, Stage::JoinEnd | Stage::WaitEnd) {
            s.joins.retain(|v| v.actor != actor || v.storage != storage);
        }
        if stage == Stage::Failed {
            s.errors = s.errors.saturating_add(1);
            if s.first_errors.len() < 16 {
                s.first_errors.push(e.clone());
            }
        }
        if s.events.len() == 128 {
            s.events.pop_front();
            s.lost = s.lost.saturating_add(1);
        }
        s.events.push_back(e);
    }
    fn numeric(&self) -> (u64, String, bool) {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut out = format!(
            "1 {} {} {} {} {} {}\n",
            s.seq,
            s.lost,
            s.errors,
            self.start.elapsed().as_millis(),
            u8::from(self.main.get().is_some()),
            self.boot.load(Ordering::Acquire)
        );
        for e in s
            .first_errors
            .iter()
            .chain(s.joins.iter())
            .chain(s.events.iter())
        {
            out.push_str(&format!(
                "{} {} {} {} {} {} {}\n",
                e.ms,
                e.storage,
                e.actor as u64,
                e.stage as u64,
                e.records,
                e.outstanding,
                u8::from(e.main)
            ));
        }
        (s.seq, out, !s.joins.is_empty())
    }
}
static GLOBAL: OnceLock<Arc<Trace>> = OnceLock::new();
static WRITER: OnceLock<bool> = OnceLock::new();
static WRITE_FAILED: AtomicBool = AtomicBool::new(false);
pub fn global() -> &'static Trace {
    GLOBAL.get_or_init(|| Arc::new(Trace::new(None)))
}
/// Register from winit's UI/event-loop callback, not an assumed guest entry
/// thread. This does not assert how LiveContainer implements process exit.
pub fn mark_event_thread() {
    let _ = global().main.set(std::thread::current().id());
}
pub fn is_event_thread() -> bool {
    global().main.get() == Some(&std::thread::current().id())
}
pub fn new_storage_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .unwrap_or(0)
}
/// Called on the actual application main thread. A reporter owns only numeric
/// snapshots and a log destination; it never owns Game, SaveDir or session locks.
/// No join is installed on this diagnostic reporter, and it acknowledges no saves.
pub fn initialize(path: PathBuf, boot: u64) {
    horizon_threads::shutdown_observer::install(|target, beginning| {
        use horizon_threads::shutdown_observer::Target;
        let actor = match target {
            Target::LodJoin => Actor::Lod,
            _ => Actor::Streaming,
        };
        let stage = match (matches!(target, Target::LodJoin), beginning) {
            (true, true) => Stage::JoinBegin,
            (true, false) => Stage::JoinEnd,
            (false, true) => Stage::WaitBegin,
            (false, false) => Stage::WaitEnd,
        };
        global().record(0, actor, stage, usize::MAX, usize::MAX);
    });
    let trace = GLOBAL.get_or_init(|| Arc::new(Trace::new(None))).clone();
    let boot = if boot == 0 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |t| t.as_nanos() as u64)
    } else {
        boot
    };
    trace.boot.store(boot, Ordering::Release);
    let started = WRITER.get_or_init(|| {
        std::thread::Builder::new()
            .name("horizon-shutdown-report".into())
            .spawn(move || {
                // Preserve a prior run with substantive events; an empty launch
                // must not overwrite the last shutdown trace.
                if let Ok(file) = std::fs::File::open(path.join("shutdown-save-numeric.txt")) {
                    use std::io::Read;
                    let mut old = String::new();
                    if file.take(32 * 1024 + 1).read_to_string(&mut old).is_ok()
                        && old.len() <= 32 * 1024
                        && old.lines().count() > 1
                    {
                        let _ = std::fs::write(path.join("shutdown-save-previous.txt"), old);
                    }
                }
                let mut persisted = u64::MAX;
                let mut second = u64::MAX;
                loop {
                    let (seq, text, joining) = trace.numeric();
                    let tick = trace.start.elapsed().as_secs();
                    if seq != persisted
                        || (joining && tick != second)
                        || WRITE_FAILED.load(Ordering::Acquire)
                    {
                        let result = std::fs::create_dir_all(&path).and_then(|_| {
                            let temporary = path.join("shutdown-save-numeric.pending");
                            std::fs::write(&temporary, text)?;
                            std::fs::rename(temporary, path.join("shutdown-save-numeric.txt"))
                        });
                        WRITE_FAILED.store(result.is_err(), Ordering::Release);
                        if result.is_ok() {
                            persisted = seq;
                            second = tick;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            })
            .is_ok()
    });
    if !*started {
        WRITE_FAILED.store(true, Ordering::Release);
    }
}

/// Counts are observations, not proof that metadata/player/other dimensions are
/// durable. A timeout must preserve the storage and its session-lock owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteStatus {
    pub outstanding_batches: usize,
    pub pending_records: usize,
    pub failed: bool,
}
impl WriteStatus {
    pub fn finished(self) -> bool {
        !self.failed && self.outstanding_batches == 0 && self.pending_records == 0
    }
}

#[cfg(test)]
mod tests;
