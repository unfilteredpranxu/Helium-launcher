//! A bounded view of accepted near terrain and a latest-only frame handoff.
use super::*;
use std::{collections::BTreeMap, sync::atomic::AtomicUsize};

const MIB: usize = 1024 * 1024;
const MAX_SECTIONS: usize = 64;
const HIGH: usize = 12 * MIB;
const LOW: usize = 8 * MIB;
const CPU_LIMIT: usize = 16 * MIB;
#[cfg(target_os = "ios")]
const ADMIT_BYTES: usize = 2 * MIB;

struct PinnedGeometry {
    mesh: Geometry,
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
    fn admit(&mut self, mesh: Geometry, camera: [f64; 3]) -> bool {
        self.remove(mesh.key);
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
            mesh.key,
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

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
pub(super) use ios::Live;

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client_render::gfx::material_snapshot::State;
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

    fn fixture(key: Key, sequence: u64, bytes: usize) -> Geometry {
        Geometry {
            key,
            origin: [key.0 * 16, key.1 * 16, key.2 * 16],
            generation: State::new().generation(),
            sequence,
            bounds: [[0.; 3]; 2],
            preparation_us: 0,
            omitted_translucent_vertices: 0,
            vertices: vec![0; bytes],
            indices: vec![],
        }
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
        assert_eq!((first.mesh.sequence, second.mesh.sequence), (7, 8));
        meshes.clear();
        assert_eq!(meshes.retained.load(Ordering::Acquire), CPU_LIMIT);
        drop((first, second, old));
        assert_eq!(meshes.retained.load(Ordering::Acquire), 0);
    }
}
