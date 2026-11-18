//! Controlled process return, after all game scopes have handed off retirement.
use horizon_threads::retirement::{self, Status};

pub(super) fn run(game: impl FnOnce() -> crate::Result<()>) -> crate::Result<()> {
    let result = game();
    let retirement = retirement::global();
    let mut reported_failure = false;
    let mut complete = || match retirement.exit_status() {
        Status::Complete => {
            #[cfg(all(target_vendor = "apple", feature = "helium-direct-metal"))]
            if horizon_metal_bridge::runtime::retirement_pending() {
                return false;
            }
            true
        }
        status => {
            if status == Status::Failed && !reported_failure {
                reported_failure = true;
                log::error!("Horizon: shutdown failed; world ownership retained");
            }
            false
        }
    };
    if complete() {
        return result;
    }
    if let Err(error) = &result {
        log::error!("Horizon: runtime returned with owned retirement: {error}");
    }
    // On Apple platforms continue dispatching native run-loop sources. There
    // is no winit event handler left to schedule another AboutToWait callback.
    #[cfg(target_vendor = "apple")]
    horizon_ios_launcher::service_retirement(&mut complete);
    // Other platforms reach this boundary after the window backend is gone.
    // Wait on the coordinator's state signal without joining any worker.
    #[cfg(not(target_vendor = "apple"))]
    loop {
        let observed = retirement.exit_status();
        if complete() {
            break;
        }
        if observed != Status::Complete {
            retirement.wait_for_exit_change(observed);
        }
    }
    retirement.poll();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::save::{LevelShape, SaveDir, SessionLock};
    use horizon_world::nbt::Tag;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        path::PathBuf,
        process::{Command, Stdio},
        sync::mpsc,
        time::Duration,
    };

    const CHILD_WORLD: &str = "HORIZON_S1_FATAL_RETURN_WORLD";
    const READY: &str = "S1 event loop returned";

    #[test]
    fn early_error_without_any_world_returns_its_original_error() {
        let result = run(|| Err("startup failed before window creation".into()));
        assert_eq!(
            result.unwrap_err().to_string(),
            "startup failed before window creation"
        );
    }

    #[test]
    #[ignore = "subprocess fixture run by fatal_return_does_not_exit_until_the_dirty_world_is_durable"]
    fn fatal_return_child() {
        horizon_threads::catch_stop_signals(false);
        let root = PathBuf::from(std::env::var_os(CHILD_WORLD).unwrap());
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("helium-s1-")
        );
        let result = run(|| -> crate::Result<()> {
            let lock = SessionLock::acquire(&root)?;
            let save = SaveDir::new(root, LevelShape::OVERWORLD);
            let dirty = Tag::compound([("fixture", Tag::Int(42))]);
            // Model a fatal backend return with Game dropped before its
            // authoritative server admits the final dirty state.
            let mut owners = Some((save, lock, dirty));
            horizon_threads::retirement::global().submit(move || {
                let mut permit = [0];
                std::io::stdin()
                    .read_exact(&mut permit)
                    .map_err(|e| e.to_string())?;
                let (save, _, dirty) = owners.as_ref().unwrap();
                save.save_entities_later(vec![((0, 0), vec![dirty.clone()])])
                    .map_err(|e| e.to_string())?;
                save.flush().map_err(|e| e.to_string())?;
                drop(owners.take());
                Ok(())
            });
            println!("{READY}");
            Err("synthetic fatal event-loop error".into())
        });
        assert!(result.is_err());
        // Same process-exit boundary reached by entry::main_with on error.
        std::process::exit(17);
    }

    #[test]
    fn fatal_return_does_not_exit_until_the_dirty_world_is_durable() {
        let root = std::env::temp_dir().join(format!(
            "helium-s1-{}-{}",
            std::process::id(),
            horizon_client::save::shutdown::new_storage_id()
        ));
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "game::runtime_exit::tests::fatal_return_child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD_WORLD, &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (ready, observed) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.as_deref().is_ok_and(|line| line.ends_with(READY)) {
                    ready.send(()).unwrap();
                    break;
                }
            }
        });
        if let Err(error) = observed.recv_timeout(Duration::from_secs(10)) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fixture failed to reach exit boundary: {error}");
        }
        // The parent withholds admission until after an exit status probe.
        let until = std::time::Instant::now() + Duration::from_millis(150);
        let mut exited_early = false;
        while std::time::Instant::now() < until {
            if child.try_wait().unwrap().is_some() {
                exited_early = true;
                break;
            }
            assert!(SessionLock::is_locked(&root));
            std::thread::sleep(Duration::from_millis(5));
        }
        // Multiple cooperative quit requests cannot override the active
        // barrier. Force-kill recovery is covered by separate save tests.
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if !exited_early {
            for _ in 0..2 {
                let status = Command::new("kill")
                    .args(["-TERM", &child.id().to_string()])
                    .status()
                    .unwrap();
                assert!(status.success());
            }
            assert!(SessionLock::is_locked(&root));
        }
        let _ = child.stdin.take().unwrap().write_all(b"x");
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() >= until {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("controlled exit did not complete after save release");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        reader.join().unwrap();
        let lock = SessionLock::acquire(&root).unwrap();
        let save = SaveDir::new(root.clone(), LevelShape::OVERWORLD);
        let recovered = save.load_entities(0, 0);
        drop((save, lock));
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(status.code(), Some(17));
        assert!(
            !exited_early,
            "process exited while dirty world was still owned"
        );
        assert_eq!(recovered, [Tag::compound([("fixture", Tag::Int(42))])]);
    }
}
