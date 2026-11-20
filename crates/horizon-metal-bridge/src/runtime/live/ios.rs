use super::*;
use helium_metal_prototype::{
    attached::{self, Layer, Ticket},
    terrain::{LiveScene, LiveSection, LiveSubmission, TerrainRenderer},
};

const INPUT_INTERVAL: Duration = Duration::from_millis(16);
const GPU_DEADLINE: Duration = Duration::from_secs(2);
const MATERIAL_BUDGET: upload::Budget = upload::Budget {
    payload_bytes: 112 * MIB,
    staging_bytes: 16 * MIB,
};
const GPU_GEOMETRY: usize = 16 * MIB;
const MAX_DRAWS: usize = 4096;

use helium_render_scene::{Camera, Frame as CpuFrame, Lightmap, MaterialSet, MaterialVersion};
use horizon_client_render::gfx::material_snapshot::State;
use std::num::NonZeroU64;

struct Request {
    scene: CpuFrame<Arc<PinnedGeometry>>,
    prepared_us: u64,
    admitted_us: u64,
    omitted_vertices: usize,
    cpu_evictions: u64,
}
struct Session {
    ticket: Ticket,
    done: Receiver<Result<(), String>>,
    input: Arc<Latest<Request>>,
    generation: Generation,
    size: [u32; 2],
}
pub(in crate::runtime) struct Live {
    meshes: Meshes,
    camera: [f64; 3],
    generation: Option<Generation>,
    banks: Option<Arc<MaterialSet>>,
    session: Option<Session>,
    next_start: Instant,
    pressure_until: Instant,
    next_input: Instant,
    dropped: Arc<AtomicU64>,
    admitted_us: u64,
}

impl Live {
    pub fn new() -> Self {
        Self {
            meshes: Meshes::default(),
            camera: [0.; 3],
            generation: None,
            banks: None,
            session: None,
            next_start: Instant::now(),
            pressure_until: Instant::now(),
            next_input: Instant::now(),
            dropped: Arc::new(AtomicU64::new(0)),
            admitted_us: 0,
        }
    }
    pub fn cancel(&mut self, clear: bool) {
        if self.session.take().is_some() {
            self.next_start = Instant::now() + Duration::from_millis(250);
        }
        self.banks = None;
        if clear {
            self.meshes.clear();
            self.camera = [0.; 3];
            self.generation = None;
        }
    }
    pub fn memory_pressure(&mut self) {
        self.cancel(true);
        self.pressure_until = Instant::now() + Duration::from_secs(5);
        self.next_start = self.pressure_until;
    }
    pub fn remove_columns(&mut self, columns: impl Iterator<Item = (i32, i32)>) {
        for column in columns {
            self.meshes.entries.retain(|key, mesh| {
                let removed = (key.0, key.2) == column;
                if removed {
                    self.meshes.bytes -= mesh.bytes;
                }
                !removed
            });
        }
    }
    fn generation(&mut self, generation: Generation) {
        if self.generation != Some(generation) {
            self.cancel(true);
            self.generation = Some(generation);
        }
    }
    pub fn observe(&mut self, state: &State, sections: AcceptedSections<'_>, epoch: u64) {
        let Some(epoch) = NonZeroU64::new(epoch) else {
            return;
        };
        let start = Instant::now();
        self.generation(state.generation());
        if start < self.pressure_until {
            return;
        }
        let Ok(snapshot) = state.snapshot() else {
            return;
        };
        let Ok(layers) = production_layers(snapshot) else {
            return;
        };
        let mut near = Vec::with_capacity(MAX_SECTIONS);
        for section in sections {
            let (key, min_y, version, mesh) = section.parts();
            self.meshes.remove(key);
            let quads = mesh.solid.len() / 4;
            // Include the worst case resident draw metadata in admission.
            let bytes = quads.saturating_mul(120 + 2 * size_of::<(usize, usize, u32)>());
            if quads == 0 || bytes > GEOMETRY_BUDGET {
                continue;
            }
            let Ok(origin) = section_origin(key, min_y) else {
                continue;
            };
            let priority = distance(origin, self.camera);
            let i = near.partition_point(|(d, _)| *d <= priority);
            if i < MAX_SECTIONS {
                if near.len() == MAX_SECTIONS {
                    near.pop();
                }
                near.insert(i, (priority, (key, min_y, version, mesh, bytes)));
            }
        }
        let mut budget = ADMIT_BYTES;
        for (_, (key, min_y, version, mesh, bytes)) in near {
            if bytes > budget || self.meshes.retained.load(Ordering::Acquire) + bytes > CPU_LIMIT {
                continue;
            }
            budget -= bytes;
            if let Ok(Some(geometry)) =
                Geometry::capture(key, min_y, mesh, state.generation(), version, layers)
                    .and_then(|captured| captured.map(|g| g.into_scene(epoch)).transpose())
            {
                self.meshes.admit(geometry, self.camera);
            }
        }
        self.admitted_us = start.elapsed().as_micros() as u64;
    }
    pub fn reference_frame(
        &mut self,
        renderer: &Renderer,
        frame: &Frame<'_>,
        window: Option<&winit::window::Window>,
        epoch: u64,
    ) {
        if frame.title || renderer.validate_material_snapshot_mode().is_err() {
            self.cancel(frame.title || epoch == 0);
            return;
        }
        let camera = Camera {
            position: frame.camera.to_array(),
            clip_from_relative: renderer
                .views(frame)
                .primary()
                .view_proj(frame.bob * frame.spin)
                .to_cols_array_2d(),
        };
        if let Err(err) = self.frame(
            renderer.material_state(),
            camera,
            renderer.lightmap_pixels,
            [renderer.config.width, renderer.config.height],
            window,
            epoch,
            false,
        ) {
            log::debug!("Helium live scene unavailable: {err}");
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        state: &State,
        camera: Camera,
        rgba: [u8; 1024],
        size: [u32; 2],
        window: Option<&winit::window::Window>,
        epoch: u64,
        fullscreen: bool,
    ) -> Result<(), String> {
        let start = Instant::now();
        self.camera = camera.position;
        self.generation(state.generation());
        if let Some(session) = &self.session {
            if let Ok(result) = session.done.try_recv() {
                self.cancel(false);
                self.next_start = start + Duration::from_secs(2);
                if let Err(err) = result {
                    if fullscreen {
                        return Err(err);
                    }
                    log::warn!("Helium S2B worker retired: {err}");
                }
            }
        }
        if self.session.as_ref().is_some_and(|s| {
            s.ticket.is_cancelled() || s.size != size || s.generation != state.generation()
        }) {
            self.cancel(false);
        }
        let (Some(epoch), Some(window)) = (NonZeroU64::new(epoch), window) else {
            self.cancel(epoch == 0);
            return Ok(());
        };
        if size.contains(&0) || start < self.next_input {
            return Ok(());
        }
        if self.meshes.entries.is_empty() {
            self.cancel(false);
            return Ok(());
        }
        let Ok(light_revision) = state.light_revision() else {
            return Ok(());
        };
        let version = MaterialVersion {
            generation: state.generation().diagnostic_id(),
            revision: state.animation_revision(),
        };
        if self.banks.as_ref().is_none_or(|b| b.version() != version) {
            match state.scene_materials(SNAPSHOT_BUDGET) {
                Ok(banks) => self.banks = Some(Arc::new(banks)),
                Err(err) => {
                    self.cancel(false);
                    return Err(err.into());
                }
            }
        }
        if self.session.is_none() {
            if start < self.next_start || !attached::can_start() {
                return Ok(());
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let input = Arc::new(Latest::default());
            let worker_input = input.clone();
            let worker_cancel = cancel.clone();
            let dropped = self.dropped.clone();
            let retained = self.meshes.retained.clone();
            match attached::submit_live(window, cancel, fullscreen, move |layer| {
                let result = run(
                    layer,
                    &worker_input,
                    &worker_cancel,
                    epoch.get(),
                    &dropped,
                    &retained,
                );
                if worker_cancel.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    result
                }
            }) {
                Ok((ticket, done)) => {
                    self.session = Some(Session {
                        ticket,
                        done,
                        input,
                        generation: state.generation(),
                        size,
                    })
                }
                Err(err) => {
                    log::debug!("Helium S2B host not ready: {err}");
                    self.next_start = start + Duration::from_secs(2);
                    return Ok(());
                }
            }
        }
        self.next_input = start + INPUT_INTERVAL;
        let request = Request {
            scene: CpuFrame {
                epoch,
                camera,
                sections: self.meshes.entries.values().cloned().collect(),
                materials: self.banks.as_ref().unwrap().clone(),
                lightmap: Lightmap {
                    revision: light_revision,
                    rgba,
                },
            },
            prepared_us: start.elapsed().as_micros() as u64,
            admitted_us: self.admitted_us,
            omitted_vertices: self
                .meshes
                .entries
                .values()
                .map(|m| m.mesh.omitted_translucent_vertices)
                .sum(),
            cpu_evictions: self.meshes.evictions,
        };
        if !self.session.as_ref().unwrap().input.offer(request) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
}

struct Resident {
    revision: u64,
    section: LiveSection<u64>,
}

fn run(
    layer: Layer,
    input: &Latest<Request>,
    cancel: &AtomicBool,
    epoch: u64,
    dropped: &AtomicU64,
    retained: &AtomicUsize,
) -> Result<(), String> {
    let renderer = TerrainRenderer::new()?;
    let display = layer.display(&renderer)?;
    let mut scene = LiveScene::new(&renderer, display.dimensions())?;
    let mut material = None;
    let mut resident = BTreeMap::<Key, Resident>::new();
    let mut metrics = Metrics::default();
    let mut report = Instant::now();
    let mut report_count = 0;
    let mut available = Instant::now();
    while !cancel.load(Ordering::Acquire) {
        let Some(request) = input.take(Duration::from_millis(25)) else {
            if available.elapsed() >= Duration::from_millis(500) {
                break;
            }
            continue;
        };
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let frame = &request.scene;
        if frame.epoch.get() != epoch {
            return Err("live scene belongs to another world epoch".into());
        }
        let started = Instant::now();
        metrics.admitted_us = request.admitted_us;
        metrics.omitted_vertices = request.omitted_vertices;
        metrics.cpu_evictions = request.cpu_evictions;
        let previous = resident.len();
        resident.retain(|key, value| {
            frame.sections.iter().any(|entry| {
                frame.accepts(&entry.mesh)
                    && entry.mesh.version.key == *key
                    && entry.mesh.version.revision == value.revision
            })
        });
        metrics.evicted += (previous - resident.len()) as u64;
        let mut visible: Vec<_> = frame
            .sections
            .iter()
            .filter_map(|entry| {
                if !frame.accepts(&entry.mesh) {
                    return None;
                }
                let view = frame.camera.section_view(entry.mesh.origin).ok()?;
                bounds_visible(view, entry.mesh.bounds).then(|| (entry, view))
            })
            .collect();
        visible.sort_unstable_by(|a, b| {
            distance(a.0.mesh.origin, frame.camera.position)
                .total_cmp(&distance(b.0.mesh.origin, frame.camera.position))
        });
        metrics.cull_us = started.elapsed().as_micros() as u64;
        if visible.is_empty() {
            metrics.skipped += 1;
            if available.elapsed() >= Duration::from_millis(500) {
                break;
            }
            continue;
        }

        let banks = frame.materials.banks();
        if material.is_none() {
            material = Some(renderer.create_material_set(banks, cancel, MATERIAL_BUDGET)?);
        }
        let material = material.as_mut().unwrap();
        for bank in banks {
            let d = bank.description();
            let target = material.bank_mut(d.bank)?;
            if target.phase() == upload::Phase::Completed
                && target.description().identity == d.identity
            {
                continue;
            }
            let start = Instant::now();
            if target
                .adopt_unchanged(&renderer, bank.clone(), cancel, MATERIAL_BUDGET)
                .is_ok()
            {
                continue;
            }
            let update = target.changed_layers(bank);
            target.begin_upload(&renderer, bank.clone(), cancel, update, MATERIAL_BUDGET)?;
            let deadline = Instant::now() + GPU_DEADLINE;
            loop {
                match target.poll_upload(&renderer, cancel) {
                    upload::Poll::Completed(_) => break,
                    upload::Poll::Pending => wait(cancel, deadline)?,
                    upload::Poll::Failed(err) => return Err(err),
                    _ => return Err("missing live material receipt".into()),
                }
            }
            finish_retirement(&renderer, cancel, deadline)?;
            metrics.upload_us = start.elapsed().as_micros() as u64;
        }
        metrics.material = material.counters().texture_payload;
        metrics.staging = material.counters().staging_peak;
        let mut budget = ADMIT_BYTES;
        let mut live_bytes: usize = resident.values().map(|r| r.section.bytes()).sum();
        for (entry, _) in &visible {
            let g = &entry.mesh;
            if resident.contains_key(&g.version.key) {
                continue;
            }
            let estimated =
                g.retained_bytes() + g.mesh().indices.len() / 3 * size_of::<(usize, usize, u32)>();
            if estimated > budget {
                continue;
            }
            if live_bytes + estimated > GPU_GEOMETRY {
                // Evict out-of-view or strictly farther buffers, never the valid
                // near sections that can render while this request catches up.
                let far = frame
                    .sections
                    .iter()
                    .filter(|mesh| resident.contains_key(&mesh.mesh.version.key))
                    .filter(|mesh| {
                        distance(mesh.mesh.origin, frame.camera.position)
                            > distance(g.origin, frame.camera.position)
                    })
                    .max_by(|a, b| {
                        distance(a.mesh.origin, frame.camera.position)
                            .total_cmp(&distance(b.mesh.origin, frame.camera.position))
                    });
                if let Some(far) = far {
                    live_bytes -= resident
                        .remove(&far.mesh.version.key)
                        .unwrap()
                        .section
                        .bytes();
                    metrics.evicted += 1;
                }
            }
            if live_bytes + estimated > GPU_GEOMETRY {
                continue;
            }
            budget -= estimated;
            let section = LiveSection::from_scene(&renderer, g, frame, estimated)?;
            live_bytes += section.bytes();
            resident.insert(
                g.version.key,
                Resident {
                    revision: g.version.revision,
                    section,
                },
            );
            metrics.uploads += 1;
        }
        metrics.peak_geometry = metrics.peak_geometry.max(live_bytes);
        let mut remaining_draws = MAX_DRAWS;
        let draw: Vec<_> = visible
            .iter()
            .filter_map(|(g, view)| {
                let section = &resident.get(&g.mesh.version.key)?.section;
                remaining_draws = remaining_draws.checked_sub(section.draw_count())?;
                Some((section, *view))
            })
            .collect();
        if draw.is_empty() {
            metrics.skipped += 1;
            if available.elapsed() >= Duration::from_millis(500) {
                break;
            }
            continue;
        }
        match scene.submit_scene(&renderer, material, Some(&display), frame, &draw, cancel)? {
            LiveSubmission::Submitted => {
                let deadline = Instant::now() + GPU_DEADLINE;
                loop {
                    if let Some(result) = scene.poll(Some(&display), cancel)? {
                        metrics.record(result, request.prepared_us);
                        available = Instant::now();
                        layer.reveal_live()?;
                        break;
                    }
                    wait(cancel, deadline)?;
                }
                finish_retirement(&renderer, cancel, deadline)?;
            }
            LiveSubmission::Pending | LiveSubmission::DrawableUnavailable => {
                metrics.skipped += 1;
                if available.elapsed() >= Duration::from_millis(500) {
                    break;
                }
            }
        }
        if report.elapsed() >= Duration::from_secs(5) {
            metrics.rate_hz =
                (metrics.completed - report_count) as f64 / report.elapsed().as_secs_f64();
            report_count = metrics.completed;
            metrics.report(
                epoch,
                dropped.load(Ordering::Relaxed),
                retained.load(Ordering::Relaxed),
                live_bytes,
                scene.bytes(),
            );
            report = Instant::now();
        }
    }
    metrics.report(
        epoch,
        dropped.load(Ordering::Relaxed),
        retained.load(Ordering::Relaxed),
        resident.values().map(|r| r.section.bytes()).sum(),
        scene.bytes(),
    );
    Ok(())
}
fn wait(cancel: &AtomicBool, deadline: Instant) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
        return Err("live GPU command cancelled or timed out; terminal keepalive retained".into());
    }
    std::thread::sleep(Duration::from_micros(500));
    Ok(())
}
fn finish_retirement(
    renderer: &TerrainRenderer,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<(), String> {
    while renderer.upload_retention().busy {
        wait(cancel, deadline)?;
    }
    Ok(())
}
