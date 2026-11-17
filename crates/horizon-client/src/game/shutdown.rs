//! MIT; Copyright (c) 2026 Horizon contributors.
//! Main keeps UIKit/GPU-facing state; the process coordinator owns teardown.
use super::*;
use horizon_client::save::{
    LevelShape,
    shutdown::{Actor, Stage},
};

impl Game<'_> {
    /// Idempotent transfer; never wait for a server/writer/LOD worker here.
    /// The authoritative world's Game remains on its own server thread.
    pub(super) fn begin_owned_ui_shutdown(&mut self) -> bool {
        if !self.options.execution_role.is_client()
            || !(self.window.is_some()
                || horizon_client::save::shutdown::is_event_thread()
                || cfg!(target_os = "ios"))
        {
            return false;
        }
        if self.exit_retirement.is_some() {
            return true;
        }
        #[cfg(feature = "helium-direct-metal")]
        if let Some(native) = self.direct_metal.as_mut() {
            native.invalidate("shutdown");
        }
        if let Some(server) = &self.integrated {
            server.request_stop();
        }
        if let Some(starting) = &self.integrated_starting {
            starting.handle.request_stop();
        }
        self.streaming.abort_generation();
        if let Some(lod) = &self.lod {
            lod.request_stop();
        }
        self.retire_remote_connection();
        if let Some(guest) = &mut self.splitscreen_guest {
            guest.begin_owned_ui_shutdown();
        }
        self.stop_splitscreen();
        self.end_screenshot_sequence(Err("cancelled: the game is closing".into()));
        self.pad_close_output();
        let placeholder = Streaming::new(Arc::new(remote::NoTerrain), self.worker_context(), 1);
        let mut streaming = Some(std::mem::replace(&mut self.streaming, placeholder));
        let mut lod = self.lod.take();
        self.shutdown_singleplayer = self.integrated.is_some();
        let mut server = self.integrated.take();
        let mut starting = self.integrated_starting.take();
        let mut resources = self.rest_loader.take();
        let mut players = Some(std::mem::take(&mut self.player_autosaves));
        let placeholder = Arc::new(SaveDir::new(PathBuf::new(), LevelShape::OVERWORLD));
        let mut save = Some(std::mem::replace(&mut self.save, placeholder));
        let mut lock = self.session_lock.take();
        let context = self.worker_context();
        let mut parked_io: Vec<_> = self
            .levels
            .retire_client_io(context)
            .into_iter()
            .map(|(stream, save)| (Some(stream), save))
            .collect();
        let storage = save.as_ref().unwrap().shutdown_id();
        horizon_client::save::shutdown::global().record(
            storage,
            Actor::Main,
            Stage::Requested,
            0,
            0,
        );
        self.exit_retirement = Some(horizon_threads::retirement::global().submit(move || {
            // These destructors join only on this coordinator worker. Keep the
            // server handle (including transferred session lock) until afterward.
            drop(lod.take());
            drop(streaming.take());
            if let Some(server) = server.as_mut() {
                server.halt().map_err(|e| e.to_string())?;
            }
            if let Some(starting) = starting.as_mut() {
                starting.handle.halt().map_err(|e| e.to_string())?;
            }
            if let Some(loader) = resources.take() {
                loader.finish().map_err(|e| e.to_string())?;
            }
            for (stream, save) in &mut parked_io {
                drop(stream.take());
                save.flush().map_err(|e| e.to_string())?;
            }
            if let Some(players) = players.as_mut() {
                players.completed(true);
                players.writes_finished().map_err(|e| e.to_string())?;
            }
            if let Some(save) = &save {
                save.flush().map_err(|e| e.to_string())?;
            }
            horizon_client::save::file_write_result().map_err(|e| e.to_string())?;
            drop(players.take());
            drop(save.take());
            parked_io.clear();
            drop(starting.take());
            drop(server.take());
            drop(lock.take());
            horizon_client::save::shutdown::global().record(
                storage,
                Actor::Main,
                Stage::LockReleased,
                0,
                0,
            );
            Ok(())
        }));
        true
    }
}

impl Drop for Game<'_> {
    fn drop(&mut self) {
        // Covers event-loop errors/early exits which bypass ApplicationHandler::exiting.
        self.begin_owned_ui_shutdown();
    }
}
