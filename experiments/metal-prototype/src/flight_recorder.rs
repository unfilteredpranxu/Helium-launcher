//! G06-D bounded flight recorder; typed numeric input only, never rendering authority.
//! MIT; Copyright (c) 2026 Horizon contributors.
use std::{
    collections::VecDeque,
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
    ParentGone,
    WindowGone,
    WindowHidden,
    ResourceReload,
    LauncherCovered,
    ProducerUnavailable,
    CameraOutside,
    DeviceFailure,
    UploadFailed,
    FrameFailed,
    PresentFailed,
    DrawableUnavailable,
    GpuTimeout,
    MemoryPressure,
    NoFreshMesh,
    RetryExhausted,
    HudUnavailable,
    InvalidAbi,
    InvalidConfig,
    PreviousBootIncomplete,
    RecorderDegraded,
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
            21 => Self::ParentGone,
            22 => Self::WindowGone,
            23 => Self::WindowHidden,
            24 => Self::ResourceReload,
            25 => Self::LauncherCovered,
            26 => Self::ProducerUnavailable,
            27 => Self::CameraOutside,
            28 => Self::DeviceFailure,
            29 => Self::UploadFailed,
            30 => Self::FrameFailed,
            31 => Self::PresentFailed,
            32 => Self::DrawableUnavailable,
            33 => Self::GpuTimeout,
            34 => Self::MemoryPressure,
            35 => Self::NoFreshMesh,
            36 => Self::RetryExhausted,
            37 => Self::HudUnavailable,
            38 => Self::InvalidAbi,
            39 => Self::InvalidConfig,
            40 => Self::PreviousBootIncomplete,
            41 => Self::RecorderDegraded,
            _ => Self::Unknown,
        }
    }
    pub fn reason(s: &str) -> Self {
        match s {
            "in-flight section replaced" | "displayed section replaced" => Self::MeshReplaced,
            "column unloaded" | "displayed column unloaded" => Self::ColumnUnloaded,
            "world replaced" | "world exit" | "world exited" => Self::WorldChanged,
            "application suspended" => Self::AppInactive,
            "actual game window changed/destroyed" => Self::RendererChanged,
            "resource pack replaced" | "resource packs reloaded" | "resource pack reloaded" => {
                Self::ResourceReload
            }
            "native memory pressure" => Self::MemoryPressure,
            "launcher covered" => Self::LauncherCovered,
            "production frame unavailable" => Self::ProducerUnavailable,
            "displayed producer state changed" => Self::BankStale,
            "renderer replaced" | "renderer rebuilding" => Self::RendererChanged,
            "world closed" => Self::WorldChanged,
            "mesh sequence exhausted" => Self::Shutdown,
            _ => Self::Unknown,
        }
    }
    pub fn recoverable(self) -> bool {
        matches!(
            self,
            Self::MeshReplaced
                | Self::ColumnUnloaded
                | Self::MaterialChanged
                | Self::LightmapChanged
                | Self::BankStale
                | Self::FrameSizeChanged
                | Self::ViewLayoutInvalid
                | Self::CameraOutside
                | Self::DrawableUnavailable
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Stage {
    Boot = 1,
    ModeSelected = 2,
    ResourcesFound = 3,
    ResourcesLoading = 4,
    LauncherShown = 5,
    LauncherDismissed = 6,
    WorldRequested = 7,
    WorldReady = 8,
    ProducerReady = 9,
    CameraReady = 10,
    MeshAccepted = 11,
    CandidateRejected = 12,
    HostRequested = 13,
    Attached = 14,
    WorkerStarted = 15,
    DeviceCreated = 16,
    UploadStarted = 17,
    BankCompleted = 18,
    BanksResident = 19,
    DrawSubmitted = 20,
    DrawCompleted = 21,
    DrawableRequested = 22,
    DrawableAcquired = 23,
    PresentSubmitted = 24,
    PresentCompleted = 25,
    ProducerRevalidated = 26,
    PublishRequested = 27,
    UiRevealed = 28,
    Removed = 29,
    Cancelled = 30,
    Retired = 31,
    RetryWaiting = 32,
    RetryExhausted = 33,
    SelfTest = 34,
    MeshIdentity = 35,
    MeshOrigin = 36,
    MaterialRevision = 37,
    BankLayout = 38,
    DeviceMetadata = 39,
    TextureLayout = 40,
    UploadSubmitted = 41,
    MemoryCounters = 42,
}
impl Stage {
    pub fn from_code(n: u32) -> Option<Self> {
        Some(match n {
            1 => Self::Boot,
            2 => Self::ModeSelected,
            3 => Self::ResourcesFound,
            4 => Self::ResourcesLoading,
            5 => Self::LauncherShown,
            6 => Self::LauncherDismissed,
            7 => Self::WorldRequested,
            8 => Self::WorldReady,
            9 => Self::ProducerReady,
            10 => Self::CameraReady,
            11 => Self::MeshAccepted,
            12 => Self::CandidateRejected,
            13 => Self::HostRequested,
            14 => Self::Attached,
            15 => Self::WorkerStarted,
            16 => Self::DeviceCreated,
            17 => Self::UploadStarted,
            18 => Self::BankCompleted,
            19 => Self::BanksResident,
            20 => Self::DrawSubmitted,
            21 => Self::DrawCompleted,
            22 => Self::DrawableRequested,
            23 => Self::DrawableAcquired,
            24 => Self::PresentSubmitted,
            25 => Self::PresentCompleted,
            26 => Self::ProducerRevalidated,
            27 => Self::PublishRequested,
            28 => Self::UiRevealed,
            29 => Self::Removed,
            30 => Self::Cancelled,
            31 => Self::Retired,
            32 => Self::RetryWaiting,
            33 => Self::RetryExhausted,
            34 => Self::SelfTest,
            35 => Self::MeshIdentity,
            36 => Self::MeshOrigin,
            37 => Self::MaterialRevision,
            38 => Self::BankLayout,
            39 => Self::DeviceMetadata,
            40 => Self::TextureLayout,
            41 => Self::UploadSubmitted,
            42 => Self::MemoryCounters,
            _ => return None,
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Emitter {
    Game = 1,
    Worker = 2,
    Swift = 3,
    Ticket = 4,
    Recorder = 5,
}
impl Emitter {
    fn from_code(n: u32) -> Option<Self> {
        Some(match n {
            1 => Self::Game,
            2 => Self::Worker,
            3 => Self::Swift,
            4 => Self::Ticket,
            5 => Self::Recorder,
            _ => return None,
        })
    }
}
#[derive(Clone, Debug)]
pub struct Event {
    pub boot: u64,
    pub seq: u64,
    pub attempt: u64,
    pub world: u64,
    pub nanos: u128,
    pub stage: Stage,
    pub emitter: Emitter,
    pub cause: Option<Cause>,
    pub first: bool,
    pub expected: u64,
    pub observed: u64,
    pub test_only: bool,
    pub last_confirmed: u32,
    pub gpu_inflight: bool,
}
impl Event {
    pub fn json(&self) -> String {
        format!(
            "{{\"schema_version\":2,\"boot_id\":{},\"event_seq\":{},\"attempt_id\":{},\"world_epoch\":{},\"monotonic_ns\":{},\"stage_code\":{},\"stage\":\"{:?}\",\"emitter_code\":{},\"emitter\":\"{:?}\",\"cause_code\":{},\"cause\":\"{:?}\",\"first\":{},\"expected\":{},\"observed\":{},\"test_only\":{},\"last_confirmed_stage\":{},\"gpu_inflight\":{}}}\n",
            self.boot,
            self.seq,
            self.attempt,
            self.world,
            self.nanos,
            self.stage as u32,
            self.stage,
            self.emitter as u32,
            self.emitter,
            self.cause.map(|c| c as u32).unwrap_or(0),
            self.cause,
            self.first,
            self.expected,
            self.observed,
            self.test_only,
            self.last_confirmed,
            self.gpu_inflight
        )
    }
    fn parse(line: &str) -> Option<Self> {
        if !line.ends_with('}') || line.len() > 1024 {
            return None;
        }
        fn n(s: &str, k: &str) -> Option<u64> {
            let key = format!("\"{k}\":");
            s.split_once(&key)?.1.split([',', '}']).next()?.parse().ok()
        }
        if n(line, "schema_version")? != 2 {
            return None;
        }
        let code = u32::try_from(n(line, "cause_code")?).ok()?;
        if code > 41 {
            return None;
        }
        Some(Self {
            boot: n(line, "boot_id")?,
            seq: n(line, "event_seq")?,
            attempt: n(line, "attempt_id")?,
            world: n(line, "world_epoch")?,
            nanos: u128::from(n(line, "monotonic_ns")?),
            stage: Stage::from_code(u32::try_from(n(line, "stage_code")?).ok()?)?,
            emitter: Emitter::from_code(u32::try_from(n(line, "emitter_code")?).ok()?)?,
            cause: if code == 0 {
                None
            } else {
                Some(Cause::from_code(code))
            },
            first: line.contains("\"first\":true"),
            expected: n(line, "expected")?,
            observed: n(line, "observed")?,
            test_only: line.contains("\"test_only\":true"),
            last_confirmed: u32::try_from(n(line, "last_confirmed_stage").unwrap_or(0)).ok()?,
            gpu_inflight: line.contains("\"gpu_inflight\":true"),
        })
    }
}
#[derive(Clone)]
struct State {
    boot: u64,
    seq: u64,
    events: VecDeque<Event>,
    first: Vec<Event>,
    attempts: Vec<(u64, u64)>,
    dropped: u64,
    fields: [u64; 16],
    previous: Vec<Event>,
    recovery: bool,
    torn: u64,
    write_errors: u64,
    blocked: Vec<u64>,
    pending: Vec<(u64, bool)>,
    ended: Vec<u64>,
}
pub struct Recorder {
    state: Arc<Mutex<State>>,
    start: Instant,
    ack: Arc<AtomicU64>,
    failed: Arc<AtomicBool>,
    test_only: bool,
}
impl Recorder {
    pub fn memory() -> Self {
        Self::new(true)
    }
    fn new(test_only: bool) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                boot: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|t| t.as_nanos() as u64)
                    .unwrap_or(1),
                seq: 0,
                events: VecDeque::with_capacity(256),
                first: Vec::with_capacity(3),
                attempts: Vec::new(),
                dropped: 0,
                fields: [0; 16],
                previous: Vec::new(),
                recovery: false,
                torn: 0,
                write_errors: 0,
                blocked: Vec::with_capacity(3),
                pending: Vec::with_capacity(3),
                ended: Vec::with_capacity(3),
            })),
            start: Instant::now(),
            ack: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicBool::new(false)),
            test_only,
        }
    }
    pub fn persistent(path: PathBuf) -> io::Result<Self> {
        Self::persistent_kind(path, false)
    }
    pub fn persistent_kind(path: PathBuf, test_only: bool) -> io::Result<Self> {
        let r = Self::new(test_only);
        let state = Arc::downgrade(&r.state);
        let ack = r.ack.clone();
        let failed = r.failed.clone();
        std::thread::Builder::new()
            .name("helium-flight-writer".into())
            .spawn(move || {
                let mut store = Store::new(path);
                let (old, torn) = store.recover();
                if let Some(state) = state.upgrade() {
                    let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
                    s.recovery = if old.iter().any(|e| e.attempt != 0) {
                        old.iter().filter(|e| e.attempt != 0).any(|e| {
                            !old.iter().any(|v| {
                                v.boot == e.boot
                                    && v.attempt == e.attempt
                                    && (v.stage == Stage::Retired
                                        || (v.stage == Stage::Removed && !v.gpu_inflight))
                            })
                        })
                    } else {
                        !old.is_empty()
                    };
                    s.previous = old;
                    s.torn = torn;
                }
                loop {
                    let Some(shared) = state.upgrade() else { break };
                    let snapshot = shared.lock().unwrap_or_else(|p| p.into_inner()).clone();
                    drop(shared);
                    if snapshot.seq > ack.load(Ordering::Acquire) || failed.load(Ordering::Acquire)
                    {
                        match store.write_snapshot(&snapshot) {
                            Ok(()) => {
                                ack.store(snapshot.seq, Ordering::Release);
                                failed.store(false, Ordering::Release);
                            }
                            Err(_) => {
                                failed.store(true, Ordering::Release);
                                if let Some(state) = state.upgrade() {
                                    let mut s = state.lock().unwrap_or_else(|p| p.into_inner());
                                    s.write_errors = s.write_errors.saturating_add(1);
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            })?;
        r.record(0, Stage::Boot, Emitter::Recorder, None, 0, 0);
        Ok(r)
    }
    pub fn allocate(&self, world: u64) -> Option<u64> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .ok()?;
        self.begin_world(id, world).then_some(id)
    }
    pub fn begin(&self, id: u64) {
        let _ = self.begin_world(id, 0);
    }
    pub fn begin_world(&self, id: u64, world: u64) -> bool {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if id == 0 || s.attempts.iter().any(|(a, _)| *a == id) || s.attempts.len() >= 3 {
            return false;
        }
        s.attempts.push((id, world));
        s.pending.push((id, false));
        s.fields[0] = 0;
        s.fields[1] = 0;
        s.fields[8] = 0;
        true
    }
    pub fn known(&self, id: u64) -> bool {
        id == 0
            || self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .attempts
                .iter()
                .any(|a| a.0 == id)
    }
    pub fn retry_safe(&self, id: u64) -> bool {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        !s.blocked.contains(&id)
            && !s.events.iter().any(|e| {
                e.attempt == id
                    && matches!(
                        e.cause,
                        Some(
                            Cause::WorldChanged
                                | Cause::AppInactive
                                | Cause::MemoryPressure
                                | Cause::GpuTimeout
                                | Cause::DeviceFailure
                                | Cause::RendererChanged
                                | Cause::ResourceReload
                                | Cause::Shutdown
                                | Cause::Panic
                        )
                    )
            })
    }
    pub fn current(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .attempts
            .last()
            .map(|a| a.0)
            .unwrap_or(0)
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
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let world = if id == 0 {
            0
        } else {
            let Some((_, w)) = s.attempts.iter().find(|a| a.0 == id) else {
                return;
            };
            *w
        };
        let ended = s.ended.contains(&id);
        if id != 0 && matches!(stage, Stage::Removed | Stage::Retired) && !ended {
            s.ended.push(id);
        }
        if s.events.back().is_some_and(|e| {
            e.attempt == id
                && e.stage == stage
                && e.emitter == emitter
                && e.cause == cause
                && e.expected == expected
                && e.observed == observed
        }) {
            return;
        }
        if id == 0
            && cause.is_some_and(|c| {
                matches!(
                    c,
                    Cause::AppInactive
                        | Cause::MemoryPressure
                        | Cause::Shutdown
                        | Cause::WorldChanged
                )
            })
        {
            let ids = s.attempts.iter().map(|a| a.0).collect::<Vec<_>>();
            for id in ids {
                if !s.blocked.contains(&id) {
                    s.blocked.push(id);
                }
            }
        }
        if id != 0
            && cause.is_some_and(|c| {
                matches!(
                    c,
                    Cause::WorldChanged
                        | Cause::AppInactive
                        | Cause::MemoryPressure
                        | Cause::GpuTimeout
                        | Cause::DeviceFailure
                        | Cause::RendererChanged
                        | Cause::ResourceReload
                        | Cause::Shutdown
                        | Cause::Panic
                )
            })
            && !s.blocked.contains(&id)
        {
            s.blocked.push(id);
        }
        let first = !ended
            && id != 0
            && !matches!(emitter, Emitter::Ticket)
            && cause.is_some_and(|c| {
                !matches!(
                    c,
                    Cause::SnapshotExpired | Cause::NoFreshMesh | Cause::RetryExhausted
                )
            })
            && !s.first.iter().any(|e| e.attempt == id);
        let Some(seq) = s.seq.checked_add(1) else {
            return;
        };
        s.seq = seq;
        let last_confirmed = s
            .events
            .iter()
            .rev()
            .find(|e| e.attempt == id)
            .map(|e| {
                if matches!(
                    e.stage,
                    Stage::Cancelled | Stage::Removed | Stage::RetryWaiting | Stage::RetryExhausted
                ) || e.stage as u32 >= 35
                {
                    e.last_confirmed
                } else {
                    e.stage as u32
                }
            })
            .unwrap_or(0);
        if let Some((_, pending)) = s.pending.iter_mut().find(|p| p.0 == id) {
            if matches!(
                stage,
                Stage::UploadSubmitted | Stage::DrawSubmitted | Stage::PresentSubmitted
            ) {
                *pending = true;
            }
            if matches!(
                stage,
                Stage::BankCompleted
                    | Stage::DrawCompleted
                    | Stage::PresentCompleted
                    | Stage::Retired
            ) {
                *pending = false;
            }
        }
        let gpu_inflight = s.pending.iter().any(|p| p.0 == id && p.1);
        let e = Event {
            boot: s.boot,
            seq,
            attempt: id,
            world,
            nanos: self.start.elapsed().as_nanos(),
            stage,
            emitter,
            cause,
            first,
            expected,
            observed,
            test_only: self.test_only,
            last_confirmed,
            gpu_inflight,
        };
        if id == 0 || s.attempts.last().is_some_and(|a| a.0 == id) {
            match stage {
                Stage::UploadStarted => {
                    s.fields[1] = observed;
                }
                Stage::BankCompleted => {
                    s.fields[0] += 1;
                }
                Stage::MaterialRevision => {
                    s.fields[4] = expected;
                }
                Stage::CameraReady => {
                    s.fields[2] = expected;
                    s.fields[3] = observed;
                }
                Stage::MeshAccepted => {
                    s.fields[5] = expected;
                    s.fields[6] = observed;
                }
                Stage::UiRevealed => {
                    s.fields[8] = 1;
                }

                Stage::ModeSelected => {
                    s.fields[9] = observed;
                }
                Stage::SelfTest => {
                    s.fields[10] = observed;
                }
                _ => {}
            }
        }
        if id != 0 && s.attempts.last().is_some_and(|a| a.0 == id) {
            s.fields[7] = world;
        }
        if first {
            s.first.push(e.clone());
        } // reserved priority: never evicted by the timeline
        if s.events.len() == 256 {
            s.events.pop_front();
            s.dropped = s.dropped.saturating_add(1);
        }
        s.events.push_back(e);
    }
    pub fn snapshot(&self) -> Vec<Event> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .events
            .iter()
            .cloned()
            .collect()
    }
    pub fn first(&self, id: u64) -> Option<Cause> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .first
            .iter()
            .find(|e| e.attempt == id)
            .and_then(|e| e.cause)
    }
    /// Fixed ABI v2 snapshot; all elements u64, owned copy, no pointers/strings.
    pub fn status(&self) -> [u64; 32] {
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let id = s.attempts.last().map(|a| a.0).unwrap_or(0);
        let e = s.events.iter().rev().find(|e| e.attempt == id);
        let first = s.first.iter().find(|e| e.attempt == id);
        [
            2,
            s.boot,
            id,
            if first.is_some() {
                Stage::Cancelled as u64
            } else {
                e.map(|e| e.stage as u64).unwrap_or(1)
            },
            first.and_then(|e| e.cause).map(|c| c as u64).unwrap_or(0),
            s.seq,
            self.ack.load(Ordering::Acquire),
            s.dropped,
            u64::from(self.failed.load(Ordering::Acquire) || s.torn != 0),
            s.attempts.len() as u64,
            self.start.elapsed().as_millis() as u64,
            e.map(|e| e.expected).unwrap_or(0),
            e.map(|e| e.observed).unwrap_or(0),
            u64::from(s.recovery),
            s.torn,
            u64::from(self.test_only),
            s.fields[0],
            s.fields[1],
            s.fields[2],
            s.fields[3],
            s.fields[4],
            s.fields[5],
            s.fields[6],
            s.fields[7],
            s.fields[8],
            s.fields[9],
            s.fields[10],
            s.write_errors,
            u64::from(s.pending.iter().any(|p| p.0 == id && p.1)),
            s.fields[13],
            s.fields[14],
            s.fields[15],
        ]
    }
    pub fn wait_ack(&self, timeout: Duration) -> bool {
        let target = self.status()[5];
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if self.ack.load(Ordering::Acquire) >= target {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }
}
pub struct Store {
    path: PathBuf,
    summary: VecDeque<Event>,
}
impl Store {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            summary: VecDeque::new(),
        }
    }
    fn recover(&mut self) -> (Vec<Event>, u64) {
        use std::io::Read;
        let mut bytes = Vec::new();
        if let Ok(file) = fs::File::open(self.path.join("helium-metal-events.jsonl")) {
            let _ = file.take(512 * 1024 + 1).read_to_end(&mut bytes);
        }
        let mut events = Vec::new();
        let mut torn = 0;
        for line in bytes.split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }
            match std::str::from_utf8(line).ok().and_then(Event::parse) {
                Some(e) => events.push(e),
                None => torn += 1,
            }
        }
        if events.len() > 259 {
            events.drain(..events.len() - 259);
        }
        if !events.is_empty() {
            let normalized = events.iter().map(Event::json).collect::<String>();
            if fs::create_dir_all(&self.path)
                .and_then(|_| self.replace("helium-metal-previous.jsonl", normalized.as_bytes()))
                .is_err()
            {
                torn += 1;
            }
        }
        (events, torn)
    }
    fn replace(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > 512 * 1024 {
            return Err(io::Error::other("report capacity"));
        }
        fs::create_dir_all(&self.path)?;
        let temp = self.path.join(format!("{name}.pending"));
        fs::write(&temp, bytes)?;
        fs::rename(temp, self.path.join(name))
    }
    fn write_snapshot(&self, s: &State) -> io::Result<()> {
        let mut events = s.events.iter().cloned().collect::<Vec<_>>();
        for e in &s.first {
            if !events.iter().any(|v| v.seq == e.seq) {
                events.push(e.clone());
            }
        }
        events.sort_by_key(|e| e.seq);
        let json = events.iter().map(Event::json).collect::<String>();
        self.replace("helium-metal-events.jsonl", json.as_bytes())?;
        let mut report = format!(
            "HELIUM METAL G06-D DIAGNOSTIC\nSCHEMA 2 | source={}\nboot={} | TEST_ONLY={}\nrecorder_status={} | persisted_sequence={} | lost_timeline_events={}\nPREVIOUS_BOOT_INCOMPLETE={} | malformed_previous_records={}\nWGpu FALLBACK ACTIVE | PHYSICAL_PIXELS_UNVERIFIED\nPersistence acknowledgement is not a power-loss/fsync guarantee.\nSELF_TEST: typed initialization/serialization running; GPU/UI checks listed separately.\n",
            option_env!("HELIUM_DIAGNOSTIC_SHA").unwrap_or("development-unidentified"),
            s.boot,
            events.first().map(|e| e.test_only).unwrap_or(false),
            if s.torn == 0 { "OK" } else { "DEGRADED" },
            s.seq,
            s.dropped,
            s.recovery,
            s.torn
        );
        report.push_str(&format!(
            "chronology_complete={}\n",
            s.dropped == 0 && s.torn == 0
        ));
        report.push_str(&format!("write_errors_total={}\n", s.write_errors));
        report.push_str("FIRST CAUSES (priority retained, at most 3):\n");
        for e in &s.first {
            report.push_str(&format!("Attempt {} FIRST CAUSE {:?} emitter={:?} last confirmed stage before cancel retained in timeline\n",e.attempt,e.cause,e.emitter));
            let last = Stage::from_code(e.last_confirmed);
            report.push_str(&format!("Last confirmed stage at first cause: {last:?}\n"));
            report.push_str(&format!(
                "Possible GPU work pending at first cause: {}\n",
                e.gpu_inflight
            ));
            report.push_str(&e.json());
        }
        report.push_str("PREVIOUS BOOT (normalized allowlisted fields only):\n");
        for e in s.previous.iter().filter(|e| e.first) {
            report.push_str(&format!(
                "Previous boot {} attempt {} FIRST CAUSE {:?} emitter={:?} seq={}\n",
                e.boot, e.attempt, e.cause, e.emitter, e.seq
            ));
        }
        for e in s.previous.iter().rev().take(80).rev() {
            report.push_str(&e.json());
        }
        report.push_str("CURRENT TIMELINE:\n");
        for e in events {
            report.push_str(&e.json());
        }
        report.push_str("NEXT ACTION FOR CODEX: use the first cause above; if none, PENDING_DEVICE_CAPTURE.\nScreenshot/recording attached? OWNER INPUT: YES / NO\nNo scanout, FPS or continuous gameplay claim.\n");
        if report.len() > 256 * 1024 {
            return Err(io::Error::other("report capacity"));
        }
        self.replace("Helium-Metal-Report.txt", report.as_bytes())
    }
    pub fn append(&mut self, e: &Event) -> io::Result<()> {
        if self.summary.len() == 256 {
            self.summary.pop_front();
        }
        self.summary.push_back(e.clone());
        let first = self
            .summary
            .iter()
            .filter(|e| e.first)
            .take(3)
            .cloned()
            .collect();
        let s = State {
            boot: e.boot,
            seq: e.seq,
            events: self.summary.clone(),
            first,
            attempts: vec![],
            dropped: 0,
            fields: [0; 16],
            previous: vec![],
            recovery: false,
            torn: 0,
            write_errors: 0,
            blocked: vec![],
            pending: vec![],
            ended: vec![],
        };
        self.write_snapshot(&s)
    }
}
static GLOBAL: OnceLock<Recorder> = OnceLock::new();
pub fn initialize(path: PathBuf, test_only: bool) -> bool {
    GLOBAL.get_or_init(|| {
        Recorder::persistent_kind(path, test_only).unwrap_or_else(|_| {
            let r = Recorder::memory();
            r.failed.store(true, Ordering::Release);
            r
        })
    });
    true
}
pub fn global() -> &'static Recorder {
    GLOBAL.get_or_init(|| {
        #[cfg(target_os = "ios")]
        {
            Recorder::persistent(std::env::current_dir().unwrap_or_default().join("logs"))
                .unwrap_or_else(|_| Recorder::memory())
        }
        #[cfg(not(target_os = "ios"))]
        {
            Recorder::memory()
        }
    })
}
/// Context-only events use id0; NEVER relabel a late attempt as the newest one.
pub fn emit(stage: Stage, emitter: Emitter, cause: Option<Cause>, expected: u64, observed: u64) {
    global().record(0, stage, emitter, cause, expected, observed)
}

pub fn retry_allowed(
    cause: Cause,
    attempts: u32,
    retired: bool,
    foreground: bool,
    fresh: bool,
    elapsed: Duration,
) -> bool {
    cause.recoverable()
        && attempts < 3
        && retired
        && foreground
        && fresh
        && elapsed < Duration::from_secs(120)
}
#[cfg(test)]
mod tests;
