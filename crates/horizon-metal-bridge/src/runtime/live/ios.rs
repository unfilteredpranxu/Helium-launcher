use super::*;
use helium_metal_prototype::{
    attached::{self, Layer, Ticket},
    terrain::{LiveFrame, LiveScene, LiveSection, LiveSubmission, TerrainRenderer},
};
use std::collections::VecDeque;

const INPUT_INTERVAL: Duration = Duration::from_millis(16);
const GPU_DEADLINE: Duration = Duration::from_secs(2);
const MATERIAL_BUDGET: upload::Budget = upload::Budget {
    payload_bytes: 112 * MIB,
    staging_bytes: 16 * MIB,
};
const GPU_GEOMETRY: usize = 16 * MIB;
const MAX_DRAWS: usize = 4096;

#[derive(Clone)]
struct Bank(BankSource);
impl Source for Bank {
    type Generation = Generation;
    type Authority = AtomicBool;
    fn description(&self) -> upload::Description<Generation> {
        let mut description = self.0.description();
        description.version = upload::BANKED_VERSION;
        description
    }
    fn pixels(&self) -> &[u8] {
        self.0.pixels()
    }
    fn retained_bytes(&self) -> usize {
        self.0.retained_bytes()
    }
    fn retention_key(&self) -> Option<(usize, usize)> {
        self.0.retention_key()
    }
    fn check_current(&self, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::Acquire) {
            Err("live session revoked".into())
        } else {
            Ok(())
        }
    }
}

struct Request {
    camera: [f64; 3],
    clip: [[f32; 4]; 4],
    meshes: Vec<Arc<PinnedGeometry>>,
    banks: Arc<Vec<Bank>>,
    light: [u8; 1024],
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
    banks: Option<Arc<Vec<Bank>>>,
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
    pub fn observe(&mut self, renderer: &Renderer, sections: AcceptedSections<'_>) {
        let start = Instant::now();
        let state = renderer.material_state();
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
            {
                self.meshes.admit(geometry, self.camera);
            }
        }
        self.admitted_us = start.elapsed().as_micros() as u64;
    }
    pub fn frame(
        &mut self,
        renderer: &Renderer,
        frame: &Frame<'_>,
        window: Option<&winit::window::Window>,
        epoch: u64,
    ) {
        let start = Instant::now();
        self.camera = frame.camera.to_array();
        let state = renderer.material_state();
        self.generation(state.generation());
        if let Some(session) = &self.session {
            if let Ok(result) = session.done.try_recv() {
                if let Err(err) = result {
                    log::warn!("Helium S2B worker retired: {err}");
                }
                self.cancel(false);
                self.next_start = start + Duration::from_secs(2);
            }
        }
        let size = [renderer.config.width, renderer.config.height];
        if self.session.as_ref().is_some_and(|s| {
            s.ticket.is_cancelled() || s.size != size || s.generation != state.generation()
        }) {
            self.cancel(false);
        }
        if epoch == 0
            || window.is_none()
            || frame.title
            || start < self.next_input
            || renderer.validate_material_snapshot_mode().is_err()
            || state.light_revision().is_err()
        {
            if epoch == 0
                || window.is_none()
                || frame.title
                || renderer.validate_material_snapshot_mode().is_err()
            {
                self.cancel(frame.title || epoch == 0);
            }
            return;
        }
        if self.meshes.entries.is_empty() {
            self.cancel(false);
            return;
        }
        let Ok(snapshot) = state.snapshot() else {
            self.cancel(false);
            return;
        };
        let id = upload::Identity {
            generation: state.generation(),
            revision: state.animation_revision(),
        };
        if self
            .banks
            .as_ref()
            .is_none_or(|b| b[0].description().identity != id)
        {
            let sources = Binding::new(renderer).and_then(|binding| {
                (0..snapshot.banks().len())
                    .map(|bank| binding.upload_source(renderer, bank as u32).map(Bank))
                    .collect::<Result<Vec<_>, _>>()
            });
            match sources {
                Ok(banks) if !banks.is_empty() && banks.len() <= 8 => {
                    self.banks = Some(Arc::new(banks))
                }
                _ => {
                    self.cancel(false);
                    return;
                }
            }
        }
        if self.session.is_none() {
            if start < self.next_start || !attached::can_start() {
                return;
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let input = Arc::new(Latest::default());
            let worker_input = input.clone();
            let worker_cancel = cancel.clone();
            let dropped = self.dropped.clone();
            let retained = self.meshes.retained.clone();
            match attached::submit_live(window.unwrap(), cancel, move |layer| {
                run(
                    layer,
                    &worker_input,
                    &worker_cancel,
                    epoch,
                    &dropped,
                    &retained,
                )
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
                    return;
                }
            }
        }
        self.next_input = start + INPUT_INTERVAL;
        let clip = renderer
            .views(frame)
            .primary()
            .view_proj(frame.bob * frame.spin)
            .to_cols_array_2d();
        let request = Request {
            camera: self.camera,
            clip,
            meshes: self.meshes.entries.values().cloned().collect(),
            banks: self.banks.as_ref().unwrap().clone(),
            light: renderer.lightmap_pixels,
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
    }
}

struct Resident {
    revision: u64,
    section: LiveSection<Generation>,
}
#[derive(Default)]
struct Metrics {
    completed: u64,
    skipped: u64,
    draws: u64,
    uploads: u64,
    evicted: u64,
    peak_geometry: usize,
    material: usize,
    staging: usize,
    upload_us: u64,
    admitted_us: u64,
    omitted_vertices: usize,
    cpu_evictions: u64,
    last_present_us: u64,
    rate_hz: f64,
    cull_us: u64,
    encode_us: u64,
    gpu_us: Option<u64>,
    stalls_50: u64,
    stalls_100: u64,
    last: Option<Instant>,
    cpu: VecDeque<u64>,
}
impl Metrics {
    fn record(&mut self, result: helium_metal_prototype::terrain::LiveCompletion, prepared: u64) {
        self.last_present_us = result.completion_latency.as_micros() as u64;
        self.completed += 1;
        self.draws += result.draws;
        self.encode_us = result.encode_time.as_micros() as u64;
        self.gpu_us = result.gpu_time.map(|t| t.as_micros() as u64);
        if self.cpu.len() == 120 {
            self.cpu.pop_front();
        }
        self.cpu.push_back(prepared + self.cull_us + self.encode_us);
        let now = Instant::now();
        if let Some(last) = self.last.replace(now) {
            let gap = now.duration_since(last);
            self.stalls_50 += u64::from(gap > Duration::from_millis(50));
            self.stalls_100 += u64::from(gap > Duration::from_millis(100));
        }
    }
    fn report(
        &self,
        epoch: u64,
        dropped: u64,
        cpu_geometry: usize,
        gpu_geometry: usize,
        targets: usize,
    ) {
        let mut samples: Vec<_> = self.cpu.iter().copied().collect();
        samples.sort_unstable();
        let quantile = |p| {
            samples
                .get(samples.len().saturating_sub(1) * p / 100)
                .copied()
        };
        log::info!(
            "Helium S2B source=game epoch={epoch} complete={} skipped={} replaced_requests={dropped} draws={} geometry_uploads={} evicted={} cpu_geometry={cpu_geometry} gpu_geometry={gpu_geometry} peak_gpu_geometry={} material={} staging_peak={} targets={targets} cpu_us_p50={:?} p95={:?} p99={:?} admit_us={} cull_us={} encode_us={} upload_us={} completion_us={} gpu_us={:?} completed_hz={:.1} cpu_evictions={} omitted_translucent_vertices={} stalls_gt_50_ms={} stalls_gt_100_ms={}; scanout unverified",
            self.completed,
            self.skipped,
            self.draws,
            self.uploads,
            self.evicted,
            self.peak_geometry,
            self.material,
            self.staging,
            quantile(50),
            quantile(95),
            quantile(99),
            self.upload_us,
            self.gpu_us,
            self.stalls_50,
            self.stalls_100
        );
    }
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
        let started = Instant::now();
        metrics.admitted_us = request.admitted_us;
        metrics.omitted_vertices = request.omitted_vertices;
        metrics.cpu_evictions = request.cpu_evictions;
        let previous = resident.len();
        resident.retain(|key, value| {
            request
                .meshes
                .iter()
                .any(|entry| entry.mesh.key == *key && entry.mesh.sequence == value.revision)
        });
        metrics.evicted += (previous - resident.len()) as u64;
        let mut visible: Vec<_> = request
            .meshes
            .iter()
            .filter_map(|entry| {
                let view = camera_view(entry.mesh.origin, request.camera, request.clip).ok()?;
                bounds_visible(view, entry.mesh.bounds).then(|| (entry, view))
            })
            .collect();
        visible.sort_unstable_by(|a, b| {
            distance(a.0.mesh.origin, request.camera)
                .total_cmp(&distance(b.0.mesh.origin, request.camera))
        });
        metrics.cull_us = started.elapsed().as_micros() as u64;
        if visible.is_empty() {
            metrics.skipped += 1;
            if available.elapsed() >= Duration::from_millis(500) {
                break;
            }
            continue;
        }

        let namespace = request.banks[0].description();
        if material.is_none() {
            material =
                Some(renderer.create_material_set(&request.banks, cancel, MATERIAL_BUDGET)?);
        }
        let material = material.as_mut().unwrap();
        for bank in request.banks.iter() {
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
            if resident.contains_key(&g.key) {
                continue;
            }
            let estimated =
                g.retained_bytes() + g.indices.len() / 3 * size_of::<(usize, usize, u32)>();
            if estimated > budget {
                continue;
            }
            if live_bytes + estimated > GPU_GEOMETRY {
                // Evict out-of-view or strictly farther buffers, never the valid
                // near sections that can render while this request catches up.
                let far = request
                    .meshes
                    .iter()
                    .filter(|mesh| resident.contains_key(&mesh.mesh.key))
                    .filter(|mesh| {
                        distance(mesh.mesh.origin, request.camera)
                            > distance(g.origin, request.camera)
                    })
                    .max_by(|a, b| {
                        distance(a.mesh.origin, request.camera)
                            .total_cmp(&distance(b.mesh.origin, request.camera))
                    });
                if let Some(far) = far {
                    live_bytes -= resident.remove(&far.mesh.key).unwrap().section.bytes();
                    metrics.evicted += 1;
                }
            }
            if live_bytes + estimated > GPU_GEOMETRY {
                continue;
            }
            budget -= estimated;
            let section = LiveSection::new(
                &renderer,
                &g.mesh(),
                namespace.identity,
                namespace.split,
                namespace.sprites,
                estimated,
            )?;
            live_bytes += section.bytes();
            resident.insert(
                g.key,
                Resident {
                    revision: g.sequence,
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
                let section = &resident.get(&g.mesh.key)?.section;
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
        let scene_frame = LiveFrame {
            sections: &draw,
            identity: namespace.identity,
            lightmap: &request.light,
        };
        match scene.submit(&renderer, material, Some(&display), scene_frame, cancel)? {
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
