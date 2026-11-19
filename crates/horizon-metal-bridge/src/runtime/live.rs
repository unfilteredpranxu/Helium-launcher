//! A bounded view of accepted near terrain and a latest-only frame handoff.
use super::*;
use helium_render_scene::Section;
use std::{collections::BTreeMap, sync::atomic::AtomicUsize};

const MIB: usize = 1024 * 1024;
const MAX_SECTIONS: usize = 64;
const HIGH: usize = 12 * MIB;
const LOW: usize = 8 * MIB;
const CPU_LIMIT: usize = 16 * MIB;
#[cfg(target_os = "ios")]
const ADMIT_BYTES: usize = 2 * MIB;

struct PinnedGeometry {
    mesh: Section,
    bytes: usize,
    retained: Arc<AtomicUsize>,
}
impl Drop for PinnedGeometry {
    fn drop(&mut self) {
        self.retained.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct Meshes {
    entries: BTreeMap<Key, Arc<PinnedGeometry>>,
    retained: Arc<AtomicUsize>,
    bytes: usize,
    evictions: u64,
}
impl Meshes {
    fn remove(&mut self, key: Key) {
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
        }
    }
    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
    fn admit(&mut self, mesh: Section, camera: [f64; 3]) -> bool {
        self.remove(mesh.version.key);
        let bytes = mesh.retained_bytes();
        if self.retained.load(Ordering::Acquire) + bytes > CPU_LIMIT {
            return false;
        }
        let priority = distance(mesh.origin, camera);
        if self.bytes + bytes > HIGH || self.entries.len() >= MAX_SECTIONS {
            while self.bytes + bytes > LOW || self.entries.len() >= MAX_SECTIONS {
                let Some((key, far)) = self
                    .entries
                    .iter()
                    .map(|(&key, value)| (key, distance(value.mesh.origin, camera)))
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                else {
                    break;
                };
                if far < priority {
                    return false;
                }
                self.remove(key);
                self.evictions += 1;
            }
        }
        // Pipeline requests can still own evicted geometry. Keep its charge
        // until the last reference drops, even across renderer/world changes.
        self.retained.fetch_add(bytes, Ordering::AcqRel);
        self.bytes += bytes;
        self.entries.insert(
            mesh.version.key,
            Arc::new(PinnedGeometry {
                mesh,
                bytes,
                retained: self.retained.clone(),
            }),
        );
        true
    }
}
fn distance(origin: [i32; 3], camera: [f64; 3]) -> f64 {
    origin
        .into_iter()
        .zip(camera)
        .map(|(o, c)| (f64::from(o) + 8. - c).powi(2))
        .sum()
}

/// A slow GPU should consume the newest camera, with no UI lock waits and no
/// stale frame backlog. Retired items are dropped outside the mutex.
struct Latest<T> {
    slot: std::sync::Mutex<Option<T>>,
    wake: std::sync::Condvar,
}
impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            slot: Default::default(),
            wake: Default::default(),
        }
    }
}
impl<T> Latest<T> {
    fn offer(&self, item: T) -> bool {
        let Ok(mut slot) = self.slot.try_lock() else {
            return false;
        };
        let old = slot.replace(item);
        drop(slot);
        self.wake.notify_one();
        old.is_none()
    }
    fn take(&self, wait: Duration) -> Option<T> {
        let slot = self.slot.lock().ok()?;
        let (mut slot, _) = self
            .wake
            .wait_timeout_while(slot, wait, |item| item.is_none())
            .ok()?;
        slot.take()
    }
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
    #[cfg(target_os = "ios")]
    last: Option<Instant>,
    cpu: std::collections::VecDeque<u64>,
}
impl Metrics {
    #[cfg(target_os = "ios")]
    fn record(&mut self, result: helium_metal_prototype::terrain::LiveCompletion, prepared: u64) {
        self.completed += 1;
        self.draws += result.draws;
        self.encode_us = result.encode_time.as_micros() as u64;
        self.gpu_us = result.gpu_time.map(|t| t.as_micros() as u64);
        self.last_present_us = result.completion_latency.as_micros() as u64;
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

    #[cfg(target_os = "ios")]
    fn report(
        &self,
        epoch: u64,
        dropped: u64,
        cpu_geometry: usize,
        gpu_geometry: usize,
        targets: usize,
    ) {
        log::info!(
            "{}",
            self.summary(epoch, dropped, cpu_geometry, gpu_geometry, targets)
        );
    }

    fn summary(
        &self,
        epoch: u64,
        dropped: u64,
        cpu_geometry: usize,
        gpu_geometry: usize,
        targets: usize,
    ) -> String {
        let mut samples: Vec<_> = self.cpu.iter().copied().collect();
        samples.sort_unstable();
        let quantile = |p| {
            samples
                .get(samples.len().saturating_sub(1) * p / 100)
                .copied()
        };
        format!(
            "Helium S2B source=game epoch={epoch} complete={completed} skipped={skipped} replaced_requests={dropped} \
             draws={draws} geometry_uploads={uploads} evicted={evicted} cpu_geometry={cpu_geometry} \
             gpu_geometry={gpu_geometry} peak_gpu_geometry={peak_geometry} material={material} \
             staging_peak={staging} targets={targets} cpu_us_p50={p50:?} p95={p95:?} p99={p99:?} \
             admit_us={admitted_us} cull_us={cull_us} encode_us={encode_us} upload_us={upload_us} \
             completion_us={last_present_us} gpu_us={gpu_us:?} completed_hz={rate_hz:.1} \
             cpu_evictions={cpu_evictions} omitted_translucent_vertices={omitted_vertices} \
             stalls_gt_50_ms={stalls_50} stalls_gt_100_ms={stalls_100}; scanout unverified",
            completed = self.completed,
            skipped = self.skipped,
            draws = self.draws,
            uploads = self.uploads,
            evicted = self.evicted,
            peak_geometry = self.peak_geometry,
            material = self.material,
            staging = self.staging,
            p50 = quantile(50),
            p95 = quantile(95),
            p99 = quantile(99),
            admitted_us = self.admitted_us,
            cull_us = self.cull_us,
            encode_us = self.encode_us,
            upload_us = self.upload_us,
            last_present_us = self.last_present_us,
            gpu_us = self.gpu_us,
            rate_hz = self.rate_hz,
            cpu_evictions = self.cpu_evictions,
            omitted_vertices = self.omitted_vertices,
            stalls_50 = self.stalls_50,
            stalls_100 = self.stalls_100,
        )
    }
}

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
pub(super) use ios::Live;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn telemetry_distinguishes_terminal_latency_and_unknown_gpu_time() {
        let metrics = Metrics {
            completed: 2,
            cpu: (1..=100).collect(),
            last_present_us: 27,
            stalls_50: 1,
            ..Metrics::default()
        };
        let line = metrics.summary(3, 5, 6, 7, 8);
        assert!(line.contains("complete=2 skipped=0 replaced_requests=5"));
        assert!(line.contains("cpu_us_p50=Some(50) p95=Some(95) p99=Some(99)"));
        assert!(line.contains("completion_us=27 gpu_us=None"));
        assert!(line.contains("stalls_gt_50_ms=1 stalls_gt_100_ms=0"));
        assert!(line.contains("scanout unverified"));
    }

    #[test]
    fn slow_consumer_gets_only_latest_camera_after_long_session() {
        let queue = Latest::default();
        for frame in 0..10_000 {
            assert_eq!(queue.offer(frame), frame == 0);
        }
        assert_eq!(queue.take(Duration::ZERO), Some(9_999));
        assert_eq!(queue.take(Duration::ZERO), None);
        let locked = queue.slot.lock().unwrap();
        assert!(!queue.offer(10_000));
        drop(locked);
        assert!(queue.offer(10_001));
    }

    fn fixture(key: Key, sequence: u64, bytes: usize) -> Section {
        // Byte-budget fixture only. It never submits geometry to the GPU.
        let mut vertices = Vec::with_capacity(bytes);
        vertices.resize(bytes / 24 * 24, 0);
        Section::new(
            helium_render_scene::SectionVersion {
                epoch: std::num::NonZeroU64::MIN,
                generation: 1,
                key,
                revision: sequence,
            },
            [key.0 * 16, key.1 * 16, key.2 * 16],
            [[0.; 3]; 2],
            0,
            vertices,
            vec![],
            bytes,
        )
        .unwrap()
    }

    #[test]
    fn replacements_evictions_and_old_frame_pins_stay_charged() {
        let mut meshes = Meshes::default();
        for x in 10..16 {
            assert!(meshes.admit(fixture((x, 0, 0), 1, 2 * MIB), [0.; 3]));
        }
        let old: Vec<_> = meshes.entries.values().cloned().collect();
        assert!(meshes.admit(fixture((0, 0, 0), 7, 2 * MIB), [0.; 3]));
        assert_eq!(meshes.bytes, LOW);
        assert_eq!(meshes.retained.load(Ordering::Acquire), 14 * MIB);
        let first = meshes.entries[&(0, 0, 0)].clone();
        assert!(meshes.admit(fixture((0, 0, 0), 8, 2 * MIB), [0.; 3]));
        let second = meshes.entries[&(0, 0, 0)].clone();
        assert!(!meshes.admit(fixture((0, 0, 0), 9, 2 * MIB), [0.; 3]));
        assert!(!meshes.entries.contains_key(&(0, 0, 0)));
        assert_eq!(
            (first.mesh.version.revision, second.mesh.version.revision),
            (7, 8)
        );
        meshes.clear();
        assert_eq!(meshes.retained.load(Ordering::Acquire), CPU_LIMIT);
        drop((first, second, old));
        assert_eq!(meshes.retained.load(Ordering::Acquire), 0);
    }
}
