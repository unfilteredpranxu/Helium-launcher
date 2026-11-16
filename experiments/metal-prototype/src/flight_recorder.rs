//! Bounded, privacy-by-construction G06-D recorder core. No rendering authorization.
//! MIT; Copyright (c) 2026 Horizon contributors.
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        mpsc::{self, SyncSender},
    },
    time::Instant,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Cause {
    Unknown = 1,
    MeshReplaced,
    ColumnUnloaded,
    WorldChanged,
    RendererChanged,
    MaterialChanged,
    LightmapChanged,
    BankStale,
    FrameSizeChanged,
    ViewLayoutInvalid,
    ViewDetached,
    WindowNotKey,
    ModalOpen,
    AppInactive,
    HostExpired,
    TicketCancelled,
    Shutdown,
    WorkerFailed,
    Panic,
    SnapshotExpired,
}
impl Cause {
    pub fn from_code(n: u32) -> Self {
        match n {
            2 => Self::MeshReplaced,
            3 => Self::ColumnUnloaded,
            4 => Self::WorldChanged,
            5 => Self::RendererChanged,
            6 => Self::MaterialChanged,
            7 => Self::LightmapChanged,
            8 => Self::BankStale,
            9 => Self::FrameSizeChanged,
            10 => Self::ViewLayoutInvalid,
            11 => Self::ViewDetached,
            12 => Self::WindowNotKey,
            13 => Self::ModalOpen,
            14 => Self::AppInactive,
            15 => Self::HostExpired,
            16 => Self::TicketCancelled,
            17 => Self::Shutdown,
            18 => Self::WorkerFailed,
            19 => Self::Panic,
            20 => Self::SnapshotExpired,
            _ => Self::Unknown,
        }
    }
    pub fn reason(s: &str) -> Self {
        match s {
            "in-flight section replaced" | "displayed section replaced" => Self::MeshReplaced,
            "column unloaded" | "displayed column unloaded" => Self::ColumnUnloaded,
            "world replaced" | "world exit" => Self::WorldChanged,
            "application suspended" => Self::AppInactive,
            "actual game window changed/destroyed" => Self::RendererChanged,
            _ => Self::Unknown,
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Stage {
    HostRequested,
    WorkerStarted,
    DeviceCreated,
    UploadStarted,
    BanksResident,
    DrawSubmitted,
    DrawCompleted,
    PresentSubmitted,
    PresentCompleted,
    PublishRequested,
    UiRevealed,
    Cancelled,
}
#[derive(Clone, Copy, Debug)]
pub enum Emitter {
    Game,
    Worker,
    Swift,
    Ticket,
}
#[derive(Clone, Debug)]
pub struct Event {
    pub seq: u64,
    pub attempt: u64,
    pub nanos: u128,
    pub stage: Stage,
    pub emitter: Emitter,
    pub cause: Option<Cause>,
    pub first: bool,
    pub expected: u64,
    pub observed: u64,
    pub test_only: bool,
}
impl Event {
    fn json(&self) -> String {
        format!(
            "{{\"test_only\":{},\"schema_version\":1,\"event_seq\":{},\"attempt_id\":{},\"monotonic_ns\":{},\"stage\":\"{:?}\",\"emitter\":\"{:?}\",\"cause\":\"{:?}\",\"first\":{},\"expected\":{},\"observed\":{}}}\n",
            self.test_only,
            self.seq,
            self.attempt,
            self.nanos,
            self.stage,
            self.emitter,
            self.cause,
            self.first,
            self.expected,
            self.observed
        )
    }
}
struct State {
    attempt: u64,
    seq: u64,
    first: Option<Cause>,
    events: VecDeque<Event>,
    dropped: u64,
}
pub struct Recorder {
    state: Mutex<State>,
    start: Instant,
    writer: Option<SyncSender<Event>>,
    test_only: bool,
}
impl Recorder {
    pub fn memory() -> Self {
        Self {
            state: Mutex::new(State {
                attempt: 0,
                seq: 0,
                first: None,
                events: VecDeque::new(),
                dropped: 0,
            }),
            start: Instant::now(),
            writer: None,
            test_only: true,
        }
    }
    pub fn persistent(path: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&path)?;
        let (send, recv) = mpsc::sync_channel::<Event>(64);
        std::thread::Builder::new()
            .name("helium-flight-writer".into())
            .spawn(move || {
                let mut store = Store::new(path);
                while let Ok(e) = recv.recv() {
                    let _ = store.append(&e);
                }
            })?;
        let mut r = Self::memory();
        r.writer = Some(send);
        r.test_only = false;
        Ok(r)
    }
    pub fn begin(&self, id: u64) {
        if let Ok(mut s) = self.state.lock() {
            s.attempt = id;
            s.first = None;
        }
    }
    pub fn current(&self) -> u64 {
        self.state.lock().map(|s| s.attempt).unwrap_or(0)
    }
    pub fn record(
        &self,
        id: u64,
        stage: Stage,
        emitter: Emitter,
        cause: Option<Cause>,
        expected: u64,
        observed: u64,
    ) {
        // Lock covers sequence assignment AND queue insertion: cross-thread ordering is canonical.
        if let Ok(mut s) = self.state.lock() {
            let first = id == s.attempt && cause.is_some() && s.first.is_none();
            if first {
                s.first = cause;
            }
            let Some(seq) = s.seq.checked_add(1) else {
                return;
            };
            s.seq = seq;
            let e = Event {
                seq,
                attempt: id,
                nanos: self.start.elapsed().as_nanos(),
                stage,
                emitter,
                cause,
                first,
                expected,
                observed,
                test_only: self.test_only,
            };
            if s.events.len() == 128 {
                s.events.pop_front();
                s.dropped += 1;
            }
            s.events.push_back(e.clone());
            if let Some(w) = &self.writer {
                if w.try_send(e).is_err() {
                    s.dropped += 1;
                }
            }
        }
    }
    pub fn snapshot(&self) -> Vec<Event> {
        self.state
            .lock()
            .map(|s| s.events.iter().cloned().collect())
            .unwrap_or_default()
    }
}
pub struct Store {
    path: PathBuf,
    summary: VecDeque<Event>,
    first: VecDeque<Event>,
}
impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            summary: VecDeque::new(),
            first: VecDeque::new(),
        }
    }
    pub fn append(&mut self, e: &Event) -> io::Result<()> {
        fs::create_dir_all(&self.path)?;
        let file = self.path.join("helium-metal-events.jsonl");
        let line = e.json();
        if fs::metadata(&file).map(|m| m.len()).unwrap_or(0) + line.len() as u64 > 512 * 1024 {
            let old = self.path.join("helium-metal-previous.jsonl");
            if old.exists() {
                fs::remove_file(&old)?;
            }
            fs::rename(&file, old)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)?
            .write_all(line.as_bytes())?;
        if e.first && self.first.len() < 3 {
            self.first.push_back(e.clone());
        }
        if self.summary.len() == 80 {
            self.summary.pop_front();
        }
        self.summary.push_back(e.clone());
        let mut report = String::from(
            "HELIUM G06-D RECORDER CORE — NOT OWNER-TEST READY\nOWNER_SCREEN_CONFIRMED: UNVERIFIED\nNo assets, account identifiers, paths, chat or free-form text accepted.\n",
        );
        report.push_str("FIRST CAUSES (retained independently of timeline tail):\n");
        for e in &self.first {
            report.push_str(&e.json());
        }
        report.push_str("TIMELINE TAIL:\n");
        for e in &self.summary {
            report.push_str(&e.json());
        }
        report.push_str("NEXT ACTION FOR CODEX: finish HUD, recovery, saturation/error reporting and full integration gates.\n");
        let tmp = self.path.join("Helium-Metal-Report.pending");
        fs::write(&tmp, report)?;
        fs::rename(tmp, self.path.join("Helium-Metal-Report.txt"))
    }
}
static GLOBAL: OnceLock<Recorder> = OnceLock::new();
pub fn global() -> &'static Recorder {
    GLOBAL.get_or_init(|| {
        let path = std::env::current_dir().unwrap_or_default().join("logs");
        #[cfg(target_os = "ios")]
        {
            Recorder::persistent(path).unwrap_or_else(|_| Recorder::memory())
        }
        #[cfg(not(target_os = "ios"))]
        {
            let _ = path;
            Recorder::memory()
        }
    })
}
pub fn emit(stage: Stage, emitter: Emitter, cause: Option<Cause>, expected: u64, observed: u64) {
    let r = global();
    r.record(r.current(), stage, emitter, cause, expected, observed);
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    #[test]
    fn concurrent_first_cause_and_order() {
        let r = Arc::new(Recorder::memory());
        r.begin(7);
        std::thread::scope(|s| {
            for c in [Cause::MaterialChanged, Cause::ViewLayoutInvalid] {
                let r = r.clone();
                s.spawn(move || r.record(7, Stage::Cancelled, Emitter::Game, Some(c), 1, 2));
            }
        });
        let e = r.snapshot();
        assert_eq!(e.iter().filter(|e| e.first).count(), 1);
        assert_eq!(e[0].seq + 1, e[1].seq);
    }
    #[test]
    fn late_previous_attempt_does_not_latch_current() {
        let r = Recorder::memory();
        r.begin(1);
        r.begin(2);
        r.record(
            1,
            Stage::Cancelled,
            Emitter::Worker,
            Some(Cause::Panic),
            0,
            0,
        );
        r.record(
            2,
            Stage::Cancelled,
            Emitter::Swift,
            Some(Cause::ViewDetached),
            0,
            0,
        );
        assert!(!r.snapshot()[0].first);
        assert!(r.snapshot()[1].first);
    }
    #[test]
    fn ring_is_bounded() {
        let r = Recorder::memory();
        for _ in 0..1000 {
            r.record(0, Stage::HostRequested, Emitter::Swift, None, 0, 0);
        }
        assert_eq!(r.snapshot().len(), 128);
    }
    #[test]
    fn roundtrip_and_rotation() {
        let path = std::env::temp_dir().join(format!(
            "helium-core-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = Store::new(path.clone());
        let r = Recorder::memory();
        r.begin(3);
        r.record(
            3,
            Stage::Cancelled,
            Emitter::Swift,
            Some(Cause::ViewLayoutInvalid),
            2360,
            0,
        );
        let e = r.snapshot().pop().unwrap();
        for _ in 0..4000 {
            store.append(&e).unwrap();
        }
        let report = fs::read_to_string(path.join("Helium-Metal-Report.txt")).unwrap();
        assert!(report.contains("ViewLayoutInvalid"));
        assert!(report.len() < 256 * 1024);
        assert!(
            fs::metadata(path.join("helium-metal-events.jsonl"))
                .unwrap()
                .len()
                <= 512 * 1024
        );
        assert!(
            fs::metadata(path.join("helium-metal-previous.jsonl"))
                .unwrap()
                .len()
                <= 512 * 1024
        );
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn arbitrary_text_is_not_persisted() {
        assert_eq!(
            Cause::reason("token=FAKE password=FAKE /private/world 10.2.3.4 PIN123456"),
            Cause::Unknown
        );
        assert_eq!(Cause::from_code(u32::MAX), Cause::Unknown);
    }
}
