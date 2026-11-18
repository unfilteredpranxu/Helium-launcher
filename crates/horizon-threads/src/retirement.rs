//! MIT; Copyright (c) 2026 Horizon contributors.
//! Process-owned asynchronous retirement. Failed work keeps its captured owners;
//! dropping a UI ticket neither joins a worker nor releases those owners.
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicU8, Ordering},
};
type Body = Box<dyn FnMut() -> Result<(), String> + Send>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Complete,
    Failed,
}
struct Work {
    state: AtomicU8,
    body: Mutex<Option<Body>>,
    error: OnceLock<String>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
#[derive(Clone)]
pub struct Ticket(Arc<Work>);
impl Ticket {
    pub fn status(&self) -> Status {
        match self.0.state.load(Ordering::Acquire) {
            1 => Status::Complete,
            2 => Status::Failed,
            _ => Status::Pending,
        }
    }
    pub fn error(&self) -> Option<&str> {
        self.0.error.get().map(String::as_str)
    }
}
#[derive(Default)]
pub struct Coordinator {
    work: Mutex<Vec<Arc<Work>>>,
    changed: Arc<(Mutex<()>, Condvar)>,
}
impl Coordinator {
    pub fn submit(&self, body: impl FnMut() -> Result<(), String> + Send + 'static) -> Ticket {
        let work = Arc::new(Work {
            state: AtomicU8::new(0),
            body: Mutex::new(Some(Box::new(body))),
            error: OnceLock::new(),
            thread: Mutex::new(None),
        });
        self.work
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(work.clone());
        let worker = work.clone();
        let changed = self.changed.clone();
        match std::thread::Builder::new()
            .name("horizon-retirement".into())
            .spawn(move || {
                let mut owned = worker.body.lock().unwrap_or_else(|p| p.into_inner());
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    owned.as_mut().unwrap()()
                }));
                match result {
                    Ok(Ok(())) => {
                        // Drop successful owners here, before publishing completion.
                        owned.take();
                        publish(&worker, &changed, 1);
                    }
                    other => {
                        let error = match other {
                            Ok(Err(e)) => e,
                            _ => "retirement worker panicked; owners retained".into(),
                        };
                        let _ = worker.error.set(error);
                        publish(&worker, &changed, 2);
                    }
                }
            }) {
            Ok(thread) => *work.thread.lock().unwrap_or_else(|p| p.into_inner()) = Some(thread),
            Err(error) => {
                // The closure/owners were parked BEFORE spawn. A failed spawn
                // must not destroy them on the submitting UI thread.
                let _ = work.error.set(error.to_string());
                publish(&work, &self.changed, 2);
            }
        }
        Ticket(work)
    }
    /// Reap only terminal threads. UI never waits for the body mutex or a live
    /// worker, and failures remain in this process-owned registry.
    pub fn poll(&self) {
        let Ok(mut work) = self.work.try_lock() else {
            return;
        };
        work.retain(|w| {
            if w.state.load(Ordering::Acquire) != 1 {
                return true;
            }
            let Ok(mut slot) = w.thread.try_lock() else {
                return true;
            };
            if slot.as_ref().is_some_and(|t| t.is_finished()) {
                let _ = slot.take().unwrap().join();
                false
            } else {
                true
            }
        });
    }
    pub fn pending(&self) -> bool {
        self.work.try_lock().map_or(true, |w| {
            w.iter().any(|v| v.state.load(Ordering::Acquire) != 1)
        })
    }
    /// After the application loop has returned and dropped its owners, a
    /// Complete result authorizes controlled process exit. Failed owners still
    /// hold their data and locks.
    pub fn exit_status(&self) -> Status {
        let work = self.work.lock().unwrap_or_else(|p| p.into_inner());
        let mut status = Status::Complete;
        for item in work.iter() {
            match item.state.load(Ordering::Acquire) {
                2 => return Status::Failed,
                1 => {}
                _ => status = Status::Pending,
            }
        }
        status
    }
    /// Only for an outer process boundary whose window backend has already
    /// returned. A running UI loop must use status polling instead.
    pub fn wait_for_exit_change(&self, observed: Status) {
        let (lock, changed) = &*self.changed;
        let guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        drop(changed.wait_while(guard, |_| self.exit_status() == observed));
    }
    /// Thread quiescence is not save completion; failed owners stay retained.
    pub fn workers_stopped(&self) -> bool {
        self.work.try_lock().is_ok_and(|w| {
            w.iter().all(|v| {
                v.thread
                    .try_lock()
                    .is_ok_and(|t| t.as_ref().is_none_or(|t| t.is_finished()))
            })
        })
    }
}
fn publish(work: &Work, changed: &(Mutex<()>, Condvar), state: u8) {
    let _guard = changed.0.lock().unwrap_or_else(|p| p.into_inner());
    work.state.store(state, Ordering::Release);
    changed.1.notify_all();
}
pub fn global() -> &'static Coordinator {
    static GLOBAL: OnceLock<Coordinator> = OnceLock::new();
    GLOBAL.get_or_init(Coordinator::default)
}
