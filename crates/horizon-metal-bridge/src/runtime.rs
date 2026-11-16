use crate::adapter::{
    self,
    material_adapter::{BankSource, Binding},
};
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
}
impl Mode {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("") | Some("off") => Ok(Self::Disabled),
            Some("one-section") => Ok(Self::OneSectionDiagnostic),
            Some(v) => Err(format!(
                "unsupported HELIUM_DIRECT_METAL={v}; use one-section or off"
            )),
        }
    }
}
pub fn enabled() -> bool {
    cfg!(any(target_os = "macos", target_os = "ios"))
        && Mode::parse(std::env::var("HELIUM_DIRECT_METAL").ok().as_deref())
            == Ok(Mode::OneSectionDiagnostic)
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
struct Flight {
    geometry: Arc<Geometry>,
    source: BankSource,
    cancel: Arc<AtomicBool>,
    light_revision: u64,
    size: [u32; 2],
    receiver: Receiver<Result<String, String>>,
}
/// One candidate and one worker, never a world mirror or mesh queue. This mode
/// produces an UNATTACHED diagnostic drawable; wgpu remains the visible backend.
pub struct Controller {
    mode: Mode,
    candidate: Option<Geometry>,
    flight: Option<Flight>,
    sequence: u64,
    finished: bool,
    counters: Counters,
    last_error: Option<String>,
}
impl Controller {
    pub fn from_environment() -> Option<Self> {
        let mode = match Mode::parse(std::env::var("HELIUM_DIRECT_METAL").ok().as_deref()) {
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
        log::info!(
            "Helium Direct Metal one-section diagnostic armed; visible backend remains wgpu"
        );
        Some(Self::new(mode))
    }
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            candidate: None,
            flight: None,
            sequence: 0,
            finished: false,
            counters: Default::default(),
            last_error: None,
        }
    }
    fn error(&mut self, error: String) {
        self.counters.rejected += 1;
        if self.last_error.as_ref() != Some(&error) {
            log::warn!("Helium Direct Metal rejected: {error}; wgpu retained");
            self.last_error = Some(error);
        }
    }
    pub fn invalidate(&mut self, reason: &str) {
        self.candidate = None;
        if let Some(f) = &self.flight {
            f.cancel.store(true, Ordering::Release);
        }
        self.counters.invalidated += 1;
        log::debug!("Helium Direct Metal revoked: {reason}");
    }
    /// Called ONLY for accepted Streaming::update results before their storage is
    /// returned to Streaming::free. Streamer has already rejected stale job seqs.
    pub fn section(&mut self, renderer: &Renderer, key: Key, min_y: i32, mesh: &SectionMesh) {
        if self.mode == Mode::Disabled || self.finished {
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
                    snapshot.sprites() as usize,
                )
            });
        self.counters.preparation_us += start.elapsed().as_micros();
        match result {
            Ok(Some(g)) => {
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
        if let Some(f) = &self.flight {
            if f.source.check_current(renderer).is_err()
                || renderer.material_state().light_revision() != Ok(f.light_revision)
                || f.size != [renderer.config.width, renderer.config.height]
            {
                f.cancel.store(true, Ordering::Release);
            }
            match f.receiver.try_recv() {
                Ok(result) => {
                    let f = self.flight.take().unwrap();
                    let valid = !f.cancel.load(Ordering::Acquire)
                        && f.source.check_current(renderer).is_ok()
                        && renderer.material_state().light_revision() == Ok(f.light_revision);
                    self.finished = true;
                    match result {
                        Ok(evidence) if valid => log::info!(
                            "Helium Direct Metal CATEGORY D handoff completed: {evidence}; {:?}; visible backend=wgpu, scanout NOT established",
                            self.counters
                        ),
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
            let source = binding.upload_source(renderer, 0)?;
            let validation = binding.upload_source(renderer, 0)?;
            let d = source.description();
            if d.allocated > 256 {
                return Err(format!(
                    "actual bank 0 allocated={} used={} split={} sprites={} exceeds unchanged G05 256-slice policy",
                    d.allocated, d.used, d.split, d.sprites
                ));
            }
            g.mesh().validate(d.used as usize)?;
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
        let (sender, receiver) = mpsc::sync_channel(1);
        let job = Job {
            source,
            geometry: geometry.clone(),
            cancel: cancel.clone(),
            lightmap,
            view,
            size: [renderer.config.width, renderer.config.height],
        };
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
                })
            }
            Err(e) => self.error(format!("native worker launch failed: {e}")),
        }
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        if let Some(f) = &self.flight {
            f.cancel.store(true, Ordering::Release);
        }
    }
}
struct Job {
    source: BankSource,
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
        self.bank.description()
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
    let d = job.source.description();
    let source = LeasedSource {
        bank: job.source,
        geometry: job.geometry.clone(),
    };
    let budget = upload::Budget {
        payload_bytes: 192 * 1024 * 1024,
        staging_bytes: 16 * 1024 * 1024,
    };
    let mut material = renderer.create_material(&source, &job.cancel, budget)?;
    material.begin_upload(&renderer, source, &job.cancel, upload::Update::Full, budget)?;
    loop {
        match material.poll_upload(&renderer, &job.cancel) {
            upload::Poll::Completed(_) => break,
            upload::Poll::Pending => pause()?,
            upload::Poll::Failed(e) => return Err(e),
            _ => return Err("missing native upload".into()),
        }
    }
    while renderer.upload_retention().busy {
        pause()?;
    }
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
    let mut display =
        renderer.detached_diagnostic_display(job.size[0] as usize, job.size[1] as usize)?;
    let output = frames.take_completed(&renderer, &material, &job.cancel, &mut ticket)?;
    let Some(mut presentation) = display.present(output, 192 * 1024 * 1024, &job.cancel)? else {
        return Err("diagnostic CAMetalDrawable unavailable; no presentation success".into());
    };
    while !presentation.poll()? {
        pause()?;
    }
    drop(presentation);
    Ok(format!(
        "backend=direct-Metal key={:?} origin={:?} accepted_mesh_sequence={} bank={} revision={} vertices={} indices={} native_upload={:?} frame={:?} presentation={:?}; UNATTACHED drawable",
        job.geometry.key,
        job.geometry.origin,
        job.geometry.sequence,
        d.bank,
        d.identity.revision,
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
                source,
                cancel: cancel.clone(),
                light_revision: 1,
                size: [256, 256],
                receiver,
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
        let bank = handcrafted_bridge_bank();
        let geometry = geometry(&bank);
        let job = Job {
            source: bank,
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
        eprintln!(
            "G06 native worker with HANDCRAFTED production structs and production mip preparation, synthetic camera/lightmap; NOT game-runtime evidence: {report}"
        );
    }
}
