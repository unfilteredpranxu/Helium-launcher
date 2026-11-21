//! Standalone no-world policy shared with the production UIKit integration.
#[cfg(any(target_os = "ios", test))]
pub(crate) fn deny(request: crate::Request) {
    request.answer(r#"{"error":"no-world host qualification disables game commands"}"#);
}

#[derive(Default)]
pub struct Lifecycle {
    active: bool,
    blocked: bool,
    stopping: bool,
    owner: Option<u64>,
}

impl Lifecycle {
    pub fn activate(&mut self) {
        if !self.active {
            self.active = true;
            self.blocked = false;
        }
    }

    pub fn suspend(&mut self) {
        self.active = false;
    }

    pub fn block_until_activation(&mut self) {
        self.blocked = true;
    }

    pub fn stop(&mut self) {
        self.stopping = true;
    }

    pub fn can_attach(&self) -> bool {
        self.active && !self.blocked && !self.stopping && self.owner.is_none()
    }

    pub fn attached(&mut self, id: u64) {
        assert!(self.can_attach());
        self.owner = Some(id);
    }

    pub fn retire(&mut self, id: u64) -> bool {
        if self.owner != Some(id) {
            return false;
        }
        self.owner = None;
        true
    }
}

#[cfg(target_os = "ios")]
pub fn preflight() -> bool {
    use std::{path::PathBuf, sync::atomic::Ordering};
    crate::QUALIFICATION.store(true, Ordering::Release);
    std::env::args_os().len() == 1
        && std::env::var_os("HOME")
            .and_then(|home| PathBuf::from(home).canonicalize().ok())
            .and_then(|home| {
                crate::standalone_image(&home, c"horizon_qualification_container_matches")
            })
            .is_some()
}

/// Fixed, non-sensitive codes for the host evidence bridge. Swift serializes
/// worker calls to the UIKit thread and rejects stale positive GPU progress.
#[cfg(target_os = "ios")]
#[repr(u32)]
pub enum Event {
    Admitted = 1,
    WindowReady,
    Active,
    Inactive,
    Background,
    MemoryPressure,
    DeviceReady,
    DeviceUnavailable,
    AttachmentUnavailable,
    RendererReady,
    Submitted,
    PresentCompleted,
    Retired,
    WorkerFailed,
    Shutdown,
}

#[cfg(target_os = "ios")]
pub fn event(event: Event, id: u64) {
    if let Some(symbol) = crate::ios::load_symbol(c"horizon_qualification_event", false) {
        // SAFETY: fixed-width Swift C export, accepts calls from both threads.
        let send: extern "C" fn(u32, u64) = unsafe { std::mem::transmute(symbol) };
        send(event as u32, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Request;

    #[test]
    fn denies_world_open_create_import_and_other_bridge_commands() {
        for line in [
            "open saves/normal",
            "worlds",
            "world-create {}",
            "import ../world.zip",
            "launcher-start",
        ] {
            let (tx, rx) = std::sync::mpsc::channel();
            deny(Request::new(line, move |reply| {
                tx.send(reply.to_owned()).unwrap()
            }));
            assert_eq!(
                rx.recv().unwrap(),
                r#"{"error":"no-world host qualification disables game commands"}"#
            );
        }
    }

    #[test]
    fn cold_launch_suspend_reactivate_and_retire_exact_owner() {
        let mut host = Lifecycle::default();
        assert!(!host.can_attach());
        host.activate();
        host.attached(1);
        host.suspend();
        host.activate();
        assert!(!host.can_attach());
        assert!(host.retire(1));
        host.attached(2);
        assert!(!host.retire(1));
        assert!(!host.can_attach());
        assert!(host.retire(2));
        assert!(host.can_attach());
    }

    #[test]
    fn pressure_blocks_retries_and_shutdown_is_terminal() {
        let mut host = Lifecycle::default();
        host.activate();
        host.attached(3);
        host.block_until_activation();
        assert!(host.retire(3));
        host.activate();
        assert!(!host.can_attach());
        host.suspend();
        host.activate();
        host.attached(4);
        host.stop();
        assert!(host.retire(4));
        host.suspend();
        host.activate();
        assert!(!host.can_attach());
        assert!(
            Lifecycle {
                active: true,
                ..Default::default()
            }
            .can_attach()
        );
    }
}
