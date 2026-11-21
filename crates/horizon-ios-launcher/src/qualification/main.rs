//! The game's UIKit host and attachment ABI with no game, world, or importer dependency.
#![forbid(unsafe_code)]

#[cfg(not(target_os = "ios"))]
fn main() -> std::process::ExitCode {
    eprintln!("Helium host qualification requires standalone iOS.");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "ios")]
mod metal;

#[cfg(target_os = "ios")]
mod ios {
    use helium_metal_prototype::attached::{self, Ticket};
    use horizon_ios_launcher::qualification::{Event, Lifecycle, event, preflight};
    use std::{
        sync::{Arc, atomic::AtomicBool, mpsc::Receiver},
        time::{Duration, Instant},
    };
    use winit::{
        application::ApplicationHandler,
        dpi::PhysicalSize,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
        window::{Window, WindowId},
    };

    struct Session {
        ticket: Ticket,
        receipt: Receiver<Result<(), String>>,
        ended: bool,
        size: PhysicalSize<u32>,
    }

    #[derive(Default)]
    struct Host {
        window: Option<Window>,
        lifecycle: Lifecycle,
        session: Option<Session>,
        ready_until: Option<Instant>,
    }

    impl Host {
        fn cancel(&self) {
            if let Some(session) = &self.session
                && !session.ticket.is_cancelled()
            {
                session.ticket.cancel();
            }
        }

        fn attach(&mut self) -> Result<(), String> {
            let window = self.window.as_ref().ok_or("no UIKit window")?;
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            let (ticket, receipt) = attached::submit_qualification(window, cancel, move |layer| {
                super::metal::render(layer, &worker_cancel)
            })?;
            self.lifecycle.attached(ticket.diagnostic_id());
            self.session = Some(Session {
                ticket,
                receipt,
                ended: false,
                size: window.inner_size(),
            });
            Ok(())
        }

        fn poll_retirement(&mut self) {
            let Some(session) = &mut self.session else {
                return;
            };
            let id = session.ticket.diagnostic_id();
            if !session.ended {
                match session.receipt.try_recv() {
                    Err(std::sync::mpsc::TryRecvError::Empty) => return,
                    Ok(Ok(())) => (),
                    Err(std::sync::mpsc::TryRecvError::Disconnected)
                        if session.ticket.is_cancelled() =>
                    {
                        ()
                    }
                    Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        event(Event::WorkerFailed, id);
                        self.lifecycle.block_until_activation();
                    }
                }
                session.ended = true;
            }
            if !attached::work_pending() && self.lifecycle.retire(id) {
                event(Event::Retired, id);
                self.session = None;
                self.retry_window();
            }
        }

        fn retry_window(&mut self) {
            self.ready_until = Some(Instant::now() + Duration::from_secs(4));
        }
    }

    impl ApplicationHandler for Host {
        fn resumed(&mut self, app: &ActiveEventLoop) {
            if self.window.is_none() {
                match app.create_window(
                    Window::default_attributes().with_title("Helium Host Qualification"),
                ) {
                    Ok(window) => self.window = Some(window),
                    Err(_) => {
                        event(Event::AttachmentUnavailable, 0);
                        return;
                    }
                }
                event(Event::WindowReady, 0);
                if attached::qualification_device_available() != Ok(true) {
                    event(Event::DeviceUnavailable, 0);
                    self.lifecycle.stop();
                    return;
                }
                event(Event::DeviceReady, 0);
            }
            event(Event::Active, 0);
            self.lifecycle.activate();
            self.retry_window();
        }

        fn suspended(&mut self, _: &ActiveEventLoop) {
            event(Event::Inactive, 0);
            self.lifecycle.suspend();
            self.cancel();
        }

        fn memory_warning(&mut self, _: &ActiveEventLoop) {
            event(Event::MemoryPressure, 0);
            self.lifecycle.block_until_activation();
            self.cancel();
        }

        fn window_event(&mut self, _: &ActiveEventLoop, id: WindowId, change: WindowEvent) {
            if !self.window.as_ref().is_some_and(|window| window.id() == id) {
                return;
            }
            match change {
                WindowEvent::Occluded(true) => {
                    event(Event::Background, 0);
                    self.lifecycle.suspend();
                    self.cancel();
                }
                WindowEvent::Resized(size)
                    if self.session.as_ref().is_some_and(|s| size != s.size) =>
                {
                    self.cancel()
                }
                WindowEvent::Destroyed | WindowEvent::CloseRequested => {
                    self.lifecycle.stop();
                    self.cancel();
                    event(Event::Shutdown, 0);
                }
                _ => (),
            }
        }

        fn about_to_wait(&mut self, app: &ActiveEventLoop) {
            self.poll_retirement();
            if self.lifecycle.can_attach() {
                if !attached::work_pending() && self.attach().is_ok() {
                    self.ready_until = None;
                } else if self
                    .ready_until
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    event(Event::AttachmentUnavailable, 0);
                    self.lifecycle.block_until_activation();
                }
            }
            if self.lifecycle.can_attach() || self.session.is_some() {
                app.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_millis(100),
                ));
            } else {
                app.set_control_flow(ControlFlow::Wait);
            }
        }

        fn exiting(&mut self, _: &ActiveEventLoop) {
            self.lifecycle.stop();
            self.cancel();
            event(Event::Shutdown, 0);
        }
    }

    pub fn start() -> Result<(), &'static str> {
        if !preflight() {
            return Err("standalone identity or qualification bundle rejected");
        }
        event(Event::Admitted, 0);
        EventLoop::new()
            .map_err(|_| "UIKit event loop unavailable")?
            .run_app(&mut Host::default())
            .map_err(|_| "UIKit event loop failed")
    }
}

#[cfg(target_os = "ios")]
fn main() -> std::process::ExitCode {
    match ios::start() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Helium Host Qualification: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
