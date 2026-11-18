use super::*;
use crate::{
    chunk::LevelShape,
    save::{SaveDir, SessionLock},
};
use horizon_world::nbt::Tag;
use std::fs;

fn temporary() -> PathBuf {
    std::env::temp_dir().join(format!(
        "horizon-shutdown-fixture-{}-{}",
        std::process::id(),
        new_storage_id()
    ))
}
fn blocked(dir: PathBuf) -> (SaveDir, Arc<(Mutex<bool>, std::sync::Condvar)>) {
    let mut save = SaveDir::new(dir, LevelShape::OVERWORLD);
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    save.shutdown_gate = Some(gate.clone());
    (save, gate)
}
fn release(gate: &Arc<(Mutex<bool>, std::sync::Condvar)>) {
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
}
#[test]
fn clean_world_is_complete_and_a_five_second_dirty_drain_retains_lock_and_ack() {
    let dir = temporary();
    let lock = SessionLock::acquire(&dir).unwrap();
    let (save, gate) = blocked(dir.clone());
    assert!(save.try_write_status().unwrap().finished());
    assert!(save.flush_until(Instant::now()).unwrap());
    let tag = Tag::compound([("fixture", Tag::Int(73))]);
    save.save_entities_later(vec![((0, 0), vec![tag.clone()])])
        .unwrap();
    assert!(!save.try_write_status().unwrap().finished());
    assert_eq!(save.try_write_status().unwrap().outstanding_batches, 1);
    let start = Instant::now();
    // WORKER-side bounded barrier: a separate main/probe loop remains responsive.
    std::thread::scope(|scope| {
        let (done, receive) = std::sync::mpsc::channel();
        let worker_save = &save;
        let writer = scope.spawn(move || {
            done.send(
                worker_save
                    .flush_until(start + Duration::from_secs(5))
                    .map_err(|e| e.to_string()),
            )
            .unwrap()
        });
        let mut probes = 0;
        while start.elapsed() < Duration::from_secs(5) {
            assert!(!save.try_write_status().is_some_and(WriteStatus::finished));
            probes += 1;
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !receive
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()
        );
        assert!(probes > 10);
        assert!(!save.writes_finished().unwrap());
        assert!(
            SessionLock::acquire(&dir).is_err(),
            "deadline must not release session ownership"
        );
        release(&gate);
        save.flush().unwrap();
        writer.join().unwrap();
    });
    assert!(save.try_write_status().unwrap().finished());
    // Acknowledged contents really reopen from the region file.
    let reopened = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    assert_eq!(reopened.load_entities(0, 0), vec![tag]);
    drop(reopened);
    drop(save);
    assert!(SessionLock::acquire(&dir).is_err());
    drop(lock);
    assert!(SessionLock::acquire(&dir).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn failed_region_write_stays_failed_and_readable_on_late_probes() {
    let dir = temporary();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("entities"), b"not a directory").unwrap();
    let lock = SessionLock::acquire(&dir).unwrap();
    let save = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    let tag = Tag::compound([("fixture", Tag::Int(19))]);
    save.save_entities_later(vec![((0, 0), vec![tag.clone()])])
        .unwrap();
    assert!(
        save.flush_until(Instant::now() + Duration::from_secs(1))
            .is_err()
    );
    assert!(save.flush().is_err());
    let status = save.try_write_status().unwrap();
    assert!(status.failed && !status.finished());
    assert_eq!(status.outstanding_batches, 0);
    assert_eq!(status.pending_records, 1);
    assert_eq!(save.load_entities(0, 0), vec![tag]);
    assert!(save.writes_finished().is_err());
    assert!(save.writes_finished().is_err());
    assert!(SessionLock::acquire(&dir).is_err());
    drop(save);
    drop(lock);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn drop_really_waits_for_the_writer_and_cannot_be_relabelled_as_an_exit_fix() {
    let dir = temporary();
    let lock = SessionLock::acquire(&dir).unwrap();
    let (save, gate) = blocked(dir.clone());
    save.save_entities_later(vec![(
        (0, 0),
        vec![Tag::compound([("fixture", Tag::Int(1))])],
    )])
    .unwrap();
    let (done, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        drop(save);
        drop(lock);
        done.send(()).unwrap();
    });
    assert!(receive.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(SessionLock::acquire(&dir).is_err());
    release(&gate);
    receive.recv_timeout(Duration::from_secs(3)).unwrap();
    worker.join().unwrap();
    assert!(SessionLock::acquire(&dir).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn first_errors_survive_bounded_trace_pressure_without_private_text() {
    let trace = Trace::new(Some(std::thread::current().id()));
    trace.record(2, Actor::Region, Stage::Failed, 3, 1);
    for n in 0..1000 {
        trace.record(2, Actor::Region, Stage::Queued, n, 1);
    }
    let (_, text, _) = trace.numeric();
    assert!(text.len() < 32 * 1024);
    let s = trace.state.lock().unwrap();
    assert_eq!(s.first_errors.len(), 1);
    assert_eq!(s.first_errors[0].stage, Stage::Failed);
    assert!(s.lost > 0);
    assert!(
        text.lines()
            .all(|l| l.split_whitespace().all(|n| n.parse::<u64>().is_ok()))
    );
}

#[test]
fn coordinator_is_responsive_while_owning_stalled_region_and_session_lock() {
    use horizon_threads::retirement::{Coordinator, Status};
    let dir = temporary();
    let lock = SessionLock::acquire(&dir).unwrap();
    let (save, gate) = blocked(dir.clone());
    save.save_entities_later(vec![(
        (0, 0),
        vec![Tag::compound([("fixture", Tag::Int(91))])],
    )])
    .unwrap();
    let coordinator = Coordinator::default();
    let mut owned = Some((save, lock));
    let submitted = Instant::now();
    let ticket = coordinator.submit(move || {
        owned
            .as_ref()
            .unwrap()
            .0
            .flush()
            .map_err(|e| e.to_string())?;
        drop(owned.take());
        Ok(())
    });
    assert!(submitted.elapsed() < Duration::from_millis(100));
    // Concurrent/repeated quit observers share this exact retirement, rather
    // than taking a second ownership path or authorizing completion early.
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let ticket = ticket.clone();
            scope.spawn(move || {
                for _ in 0..1000 {
                    assert_eq!(ticket.status(), Status::Pending);
                }
            });
        }
    });
    for _ in 0..100 {
        coordinator.poll();
        assert_eq!(ticket.status(), Status::Pending);
        assert!(SessionLock::acquire(&dir).is_err());
    }
    release(&gate);
    let until = Instant::now() + Duration::from_secs(3);
    while ticket.status() == Status::Pending && Instant::now() < until {
        std::thread::yield_now();
    }
    assert_eq!(ticket.status(), Status::Complete);
    let lock = SessionLock::acquire(&dir).unwrap();
    let reopened = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    assert_eq!(reopened.load_entities(0, 0).len(), 1);
    drop(reopened);
    drop(lock);
    coordinator.poll();
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_retirement_never_releases_failed_writes_or_their_lock() {
    use horizon_threads::retirement::{Coordinator, Status};
    let dir = temporary();
    let lock = SessionLock::acquire(&dir).unwrap();
    let save = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    // Journal admission succeeds, destination storage fails.
    fs::write(dir.join("entities"), b"not a directory").unwrap();
    save.save_entities_later(vec![(
        (0, 0),
        vec![Tag::compound([("fixture", Tag::Int(8))])],
    )])
    .unwrap();
    let coordinator = Coordinator::default();
    let mut owned = Some((save, lock));
    let ticket = coordinator.submit(move || {
        owned
            .as_ref()
            .unwrap()
            .0
            .flush()
            .map_err(|e| e.to_string())?;
        drop(owned.take());
        Ok(())
    });
    let until = Instant::now() + Duration::from_secs(3);
    while ticket.status() == Status::Pending && Instant::now() < until {
        std::thread::yield_now();
    }
    assert_eq!(ticket.status(), Status::Failed);
    assert_eq!(coordinator.exit_status(), Status::Failed);
    assert!(ticket.error().is_some());
    for _ in 0..20 {
        coordinator.poll();
        assert!(SessionLock::acquire(&dir).is_err());
    }
    // This LOCAL test coordinator is explicitly retired off the probe thread.
    // Production's registry has process lifetime and retains failures.
    let until = Instant::now() + Duration::from_secs(3);
    while !coordinator.workers_stopped() && Instant::now() < until {
        std::thread::yield_now();
    }
    assert!(coordinator.workers_stopped());
    drop(ticket);
    std::thread::spawn(move || drop(coordinator))
        .join()
        .unwrap();
    fs::remove_file(dir.join("entities")).unwrap();
    let lock = SessionLock::acquire(&dir).unwrap();
    let reopened = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    assert_eq!(reopened.load_entities(0, 0).len(), 1);
    drop(reopened);
    drop(lock);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
#[ignore = "subprocess fixture invoked explicitly by the abrupt-kill parent test"]
fn abrupt_exit_child_fixture() {
    let Some(dir) = std::env::var_os("HORIZON_SHUTDOWN_CHILD_FIXTURE") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let _lock = SessionLock::acquire(&dir).unwrap();
    let (save, _gate) = blocked(dir.clone());
    save.save_entities_later(vec![(
        (0, 0),
        vec![Tag::compound([("fixture", Tag::Int(1))])],
    )])
    .unwrap();
    assert!(!save.try_write_status().unwrap().finished());
    let _player = crate::save::prepare_dat(
        dir.join("players/data/fixture.dat"),
        Tag::compound([("fixture", Tag::Int(4))]),
    )
    .unwrap();
    let _metadata = crate::save::prepare_dat(
        dir.join("level.dat"),
        Tag::compound([("Data", Tag::compound([("fixture", Tag::Int(5))]))]),
    )
    .unwrap();
    std::fs::write(dir.join("queued-not-durable"), b"synthetic child fixture").unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn host_style_abrupt_kill_recovers_acknowledged_handoff_before_world_reads() {
    // Temporary fixture only; the region writer is blocked at process death.
    let dir = temporary();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "save::shutdown::tests::abrupt_exit_child_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("HORIZON_SHUTDOWN_CHILD_FIXTURE", &dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while !dir.join("queued-not-durable").exists() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(5));
    }
    if !dir.join("queued-not-durable").exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("child fixture did not queue its write");
    }
    assert!(SessionLock::acquire(&dir).is_err());
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    let lock = SessionLock::acquire(&dir).unwrap();
    assert_eq!(
        crate::save::read_dat(&dir.join("players/data/fixture.dat"))
            .unwrap()
            .get("fixture"),
        Some(&Tag::Int(4))
    );
    assert_eq!(
        crate::save::read_dat(&dir.join("level.dat"))
            .unwrap()
            .get("Data")
            .unwrap()
            .get("fixture"),
        Some(&Tag::Int(5))
    );
    let reopened = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    assert_eq!(
        reopened.load_entities(0, 0),
        vec![Tag::compound([("fixture", Tag::Int(1))])]
    );
    drop(reopened);
    drop(lock);
    let lock = SessionLock::acquire(&dir).unwrap();
    let reopened = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    assert_eq!(
        reopened.load_entities(0, 0).len(),
        1,
        "replay must be idempotent"
    );
    drop(reopened);
    drop(lock);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "subprocess fixture explicitly invoked by fatal_runtime_exit_exposure_remains_a_release_blocker"]
fn runtime_exit_child_fixture() {
    let Some(root) = std::env::var_os("HORIZON_RUNTIME_EXIT_CHILD_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("horizon-shutdown-fixture-")
    );
    let lock = SessionLock::acquire(&root).unwrap();
    let save = SaveDir::new(root.clone(), LevelShape::OVERWORLD);
    let dirty = vec![Tag::compound([("fixture", Tag::Int(123))])];
    let (_release, blocked) = std::sync::mpsc::channel::<()>();
    let mut owner = Some((save, lock, dirty));
    let ticket = horizon_threads::retirement::global().submit(move || {
        blocked.recv().map_err(|e| e.to_string())?;
        let (save, _, dirty) = owner.as_ref().unwrap();
        save.save_entities_later(vec![((0, 0), dirty.clone())])
            .map_err(|e| e.to_string())?;
        save.flush().map_err(|e| e.to_string())?;
        drop(owner.take());
        Ok(())
    });
    assert_eq!(
        ticket.status(),
        horizon_threads::retirement::Status::Pending
    );
    fs::write(
        root.join("registered-pending-not-admitted"),
        b"synthetic pending owner; NOT a recoverable handoff",
    )
    .unwrap();
    // Exact unsafe process-lifetime boundary still reachable after fatal
    // event-loop return: process::exit does not wait for registered owners.
    std::process::exit(1);
}

#[test]
fn fatal_runtime_exit_exposure_remains_a_release_blocker() {
    let root = temporary();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "save::shutdown::tests::runtime_exit_child_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("HORIZON_RUNTIME_EXIT_CHILD_ROOT", &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
    assert!(root.join("registered-pending-not-admitted").exists());
    let lock = SessionLock::acquire(&root).unwrap();
    let save = SaveDir::new(root.clone(), LevelShape::OVERWORLD);
    assert!(
        save.load_entities(0, 0).is_empty(),
        "This exposure test documents missing unadmitted data, NOT a passing release safety gate"
    );
    drop(save);
    drop(lock);
    fs::remove_dir_all(root).unwrap();
}
