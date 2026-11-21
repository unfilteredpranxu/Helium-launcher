//! The save screen freezes simulation, but keeps pumping window events while
//! snapshots are prepared in small batches and the region workers drain.
use super::{Game, loading_frame};
use horizon_client::{Result, hud::HudBatch};
use std::{
    collections::{HashSet, VecDeque},
    time::{Duration, Instant},
};

pub(super) struct ClosingSave {
    pub quit: bool,
    close: bool,
    reply: Option<std::sync::mpsc::Sender<String>>,
    started: Instant,
    stage: Stage,
}
impl ClosingSave {
    fn initial_close(
        quit: bool,
        has_server: bool,
        title_or_saved: bool,
        retire: impl FnOnce() -> bool,
    ) -> Option<Self> {
        // World creation can retain the title screen while the integrated
        // server owns a session. Resolve that owner before treating the UI as idle.
        let stage = if has_server && retire() {
            Stage::Retiring
        } else if title_or_saved {
            Stage::Quit
        } else {
            return None;
        };
        Some(Self {
            quit,
            close: true,
            reply: None,
            started: Instant::now(),
            stage,
        })
    }

    fn retire_after_error(&mut self) {
        self.quit = true;
        self.close = true;
        self.stage = Stage::Retiring;
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(
                serde_json::json!({"error":"game error; owned shutdown pending"}).to_string(),
            );
        }
    }
}
enum Stage {
    Generation,
    Chunks(VecDeque<(i32, i32)>),
    Entities(VecDeque<(i32, i32)>),
    Writes,
    Server,
    Retiring,
    Quit,
}

// Bound the work in both sparse and block-entity-heavy worlds. A single chunk
// is indivisible; check the time budget after each snapshot, not each batch.
const SNAPSHOTS_PER_FRAME: usize = 16;
const SNAPSHOT_BUDGET: Duration = Duration::from_millis(2);

impl Game<'_> {
    /// An ordinary game error still has a live event loop. Keep pumping it
    /// until the real owned retirement completes; never return the Rust main
    /// while a normal error path still owns an unsaved authoritative world.
    pub(super) fn wait_for_owned_exit(&mut self) {
        if let Some(save) = &mut self.pending_save {
            save.retire_after_error();
            return;
        }
        self.pending_save = Some(ClosingSave {
            quit: true,
            close: true,
            reply: None,
            started: Instant::now(),
            stage: Stage::Retiring,
        });
    }
    pub(super) fn begin_closing_save(&mut self, quit: bool) -> Result<()> {
        if let Some(save) = &mut self.pending_save {
            save.quit |= quit;
            return Ok(());
        }
        let has_server = self.integrated.is_some()
            || self.integrated_starting.is_some()
            || self.remote.is_some();
        if let Some(save) = ClosingSave::initial_close(
            quit,
            has_server,
            self.title.is_some() || self.saved_for_exit,
            || self.begin_owned_ui_shutdown(),
        ) {
            let retiring = matches!(save.stage, Stage::Retiring);
            self.pending_save = Some(save);
            if retiring {
                self.capture(false);
            }
            return Ok(());
        }
        // Stop client-side terrain work before the integrated server is shut down. The
        // server's own stop path aborts its workers, but the window's streaming registration
        // is dropped only after the event loop exits and would otherwise wait for queued work.
        self.streaming.abort_generation();
        if let Some(integrated) = &self.integrated {
            integrated.request_stop();
            self.pending_save = Some(ClosingSave {
                quit,
                close: true,
                reply: None,
                started: Instant::now(),
                stage: Stage::Server,
            });
        } else if self.remote.is_some() {
            if !quit {
                return self.leave_server(None);
            }
            if let Some(mut remote) = self.remote.take() {
                remote.session.disconnect();
            }
            self.saved_for_exit = true;
            self.pending_save = Some(ClosingSave {
                quit,
                close: true,
                reply: None,
                started: Instant::now(),
                stage: Stage::Quit,
            });
        } else {
            self.close_container();
            self.award_custom_stat("leave_game", 1);
            self.lan_close(false);
            self.free_player_beds()?;
            self.pending_save = Some(ClosingSave {
                quit,
                close: true,
                reply: None,
                started: Instant::now(),
                stage: Stage::Generation,
            });
        }
        self.capture(false);
        Ok(())
    }

    /// Control saves acknowledge only after durable completion, while their
    /// snapshot work is spread across event-loop turns too.
    pub(super) fn begin_control_save(
        &mut self,
        all: bool,
        reply: std::sync::mpsc::Sender<String>,
    ) -> Result<()> {
        if self.pending_save.is_some() {
            return Err("a world save is already in progress".into());
        }
        if all {
            self.world
                .modified
                .extend(self.world.chunks.keys().copied());
        }
        self.keys.clear();
        self.physical_keys.clear();
        self.left = false;
        self.right = false;
        self.pending_save = Some(ClosingSave {
            quit: false,
            close: false,
            reply: Some(reply),
            started: Instant::now(),
            stage: Stage::Generation,
        });
        Ok(())
    }

    /// `true` means the window can exit. The session lock stays in Game until
    /// every snapshot has been queued and every writer has synced its files.
    pub(super) fn poll_closing_save(&mut self) -> Result<bool> {
        let Some(mut save) = self.pending_save.take() else {
            return Ok(false);
        };
        let result = (|| -> Result<bool> {
            match &mut save.stage {
                Stage::Quit => Ok(true),
                Stage::Retiring => {
                    let ticket = self
                        .exit_retirement
                        .as_ref()
                        .ok_or("shutdown ownership missing")?;
                    match ticket.status() {
                        horizon_threads::retirement::Status::Pending => Ok(false),
                        horizon_threads::retirement::Status::Failed => Err(ticket
                            .error()
                            .unwrap_or("shutdown failed; owners retained")
                            .to_owned()
                            .into()),
                        horizon_threads::retirement::Status::Complete => {
                            if !save.quit {
                                let reason = self.shutdown_disconnect_reason.take();
                                self.leave_server(reason)?;
                                self.exit_retirement = None;
                            }
                            Ok(true)
                        }
                    }
                }
                Stage::Server => {
                    let server = self
                        .integrated
                        .as_ref()
                        .ok_or("the integrated server stopped with an error")?;
                    if !server.stopped() {
                        return Ok(false);
                    }
                    // Drop/join only after completion; leave_server's teardown
                    // can then run without blocking on the server's writes.
                    if save.quit {
                        self.integrated.take().unwrap().stop()?;
                    } else {
                        self.leave_server(None)?;
                    }
                    Ok(true)
                }
                Stage::Generation => {
                    if save.started.elapsed() > Duration::from_secs(30) {
                        return Err("chunk generation did not finish before saving".into());
                    }
                    if self.streaming.poll_save(&mut self.world) {
                        save.stage = Stage::Chunks(self.world.modified.iter().copied().collect());
                    }
                    Ok(false)
                }
                Stage::Chunks(keys) => {
                    let started = Instant::now();
                    for _ in 0..SNAPSHOTS_PER_FRAME {
                        let Some(&key) = keys.front() else { break };
                        let chunks = self.with_entities(self.world.chunks.get(&key));
                        self.save.save_later(
                            &chunks,
                            self.pack.assets.registry,
                            world_biomes!(self),
                        )?;
                        self.world.modified.remove(&key);
                        keys.pop_front();
                        if started.elapsed() >= SNAPSHOT_BUDGET {
                            break;
                        }
                    }
                    if keys.is_empty() {
                        self.spawn_loaded_entities();
                        save.stage = Stage::Entities(self.world.chunks.keys().copied().collect());
                    }
                    Ok(false)
                }
                Stage::Entities(keys) => {
                    let started = Instant::now();
                    for _ in 0..SNAPSHOTS_PER_FRAME {
                        let Some(&key) = keys.front() else { break };
                        let chunks = self.entity_chunks(&HashSet::from([key]), false);
                        self.save.save_entities_later(chunks)?;
                        keys.pop_front();
                        if started.elapsed() >= SNAPSHOT_BUDGET {
                            break;
                        }
                    }
                    if keys.is_empty() {
                        self.prune_entity_memory();
                        self.save_raids();
                        self.save.save_attachments(&self.world.attachments)?;
                        self.save.save_forced_chunks(&self.streaming.forced)?;
                        self.save_game_metadata()?;
                        save.stage = Stage::Writes;
                    }
                    Ok(false)
                }
                Stage::Writes => {
                    let players = self.finish_player_autosaves(false)?;
                    Ok(self.save.writes_finished()? && players)
                }
            }
        })();
        match result {
            Ok(true) => {
                if let Some(reply) = save.reply.take() {
                    let _ = reply.send(serde_json::json!({ "ok": true }).to_string());
                }
                if save.quit && !save.close {
                    // A window close during a control save still has to close
                    // containers and save the resulting inventory changes.
                    self.begin_closing_save(true)?;
                    return Ok(false);
                }
                if save.quit {
                    self.saved_for_exit = true;
                    // UIKit owns process lifetime and ignores programmatic
                    // Exit. Keep retired gameplay stopped on future turns.
                    #[cfg(target_os = "ios")]
                    {
                        save.stage = Stage::Quit;
                        self.pending_save = Some(save);
                    }
                    Ok(true)
                } else {
                    if save.close && self.title.is_none() {
                        self.finish_perf();
                        self.finish_close_world()?;
                    }
                    self.clock.reset();
                    self.previous = Instant::now();
                    Ok(false)
                }
            }
            other => {
                if let Err(error) = &other
                    && let Some(reply) = save.reply.take()
                {
                    let _ =
                        reply.send(serde_json::json!({ "error": error.to_string() }).to_string());
                }
                // Keep ownership (and the lock) on errors too. Native shutdown
                // sees the same durable-write failure instead of declaring success.
                self.pending_save = Some(save);
                other
            }
        }
    }

    pub(super) fn render_closing_save(&mut self) -> Result<()> {
        if matches!(
            self.pending_save.as_ref().map(|save| &save.stage),
            Some(Stage::Quit)
        ) {
            return Ok(());
        }
        // disconnectWithSavingScreen: a GenericMessageScreen, its title and
        // no navigation.
        horizon_client::widgets::begin_screen(|| {
            horizon_client::widgets::ScreenInfo::new(
                "saving",
                super::screen_info::tr("menu.savingLevel", &[]),
            )
            .without_navigation()
        });
        let Some(renderer) = &mut self.renderer else {
            return Ok(());
        };
        let mut hud = HudBatch::new([renderer.config.width, renderer.config.height]);
        let progress = match self.pending_save.as_ref().map(|save| &save.stage) {
            Some(Stage::Chunks(_)) => 0.25,
            Some(Stage::Entities(_)) => 0.5,
            Some(Stage::Writes) => 0.9,
            _ => 0.0,
        };
        horizon_client::level_loading::draw_progress(
            &mut hud,
            &self.gui.atlas,
            "Saving world...",
            progress,
        );
        renderer.render(&loading_frame(&hud))?;
        Ok(())
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    #[test]
    fn closing_from_title_waits_for_an_opening_world_owner() {
        use horizon_threads::retirement::{Coordinator, Status};
        use std::sync::mpsc::channel;

        let coordinator = Coordinator::default();
        let (release, wait) = channel();
        let mut ticket = None;
        let save = ClosingSave::initial_close(true, true, true, || {
            ticket = Some(coordinator.submit(move || wait.recv().map_err(|e| e.to_string())));
            true
        })
        .unwrap();
        assert!(matches!(save.stage, Stage::Retiring));
        let ticket = ticket.unwrap();
        assert_eq!(ticket.status(), Status::Pending);
        release.send(()).unwrap();
        coordinator.wait_for_exit_change(Status::Pending);
        assert_eq!(ticket.status(), Status::Complete);
    }

    #[test]
    fn idle_title_close_never_creates_a_retirement_ticket() {
        let save = ClosingSave::initial_close(true, false, true, || {
            panic!("there is no world owner to retire")
        })
        .unwrap();
        assert!(matches!(save.stage, Stage::Quit));
    }
    #[test]
    fn interrupted_control_save_cannot_reply_success_or_restart_a_local_save_after_retirement() {
        let (reply, receive) = std::sync::mpsc::channel();
        let mut save = ClosingSave {
            quit: false,
            close: false,
            reply: Some(reply),
            started: Instant::now(),
            stage: Stage::Generation,
        };
        save.retire_after_error();
        let result: serde_json::Value = serde_json::from_str(&receive.recv().unwrap()).unwrap();
        assert!(result.get("error").is_some());
        assert!(result.get("ok").is_none());
        assert!(
            !(save.quit && !save.close),
            "retirement must not enter the control-save-to-local-save retry branch"
        );
    }
    #[test]
    fn repeated_error_close_reports_once_and_never_creates_a_success_ack() {
        let (reply, receive) = std::sync::mpsc::channel();
        let mut save = ClosingSave {
            quit: true,
            close: true,
            reply: Some(reply),
            started: Instant::now(),
            stage: Stage::Writes,
        };
        save.retire_after_error();
        save.retire_after_error();
        assert!(receive.recv().unwrap().contains("error"));
        assert_eq!(
            receive.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        );
    }
}
