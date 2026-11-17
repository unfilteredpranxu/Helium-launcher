use super::*;
use crate::{
    chunk::LevelShape,
    save::{SaveDir, SessionLock},
};
use horizon_world::nbt::Tag;

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
    std::fs::write(dir.join("queued-not-durable"), b"synthetic child fixture").unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn host_style_abrupt_kill_loses_unacknowledged_memory_without_false_completion() {
    // Temporary fixture only. This deliberately proves the remaining exposure,
    // not that the app has a journal/recovery or watchdog prevention mechanism.
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
    assert!(SessionLock::acquire(&dir).is_ok());
    let reopened = SaveDir::new(dir.clone(), LevelShape::OVERWORLD);
    assert!(
        reopened.load_entities(0, 0).is_empty(),
        "no durable journal exists for this queued-only write"
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}
