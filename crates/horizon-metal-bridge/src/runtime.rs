use crate::adapter::{
    self,
    material_adapter::{BankSource, Binding},
};
use helium_metal_prototype::flight_recorder::{self as recorder, Cause, Emitter, Stage};
use helium_metal_prototype::{
    section::{Format, Limits},
    terrain::{Mesh, View},
    upload::{self, Source},
};
use horizon_client_render::{
    gfx::{Frame, Renderer, material_snapshot::Generation},
    mesher::SectionMesh,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::Instant,
};
type Key = (i32, i32, i32);
pub const SNAPSHOT_BUDGET: usize = 64 * 1024 * 1024;
const GEOMETRY_BUDGET: usize = 2 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Disabled,
    OneSectionDiagnostic,
    AttachedOneSection,
}
impl Mode {
    fn observes_surface(self, offscreen: bool) -> bool {
        self != Self::Disabled && !(self == Self::AttachedOneSection && offscreen)
    }
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("") | Some("off") => Ok(Self::Disabled),
            Some("one-section") => Ok(Self::OneSectionDiagnostic),
            Some("attached-one-section") => Ok(Self::AttachedOneSection),
            Some(_) => Err("unsupported HELIUM_DIRECT_METAL value; use one-section, attached-one-section or off".into()),
        }
    }
}
fn selected_mode() -> Result<Mode, String> {
    match std::env::var("HELIUM_DIRECT_METAL") {
        Ok(value) => return Mode::parse(Some(&value)),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("non-UTF8 native mode rejected".into());
        }
        Err(std::env::VarError::NotPresent) => {}
    }
    #[cfg(target_os = "ios")]
    return match helium_metal_prototype::attached::device_test_mode()? {
        0 => Ok(Mode::Disabled),
        2 => Ok(Mode::AttachedOneSection),
        _ => Err("invalid test-only sandbox configuration".into()),
    };
    #[cfg(not(target_os = "ios"))]
    Ok(Mode::Disabled)
}
pub fn enabled() -> bool {
    Controller::observe_surface(false)
}
#[derive(Default, Debug)]
struct Counters {
    accepted: u64,
    rejected: u64,
    busy: u64,
    replaced: u64,
    unloaded: u64,
    invalidated: u64,
    vertices: usize,
    indices: usize,
    preparation_us: u128,
}
struct Geometry {
    key: Key,
    origin: [i32; 3],
    generation: Generation,
    sequence: u64,
    vertices: Vec<u8>,
    indices: Vec<u32>,
}
impl Geometry {
    fn capture(
        key: Key,
        min_y: i32,
        mesh: &SectionMesh,
        generation: Generation,
        sequence: u64,
        layers: usize,
    ) -> Result<Option<Self>, String> {
        let origin = section_origin(key, min_y)?;
        let mut indices = Vec::new();
        let export = adapter::expanded_solid(
            mesh,
            origin,
            Format::BakedSectionSolid,
            Limits {
                max_buffer_bytes: GEOMETRY_BUDGET,
                geometry_bytes: GEOMETRY_BUDGET,
                texture_layers: layers,
            },
            &mut indices,
        )?;
        let Some(prepared) = export.solid.mesh() else {
            return Ok(None);
        };
        let mut vertices = Vec::new();
        vertices
            .try_reserve_exact(prepared.vertices.len())
            .map_err(|_| "section retention allocation failed")?;
        if vertices.capacity() != prepared.vertices.len() {
            return Err("unexpected retained geometry capacity".into());
        }
        vertices.extend_from_slice(prepared.vertices);
        if indices
            .capacity()
            .checked_mul(4)
            .and_then(|n| n.checked_add(vertices.capacity()))
            .is_none_or(|n| n > GEOMETRY_BUDGET)
        {
            return Err("retained geometry capacity exceeds policy".into());
        }
        Ok(Some(Self {
            key,
            origin,
            generation,
            sequence,
            vertices,
            indices,
        }))
    }
    fn mesh(&self) -> Mesh<'_> {
        Mesh {
            vertices: &self.vertices,
            indices: &self.indices,
        }
    }
}
fn production_layers(
    snapshot: &horizon_client_render::gfx::material_snapshot::Snapshot,
) -> Result<usize, String> {
    if snapshot.split() == 0 || snapshot.split() > 2048 || snapshot.banks().len() > 8 {
        return Err("unsupported production namespace".into());
    }
    Ok(snapshot.sprites() as usize)
}

fn section_origin(key: Key, min_y: i32) -> Result<[i32; 3], String> {
    Ok([
        key.0.checked_mul(16).ok_or("section X overflow")?,
        key.1
            .checked_mul(16)
            .and_then(|n| n.checked_add(min_y))
            .ok_or("section Y overflow")?,
        key.2.checked_mul(16).ok_or("section Z overflow")?,
    ])
}
fn camera_view(origin: [i32; 3], camera: [f64; 3], clip: [[f32; 4]; 4]) -> Result<View, String> {
    let relative = std::array::from_fn(|i| (f64::from(origin[i]) - camera[i]) as f32);
    if !camera.iter().all(|v| v.is_finite())
        || !relative
            .iter()
            .chain(clip.iter().flatten())
            .all(|v| v.is_finite())
    {
        return Err("invalid game camera".into());
    }
    Ok(View {
        origin: relative,
        clip_from_relative: clip,
    })
}
/// Conservative geometry/frustum intersection in Metal's 0..w depth space.
/// Uses ORIGINAL positions, including model overhang beyond the 16-block section.
/// A crossing triangle can be visible with no vertex inside; reject only if all
/// vertices are outside the same plane. This does not prove occlusion visibility.
fn points_visible(view: View, points: impl IntoIterator<Item = [f32; 3]>) -> bool {
    let mut outside = [true; 6];
    for position in points {
        let p = [
            position[0] + view.origin[0],
            position[1] + view.origin[1],
            position[2] + view.origin[2],
            1.,
        ];
        let c: [f32; 4] =
            std::array::from_fn(|r| (0..4).map(|i| view.clip_from_relative[i][r] * p[i]).sum());
        if !c.iter().all(|v| v.is_finite()) {
            return false;
        }
        for (i, d) in [
            c[0] + c[3],
            c[3] - c[0],
            c[1] + c[3],
            c[3] - c[1],
            c[2],
            c[3] - c[2],
        ]
        .into_iter()
        .enumerate()
        {
            outside[i] &= d < 0.;
        }
    }
    !outside.into_iter().any(|v| v)
}
struct Flight {
    geometry: Arc<Geometry>,
    source: Vec<BankSource>,
    cancel: Arc<AtomicBool>,
    light_revision: u64,
    size: [u32; 2],
    receiver: Receiver<Result<String, String>>,
    #[cfg(target_os = "ios")]
    attached: Option<helium_metal_prototype::attached::Ticket>,
}
#[cfg(target_os = "ios")]
struct Shown {
    _ticket: helium_metal_prototype::attached::Ticket,
    source: Vec<BankSource>,
    key: Key,
    light_revision: u64,
    size: [u32; 2],
}
/// One candidate and one worker, never a world mirror or mesh queue. This mode
/// keeps wgpu as producer/fallback. Attached mode uses a distinct UIKit child layer.
pub struct Controller {
    mode: Mode,
    candidate: Option<Geometry>,
    flight: Option<Flight>,
    sequence: u64,
    world_epoch: u64,
    finished: bool,
    counters: Counters,
    last_error: Option<String>,
    window: Option<Arc<winit::window::Window>>,
    camera: Option<([f64; 3], [[f32; 4]; 4])>,
    #[cfg(target_os = "ios")]
    displayed: Option<Shown>,
}
impl Controller {
    pub fn observe_surface(offscreen: bool) -> bool {
        cfg!(any(target_os = "macos", target_os = "ios"))
            && selected_mode().is_ok_and(|m| {
                m.observes_surface(offscreen)
                    && (m != Mode::AttachedOneSection || cfg!(target_os = "ios"))
            })
    }
    /// Local runtime session counter, never a save ID or pointer identity.
    pub fn world_changed(&mut self) {
        self.invalidate("world replaced");
        match self.world_epoch.checked_add(1) {
            Some(epoch) => {
                self.world_epoch = epoch;
                log::info!("Helium native world session epoch={epoch}");
            }
            None => {
                self.finished = true;
                self.error("world epochs exhausted; restart diagnostic session".into());
            }
        }
    }
    pub fn from_environment() -> Option<Self> {
        let mode = match selected_mode() {
            Ok(m) => m,
            Err(e) => {
                log::warn!("Helium Direct Metal: {e}; wgpu retained");
                return None;
            }
        };
        if mode == Mode::Disabled {
            return None;
        }
        if !cfg!(any(target_os = "macos", target_os = "ios")) {
            log::warn!("Helium Direct Metal requires Apple; wgpu retained");
            return None;
        }
        if mode == Mode::AttachedOneSection && !cfg!(target_os = "ios") {
            log::warn!("attached-one-section requires the actual iOS game host; wgpu retained");
            return None;
        }
        log::info!("Helium Direct Metal selected={mode:?}; wgpu producer/fallback retained");
        Some(Self::new(mode))
    }
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            candidate: None,
            flight: None,
            sequence: 0,
            world_epoch: 0,
            finished: false,
            counters: Default::default(),
            last_error: None,
            window: None,
            camera: None,
            #[cfg(target_os = "ios")]
            displayed: None,
        }
    }
    pub fn set_window(&mut self, window: Option<Arc<winit::window::Window>>) {
        if self
            .window
            .as_ref()
            .is_some_and(|old| window.as_ref().is_none_or(|new| old.id() != new.id()))
        {
            self.invalidate("actual game window changed/destroyed");
        }
        self.window = window;
    }
    fn error(&mut self, error: String) {
        self.counters.rejected += 1;
        if self.last_error.as_ref() != Some(&error) {
            log::warn!("Helium Direct Metal rejected: {error}; wgpu retained");
            self.last_error = Some(error);
        }
    }
    pub fn invalidate(&mut self, reason: &str) {
        recorder::emit(
            Stage::Cancelled,
            Emitter::Game,
            Some(Cause::reason(reason)),
            0,
            0,
        );
        self.candidate = None;
        self.camera = None;
        #[cfg(target_os = "ios")]
        {
            self.displayed = None;
        }
        if let Some(f) = &self.flight {
            f.cancel.store(true, Ordering::Release);
            #[cfg(target_os = "ios")]
            if let Some(t) = &f.attached {
                t.cancel();
            }
        }
        self.counters.invalidated += 1;
        log::debug!("Helium Direct Metal revoked: {reason}");
    }
    /// Called ONLY for accepted Streaming::update results before their storage is
    /// returned to Streaming::free. Streamer has already rejected stale job seqs.
    pub fn section(&mut self, renderer: &Renderer, key: Key, min_y: i32, mesh: &SectionMesh) {
        #[cfg(target_os = "ios")]
        if self.displayed.as_ref().is_some_and(|d| d.key == key) {
            self.invalidate("displayed section replaced");
        }
        if self.mode == Mode::Disabled || self.finished {
            return;
        }
        if self.mode == Mode::AttachedOneSection && self.camera.is_none() {
            self.counters.busy += 1; // wait for a genuine world camera, never a title/old-world transform
            return;
        }
        self.sequence = match self.sequence.checked_add(1) {
            Some(n) => n,
            None => {
                self.invalidate("mesh sequence exhausted");
                self.finished = true;
                return;
            }
        };
        if self.flight.as_ref().is_some_and(|f| f.geometry.key == key) {
            self.invalidate("in-flight section replaced");
        }
        if self.flight.is_some() {
            self.counters.busy += 1;
            return;
        }
        if let Some((camera, clip)) = self.camera.filter(|_| !mesh.solid.is_empty()) {
            let visible = section_origin(key, min_y)
                .and_then(|o| camera_view(o, camera, clip))
                .is_ok_and(|v| points_visible(v, mesh.solid.iter().map(|p| p.pos)));
            if !visible {
                self.counters.rejected += 1;
                return;
            }
        }
        if self.candidate.as_ref().is_some_and(|c| c.key != key) {
            self.counters.busy += 1;
            return;
        }
        if self.candidate.take().is_some() {
            self.counters.replaced += 1;
        }
        let start = Instant::now();
        let result = renderer
            .validate_material_snapshot_mode()
            .map_err(str::to_owned)
            .and_then(|_| {
                let state = renderer.material_state();
                let snapshot = state
                    .current(state.snapshot().map_err(str::to_owned)?)
                    .map_err(str::to_owned)?;
                Geometry::capture(
                    key,
                    min_y,
                    mesh,
                    state.generation(),
                    self.sequence,
                    production_layers(snapshot)?,
                )
            });
        self.counters.preparation_us += start.elapsed().as_micros();
        match result {
            Ok(Some(g)) => {
                if self.counters.accepted == 0 {
                    log::info!(
                        "Helium native checkpoint: accepted actual Streaming::update SectionMesh key={:?} sequence={} vertices={} indices={}",
                        g.key,
                        g.sequence,
                        g.vertices.len() / 24,
                        g.indices.len()
                    );
                }
                self.counters.accepted += 1;
                self.counters.vertices = g.vertices.len() / 24;
                self.counters.indices = g.indices.len();
                self.candidate = Some(g)
            }
            Ok(None) => {}
            Err(e) => self.error(e),
        }
    }
    pub fn remove_columns(&mut self, columns: impl Iterator<Item = (i32, i32)>) {
        for key in columns {
            #[cfg(target_os = "ios")]
            if self
                .displayed
                .as_ref()
                .is_some_and(|d| (d.key.0, d.key.2) == key)
            {
                self.invalidate("displayed column unloaded");
            }
            if self
                .candidate
                .as_ref()
                .is_some_and(|g| (g.key.0, g.key.2) == key)
            {
                self.candidate = None;
                self.counters.unloaded += 1;
            }
            if self
                .flight
                .as_ref()
                .is_some_and(|f| (f.geometry.key.0, f.geometry.key.2) == key)
            {
                self.invalidate("column unloaded");
                self.counters.unloaded += 1;
            }
        }
    }
    /// Called after production render generates this frame's lightmap/animation
    /// state. Uses exact game view/projection/bob/spin, not a diagnostic camera.
    pub fn frame(&mut self, renderer: &Renderer, frame: &Frame<'_>) {
        if self.mode == Mode::Disabled {
            return;
        }
        #[cfg(target_os = "ios")]
        if self.displayed.as_ref().is_some_and(|d| {
            d._ticket.is_cancelled()
                || d.source.iter().any(|s| s.check_current(renderer).is_err())
                || renderer.material_state().light_revision() != Ok(d.light_revision)
                || d.size != [renderer.config.width, renderer.config.height]
        }) {
            self.invalidate("displayed producer state changed");
        }
        self.camera = (!frame.title).then(|| {
            (
                frame.camera.to_array(),
                renderer
                    .views(frame)
                    .primary()
                    .view_proj(frame.bob * frame.spin)
                    .to_cols_array_2d(),
            )
        });
        if let Some(f) = &self.flight {
            if f.source.iter().any(|s| s.check_current(renderer).is_err())
                || renderer.material_state().light_revision() != Ok(f.light_revision)
                || f.size != [renderer.config.width, renderer.config.height]
            {
                let cause = if f.source.iter().any(|s| s.check_current(renderer).is_err()) {
                    Cause::BankStale
                } else if renderer.material_state().light_revision() != Ok(f.light_revision) {
                    Cause::LightmapChanged
                } else {
                    Cause::FrameSizeChanged
                };
                recorder::emit(
                    Stage::Cancelled,
                    Emitter::Game,
                    Some(cause),
                    if cause == Cause::LightmapChanged {
                        f.light_revision
                    } else {
                        0
                    },
                    if cause == Cause::LightmapChanged {
                        renderer.material_state().light_revision().unwrap_or(0)
                    } else {
                        0
                    },
                );
                f.cancel.store(true, Ordering::Release);
            }
            match f.receiver.try_recv() {
                Ok(result) => {
                    #[allow(unused_mut)]
                    let mut f = self.flight.take().unwrap();
                    let valid = !f.cancel.load(Ordering::Acquire)
                        && f.source.iter().all(|s| s.check_current(renderer).is_ok())
                        && renderer.material_state().light_revision() == Ok(f.light_revision);
                    self.finished = true;
                    match result {
                        Ok(evidence) if valid => {
                            #[cfg(target_os = "ios")]
                            if let Some(t) = f.attached.take() {
                                t.publish();
                                self.displayed = Some(Shown {
                                    _ticket: t,
                                    source: f.source,
                                    key: f.geometry.key,
                                    light_revision: f.light_revision,
                                    size: f.size,
                                });
                            }
                            log::info!(
                                "Helium Direct Metal CATEGORY D handoff completed: {evidence}; {:?}; scanout NOT established",
                                self.counters
                            );
                        }
                        Ok(_) => self.error(
                            "completed historical worker revoked by live producer/mesh".into(),
                        ),
                        Err(e) => self.error(e),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.flight = None;
                    self.finished = true;
                    self.error("native worker disconnected".into())
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            return;
        }
        if self.finished || frame.title {
            return;
        }
        let Some(g) = self.candidate.take() else {
            return;
        };
        if g.generation != renderer.material_state().generation() {
            self.error("candidate belongs to old resource/renderer generation".into());
            return;
        }
        let prepare = (|| {
            let binding = Binding::new(renderer)?;
            let snapshot = renderer
                .material_state()
                .snapshot()
                .map_err(str::to_owned)?;
            let required = helium_metal_prototype::banks::required(
                &g.mesh(),
                snapshot.split(),
                snapshot.sprites(),
            )?;
            let source = required
                .iter()
                .map(|&b| binding.upload_source(renderer, b))
                .collect::<Result<Vec<_>, _>>()?;
            let validation = required
                .iter()
                .map(|&b| binding.upload_source(renderer, b))
                .collect::<Result<Vec<_>, _>>()?;
            let light_revision = renderer
                .material_state()
                .light_revision()
                .map_err(str::to_owned)?;
            if renderer
                .lightmap_pixels
                .chunks_exact(4)
                .any(|p| p[3] != 255)
            {
                return Err("production lightmap uninitialized".into());
            }
            let clip = renderer
                .views(frame)
                .primary()
                .view_proj(frame.bob * frame.spin)
                .to_cols_array_2d();
            let view = camera_view(g.origin, frame.camera.to_array(), clip)?;
            if !points_visible(
                view,
                (0..g.vertices.len() / 24)
                    .map(|i| g.mesh().vertex(i).map(|v| v.pos).unwrap_or([f32::NAN; 3])),
            ) {
                return Err("candidate outside current camera frustum; waiting for visible accepted section".into());
            }
            log::info!(
                "Helium native source accepted key={:?} sequence={} generation={:?} light_revision={} camera={:?} size={}x{}",
                g.key,
                g.sequence,
                g.generation,
                light_revision,
                frame.camera,
                renderer.config.width,
                renderer.config.height
            );
            Ok((
                source,
                validation,
                light_revision,
                renderer.lightmap_pixels,
                view,
            ))
        })();
        let (source, validation, light_revision, lightmap, view) = match prepare {
            Ok(p) => p,
            Err(e) => {
                self.error(e);
                return;
            }
        };
        let geometry = Arc::new(g);
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            world_epoch: self.world_epoch,
            source,
            geometry: geometry.clone(),
            cancel: cancel.clone(),
            lightmap,
            view,
            size: [renderer.config.width, renderer.config.height],
        };
        #[cfg(target_os = "ios")]
        if self.mode == Mode::AttachedOneSection {
            let Some(window) = &self.window else {
                self.error("actual game window unavailable".into());
                return;
            };
            match helium_metal_prototype::attached::submit(
                window.as_ref(),
                cancel.clone(),
                move |layer| run_impl(job, Some(layer)),
            ) {
                Ok((ticket, receiver)) => {
                    self.flight = Some(Flight {
                        geometry,
                        source: validation,
                        cancel,
                        light_revision,
                        size: [renderer.config.width, renderer.config.height],
                        receiver,
                        attached: Some(ticket),
                    })
                }
                Err(e) => self.error(e),
            }
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("helium-real-section-metal".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job)))
                    .unwrap_or_else(|_| Err("native worker panicked".into()));
                let _ = sender.send(outcome);
            }) {
            Ok(_) => {
                self.flight = Some(Flight {
                    geometry,
                    source: validation,
                    cancel,
                    light_revision,
                    size: [renderer.config.width, renderer.config.height],
                    receiver,
                    #[cfg(target_os = "ios")]
                    attached: None,
                })
            }
            Err(e) => self.error(format!("native worker launch failed: {e}")),
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        recorder::emit(Stage::Cancelled, Emitter::Game, Some(Cause::Shutdown), 0, 0);
        if let Some(f) = &self.flight {
            f.cancel.store(true, Ordering::Release);
        }
    }
}
struct Job {
    world_epoch: u64,
    source: Vec<BankSource>,
    geometry: Arc<Geometry>,
    cancel: Arc<AtomicBool>,
    lightmap: [u8; 1024],
    view: View,
    size: [u32; 2],
}
/// Versioned revocable CPU lease, NOT production global GPU authority. The live
/// Renderer authorizes before dispatch and rechecks after completion on its thread.
/// Worker success describes historical GPU bytes until that final check passes.
struct LeasedSource {
    bank: BankSource,
    geometry: Arc<Geometry>,
}
impl Source for LeasedSource {
    type Generation = Generation;
    type Authority = AtomicBool;
    fn description(&self) -> upload::Description<Generation> {
        let mut d = self.bank.description();
        d.version = upload::BANKED_VERSION;
        d
    }
    fn pixels(&self) -> &[u8] {
        self.bank.pixels()
    }
    fn retained_bytes(&self) -> usize {
        self.geometry
            .indices
            .capacity()
            .checked_mul(4)
            .and_then(|n| n.checked_add(self.geometry.vertices.capacity()))
            .and_then(|n| n.checked_add(self.bank.retained_bytes()))
            .unwrap_or(usize::MAX)
    }
    fn retention_key(&self) -> Option<(usize, usize)> {
        self.bank
            .retention_key()
            .map(|k| (k.0, Arc::as_ptr(&self.geometry) as usize))
    }
    fn check_current(&self, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Acquire) {
            Err("game producer/mesh lease revoked".into())
        } else {
            Ok(())
        }
    }
}
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn run(_job: Job) -> Result<String, String> {
    Err("direct Metal requires Apple; no native submission".into())
}
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn run(job: Job) -> Result<String, String> {
    run_impl(
        job,
        #[cfg(target_os = "ios")]
        None,
    )
}
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn run_impl(
    job: Job,
    #[cfg(target_os = "ios")] attached: Option<helium_metal_prototype::attached::Layer>,
) -> Result<String, String> {
    use helium_metal_prototype::{
        frame,
        terrain::{Budget, Options, TerrainRenderer},
    };
    use std::time::Duration;
    let deadline = Instant::now() + Duration::from_secs(30);
    let pause = || -> Result<(), String> {
        if Instant::now() >= deadline {
            return Err("native diagnostic deadline; submitted resources retire through G05 completion owners".into());
        }
        std::thread::sleep(Duration::from_millis(2));
        Ok(())
    };
    let renderer = TerrainRenderer::new()?;
    let sources: Vec<_> = job
        .source
        .into_iter()
        .map(|bank| LeasedSource {
            bank,
            geometry: job.geometry.clone(),
        })
        .collect();
    let descriptions: Vec<_> = sources.iter().map(Source::description).collect();
    let budget = upload::Budget {
        payload_bytes: 192 * 1024 * 1024,
        staging_bytes: 16 * 1024 * 1024,
    };
    let mut material = renderer.create_material_set(&sources, &job.cancel, budget)?;
    for source in sources {
        let bank = source.description().bank;
        material.bank_mut(bank)?.begin_upload(
            &renderer,
            source,
            &job.cancel,
            upload::Update::Full,
            budget,
        )?;
        loop {
            match material.bank_mut(bank)?.poll_upload(&renderer, &job.cancel) {
                upload::Poll::Completed(_) => break,
                upload::Poll::Pending => pause()?,
                upload::Poll::Failed(e) => return Err(e),
                _ => return Err("missing bank upload".into()),
            }
        }
        while renderer.upload_retention().busy {
            pause()?;
        }
    }
    while renderer.upload_retention().busy {
        pause()?;
    }
    log::info!(
        "Helium native bank uploads ready count={}",
        descriptions.len()
    );
    let mut frames = renderer.create_frames()?;
    frames.begin(
        &renderer,
        &material,
        &job.cancel,
        &job.geometry.mesh(),
        &job.lightmap,
        job.view,
        Options {
            width: job.size[0] as usize,
            height: job.size[1] as usize,
            depth: helium_metal_prototype::terrain::Depth::Reversed,
            budget: Budget {
                payload_bytes: 192 * 1024 * 1024,
                target_pixels: 6 * 1024 * 1024,
            },
        },
    )?;
    log::info!(
        "Helium native frame submitted triangles={}",
        job.geometry.indices.len() / 3
    );
    let mut ticket = loop {
        match frames.poll(&renderer, &material, &job.cancel) {
            frame::Poll::Completed(t) => break t,
            frame::Poll::Pending => pause()?,
            frame::Poll::Failed(e) => return Err(e),
            _ => return Err("missing native frame".into()),
        }
    };
    while renderer.upload_retention().busy {
        pause()?;
    }
    log::info!("Helium native GPU frame completed");
    #[cfg(target_os = "ios")]
    let attachment = attached.as_ref().map(|l| l.generation());
    #[cfg(not(target_os = "ios"))]
    let attachment: Option<u64> = None;
    #[cfg(target_os = "ios")]
    let mut display = match attached {
        Some(layer) => layer.display(&renderer)?,
        None => renderer.detached_diagnostic_display(job.size[0] as usize, job.size[1] as usize)?,
    };
    #[cfg(not(target_os = "ios"))]
    let mut display =
        renderer.detached_diagnostic_display(job.size[0] as usize, job.size[1] as usize)?;
    let output = frames.take_completed(&renderer, &material, &job.cancel, &mut ticket)?;
    let Some(mut presentation) = display.present(output, 192 * 1024 * 1024, &job.cancel)? else {
        return Err("diagnostic CAMetalDrawable unavailable; no presentation success".into());
    };
    log::info!(
        "Helium native presentation submitted attached_generation={attachment:?}; scanout unverified"
    );
    while !presentation.poll()? {
        pause()?;
    }
    drop(presentation);
    Ok(format!(
        "backend=direct-Metal world_epoch={} key={:?} origin={:?} accepted_mesh_sequence={} banks={:?} revision={} vertices={} indices={} native_upload={:?} frame={:?} presentation={:?}; attached_generation={attachment:?}; GPU command completed, scanout unverified",
        job.world_epoch,
        job.geometry.key,
        job.geometry.origin,
        job.geometry.sequence,
        descriptions
            .iter()
            .map(|d| (d.bank, d.split, d.used, d.allocated))
            .collect::<Vec<_>>(),
        descriptions[0].identity.revision,
        job.geometry.vertices.len() / 24,
        job.geometry.indices.len(),
        material.counters(),
        frames.counters(),
        display.counters()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client_render::{
        gfx::material_snapshot::State,
        mesher::{OPAQUE_LAYER, TerrainVertex},
    };
    // Handcrafted actual SectionMesh type, NOT production mesher output/category D.
    fn mesh() -> SectionMesh {
        let mut m = SectionMesh::default();
        for p in [
            [0.125, -0.5, 0.],
            [1.25, -0.5, 0.],
            [1.25, 0.75, 0.],
            [0.125, 0.75, 0.],
        ] {
            m.solid.push(TerrainVertex::new(
                p,
                [1., 0.25],
                OPAQUE_LAYER | 257,
                0x9140ff80,
                0x1878,
            ));
        }
        m
    }
    fn geometry(key: Key, sequence: u64) -> Geometry {
        Geometry::capture(key, -64, &mesh(), State::new().generation(), sequence, 258)
            .unwrap()
            .unwrap()
    }
    #[test]
    fn explicit_selection_and_disabled_path() {
        assert_eq!(Mode::parse(None).unwrap(), Mode::Disabled);
        assert_eq!(Mode::parse(Some("off")).unwrap(), Mode::Disabled);
        assert_eq!(
            Mode::parse(Some("one-section")).unwrap(),
            Mode::OneSectionDiagnostic
        );
        assert!(Mode::parse(Some("metal")).is_err());
        let c = Controller::new(Mode::Disabled);
        assert!(c.candidate.is_none() && c.flight.is_none());
        assert_eq!(c.counters.accepted, 0);
        assert!(!Mode::Disabled.observes_surface(false));
        assert!(!Mode::AttachedOneSection.observes_surface(true));
        assert!(Mode::AttachedOneSection.observes_surface(false));
        assert!(Mode::OneSectionDiagnostic.observes_surface(true));
    }
    #[test]
    fn attached_selection_is_explicit_and_strict() {
        assert_eq!(
            Mode::parse(Some("attached-one-section")).unwrap(),
            Mode::AttachedOneSection
        );
        for bad in [
            "attached",
            "Attached-one-section",
            "attached-one-section\n",
            " one-section",
        ] {
            assert!(Mode::parse(Some(bad)).is_err());
        }
    }
    #[test]
    fn handcrafted_geometry_frustum_uses_original_positions_and_metal_depth() {
        let clip = [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ];
        let v = |origin| View {
            origin,
            clip_from_relative: clip,
        };
        assert!(points_visible(v([0.; 3]), [[0.; 3], [16.; 3]])); // crosses clip planes
        assert!(!points_visible(v([2., 0., 0.]), [[0.; 3], [16.; 3]]));
        assert!(!points_visible(v([0., 0., -17.]), [[0.; 3], [16.; 3]]));
        assert!(!points_visible(v([0., 0., 2.]), [[0.; 3], [16.; 3]]));
        assert!(!points_visible(v([f32::NAN, 0., 0.]), [[0.; 3], [16.; 3]]));
        assert!(points_visible(
            v([2., 0., 0.]),
            [[-2., 0., 0.], [-1.5, 0.5, 0.5]]
        )); // true model overhang
        assert!(!points_visible(v([0.; 3]), []));
        assert!(points_visible(
            v([0.; 3]),
            [[-2., -2., 0.5], [2., -2., 0.5], [0., 2., 0.5]]
        ));
        let perspective = View {
            origin: [0.; 3],
            clip_from_relative: [
                [1., 0., 0., 0.],
                [0., 1., 0., 0.],
                [0., 0., -1., -1.],
                [0., 0., -1., 0.],
            ],
        };
        assert!(points_visible(perspective, [[0., 0., -2.]]));
        assert!(!points_visible(perspective, [[0., 0., 2.]]));
        let mut translated = v([0.; 3]);
        translated.clip_from_relative[3][0] = -10.;
        assert!(points_visible(translated, [[10., 0., 0.5]]));
        assert!(!points_visible(translated, [[0., 0., 0.5]]));
    }
    #[test]
    fn handoff_preserves_all_bytes_and_large_global_ids() {
        let m = mesh();
        let g = Geometry::capture((2, 3, -4), -64, &m, State::new().generation(), 42, 258)
            .unwrap()
            .unwrap();
        assert_eq!(g.vertices, bytemuck::cast_slice::<_, u8>(&m.solid));
        assert_eq!(g.indices, [0, 1, 2, 0, 2, 3]);
        assert_eq!(g.origin, [32, -16, -64]);
        assert_eq!(g.sequence, 42);
        assert_eq!(g.mesh().vertex(0).unwrap().layer, OPAQUE_LAYER | 257);
        assert!(g.mesh().validate(256).is_err());
    }
    #[test]
    fn exact_world_origin_camera_subtraction_and_overflow() {
        assert_eq!(section_origin((-2, 3, 4), -64).unwrap(), [-32, -16, 64]);
        assert!(section_origin((i32::MAX, 0, 0), 0).is_err());
        assert!(section_origin((0, 1, 0), i32::MAX).is_err());
        let clip = [
            [1., 0., 0., 0.],
            [0., 1., 0., 0.],
            [0., 0., 1., 0.],
            [0., 0., 0., 1.],
        ];
        let v = camera_view(
            [30_000_000, -64, -30_000_000],
            [29_999_999.75, -64.125, -29_999_999.5],
            clip,
        )
        .unwrap();
        assert_eq!(v.origin, [0.25, 0.125, -0.5]);
        assert!(camera_view([0; 3], [f64::NAN, 0., 0.], clip).is_err());
    }
    #[test]
    fn unsupported_and_empty_sections_do_not_become_partial_scenes() {
        let state = State::new();
        assert!(
            Geometry::capture(
                (0, 0, 0),
                -64,
                &SectionMesh::default(),
                state.generation(),
                1,
                300
            )
            .unwrap()
            .is_none()
        );
        let mut m = mesh();
        m.translucent = m.solid.clone();
        assert!(Geometry::capture((0, 0, 0), 0, &m, state.generation(), 1, 300).is_err());
        let m = mesh();
        assert!(Geometry::capture((0, 0, 0), 0, &m, state.generation(), 1, 256).is_err());
    }
    #[test]
    fn unload_reload_replacement_and_shutdown_revoke_bounded_ownership() {
        let mut c = Controller::new(Mode::OneSectionDiagnostic);
        c.camera = Some(([0.; 3], [[0.; 4]; 4]));
        c.world_changed();
        assert!(c.camera.is_none());
        assert_eq!(c.world_epoch, 1);
        c.world_changed();
        assert_eq!(c.world_epoch, 2);
        c.candidate = Some(geometry((1, 2, 3), 1));
        c.remove_columns([(1, 3)].into_iter());
        assert!(c.candidate.is_none());
        c.candidate = Some(geometry((1, 2, 3), 2));
        c.invalidate("resource pack replaced");
        assert!(c.candidate.is_none());
        let mut c = Controller::new(Mode::OneSectionDiagnostic);
        c.candidate = Some(geometry((1, 2, 3), 3));
        c.invalidate("section superseded");
        assert_eq!(c.counters.invalidated, 1);
        let a = State::new();
        let b = State::new();
        assert_ne!(a.generation(), b.generation());
    }
    #[test]
    fn retained_size_failure_is_fail_closed() {
        let mut m = mesh();
        m.solid = vec![m.solid[0]; GEOMETRY_BUDGET / 24 + 4];
        assert!(Geometry::capture((0, 0, 0), 0, &m, State::new().generation(), 1, 300).is_err());
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    use crate::adapter::material_adapter::handcrafted_bridge_bank;
    fn geometry(source: &BankSource) -> Arc<Geometry> {
        let mut mesh = SectionMesh::default();
        for pos in [
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ] {
            mesh.solid
                .push(horizon_client_render::mesher::TerrainVertex::new(
                    pos,
                    [0., 0.],
                    0x8000,
                    0x00ffffff,
                    0,
                ));
        }
        Arc::new(
            Geometry::capture(
                (0, 0, 0),
                0,
                &mesh,
                source.description().identity.generation,
                1,
                3,
            )
            .unwrap()
            .unwrap(),
        )
    }
    #[test]
    fn game_shutdown_unload_and_replacement_revoke_worker_lease() {
        for reason in [
            "shutdown",
            "unload",
            "replacement",
            "suspend",
            "pack reload",
        ] {
            let source = handcrafted_bridge_bank();
            let geometry = geometry(&source);
            let cancel = Arc::new(AtomicBool::new(false));
            let (_send, receiver) = mpsc::sync_channel(1);
            let mut owner = Controller::new(Mode::OneSectionDiagnostic);
            owner.flight = Some(Flight {
                geometry: geometry.clone(),
                source: vec![source],
                cancel: cancel.clone(),
                light_revision: 1,
                size: [256, 256],
                receiver,
                #[cfg(target_os = "ios")]
                attached: None,
            });
            if reason == "shutdown" {
                drop(owner)
            } else if reason == "unload" {
                owner.remove_columns([(0, 0)].into_iter());
            } else {
                owner.invalidate(reason);
            }
            assert!(cancel.load(Ordering::Acquire));
            assert!(Arc::strong_count(&geometry) >= 1); // CPU lease survives producer release
        }
    }
    #[test]
    fn cpu_lease_accounts_shared_pixels_and_rejects_revocation() {
        let bank = handcrafted_bridge_bank();
        let geometry = geometry(&bank);
        let pixels = bank.pixels().as_ptr();
        let lease = LeasedSource { bank, geometry };
        let authority = AtomicBool::new(false);
        assert_eq!(pixels, lease.pixels().as_ptr());
        assert!(lease.retained_bytes() >= lease.pixels().len() + 120);
        lease.check_current(&authority).unwrap();
        authority.store(true, Ordering::Release);
        assert!(lease.check_current(&authority).is_err());
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    #[test]
    #[ignore = "actual Metal worker; handcrafted SectionMesh/lighting/camera, production mip generator; NOT category D"]
    fn native_worker_from_production_owned_bank_and_typed_section() {
        let banks = crate::adapter::material_adapter::handcrafted_banked_sources();
        assert_eq!(
            banks[0].description().split,
            512,
            "native test must set HORIZON_SPRITE_SPLIT=512"
        );
        assert_eq!(banks.len(), 3);
        let original = geometry(&banks[0]);
        let mut vertices = original.vertices.clone();
        let mut indices = original.indices.clone();
        for id in [512u16, 1026] {
            let start = (vertices.len() / 24) as u32;
            let mut quad = original.vertices.clone();
            for v in quad.chunks_exact_mut(24) {
                v[16..18].copy_from_slice(&(0x8000 | id).to_le_bytes());
            }
            vertices.extend_from_slice(&quad);
            indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        }
        let geometry = Arc::new(Geometry {
            vertices,
            indices,
            ..Arc::try_unwrap(original).ok().unwrap()
        });
        let expected = banks[0].retained_bytes()
            + geometry.vertices.capacity()
            + geometry.indices.capacity() * 4;
        let job = Job {
            world_epoch: 0, // handcrafted test, NOT an actual game session
            source: banks,
            geometry,
            cancel: Arc::new(AtomicBool::new(false)),
            lightmap: [255; 1024],
            view: View {
                origin: [0.; 3],
                clip_from_relative: [
                    [1., 0., 0., 0.],
                    [0., 1., 0., 0.],
                    [0., 0., 1., 0.],
                    [0., 0., 0., 1.],
                ],
            },
            size: [256, 256],
        };
        let report = run(job).expect("real Metal bridge worker");
        assert!(report.contains("backend=direct-Metal"));
        assert!(
            report.contains(&format!("source_retained: {expected}")),
            "shared snapshot must count once: {report}"
        );
        eprintln!(
            "G06 native worker with HANDCRAFTED production structs and production mip preparation, synthetic camera/lightmap; NOT game-runtime evidence: {report}"
        );
    }
}

#[cfg(test)]
mod bank_tests {
    use super::*;
    #[test]
    fn real_prepared_metadata_retains_namespace_without_bank_zero_assumption() {
        use horizon_client_render::gfx::PreparedTextures;
        let pixels = vec![255; 512 * 4];
        let p = PreparedTextures::new(&pixels, 1, 512, &[], (&[255; 4], 1))
            .retain_material_snapshot(1 << 20);
        let s = p.capture_materials(1, 512, false);
        assert_eq!(production_layers(s.snapshot().unwrap()).unwrap(), 512);
        let p = PreparedTextures::new(&pixels[..256 * 4], 1, 256, &[], (&[255; 4], 1))
            .retain_material_snapshot(1 << 20);
        let s = p.capture_materials(1, 256, false);
        assert_eq!(production_layers(s.snapshot().unwrap()).unwrap(), 256);
    }
}
