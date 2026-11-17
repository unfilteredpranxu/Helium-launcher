use super::*;
#[test]
fn pressure_aggregation_is_persisted_with_first_cause_and_distinct_transitions() {
    let path = temp();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    r.begin_world(1, 1);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Game,
        Some(Cause::MaterialChanged),
        47,
        48,
    );
    for _ in 0..300 {
        for (id, emitter) in [(1, Emitter::Game), (0, Emitter::Swift)] {
            r.record(
                id,
                Stage::Cancelled,
                emitter,
                Some(Cause::MemoryPressure),
                0,
                0,
            );
        }
    }
    r.record(
        1,
        Stage::Removed,
        Emitter::Swift,
        Some(Cause::ViewDetached),
        1,
        0,
    );
    assert!(r.wait_ack(Duration::from_secs(5)));
    let text = fs::read_to_string(path.join("Helium-Metal-Report.txt")).unwrap();
    assert!(text.contains("notices=600"));
    assert!(text.contains("ViewDetached"));
    assert!(text.contains("MaterialChanged"));
    assert!(text.contains("chronology_complete=true"));
    assert!(text.contains("PHYSICAL_SCANOUT_UNVERIFIED"));
    assert_eq!(r.first(1), Some(Cause::MaterialChanged));
}
#[test]
fn alternating_memory_emitters_do_not_evict_attempt_history() {
    let r = Recorder::memory();
    assert!(r.begin_world(1, 7));
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Game,
        Some(Cause::LightmapChanged),
        32,
        33,
    );
    for n in 0..1000 {
        r.record(
            1,
            Stage::Cancelled,
            if n % 2 == 0 {
                Emitter::Game
            } else {
                Emitter::Swift
            },
            Some(Cause::MemoryPressure),
            0,
            0,
        );
    }
    assert_eq!(r.first(1), Some(Cause::LightmapChanged));
    assert!(!r.retry_safe(1));
    assert_eq!(
        r.status()[7],
        0,
        "duplicate pressure notices must not rotate history"
    );
    let s = r.state.lock().unwrap();
    assert_eq!(s.pressure.len(), 1);
    assert_eq!(s.pressure[0].count, 1000);
    assert_eq!(s.pressure[0].seen.len(), 2);
    assert!(s.pressure[0].last_ns >= s.pressure[0].first_ns);
}
fn temp() -> PathBuf {
    std::env::temp_dir().join(format!(
        "helium-recorder-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}
#[test]
fn first_cause_priority_survives_saturation_and_ack() {
    let path = temp();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    assert!(r.begin_world(1, 12));
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::ViewLayoutInvalid),
        2360,
        0,
    );
    for n in 0..3000 {
        r.record(1, Stage::UploadStarted, Emitter::Worker, None, n, 8);
    }
    assert!(r.wait_ack(Duration::from_secs(5)));
    let text = fs::read_to_string(path.join("Helium-Metal-Report.txt")).unwrap();
    assert!(text.contains("ViewLayoutInvalid"));
    assert!(r.status()[7] > 0);
    assert_eq!(r.status()[8], 0);
    assert!(
        fs::metadata(path.join("helium-metal-events.jsonl"))
            .unwrap()
            .len()
            <= 512 * 1024
    );
    assert!(text.len() <= 256 * 1024);
}
#[test]
fn concurrent_cancel_latch_and_stale_callback_separation() {
    let r = Arc::new(Recorder::memory());
    r.begin_world(1, 5);
    r.begin_world(2, 5);
    std::thread::scope(|s| {
        for c in [Cause::MaterialChanged, Cause::ViewLayoutInvalid] {
            let r = r.clone();
            s.spawn(move || r.record(1, Stage::Cancelled, Emitter::Swift, Some(c), 10, 11));
        }
    });
    r.record(
        2,
        Stage::Cancelled,
        Emitter::Game,
        Some(Cause::LightmapChanged),
        11,
        12,
    );
    assert_eq!(
        r.snapshot()
            .iter()
            .filter(|e| e.attempt == 1 && e.first)
            .count(),
        1
    );
    assert_eq!(r.first(2), Some(Cause::LightmapChanged));
    let before = r.first(1);
    assert!(!r.begin_world(1, 99));
    assert_eq!(before, r.first(1));
    let events = r.snapshot();
    assert!(events.windows(2).all(|e| e[0].seq < e[1].seq));
}
#[test]
fn attempt_cap_and_explicit_ids() {
    let r = Recorder::memory();
    for id in 1..=3 {
        assert!(r.begin_world(id, 7));
    }
    assert!(!r.begin_world(4, 7));
    r.record(
        99,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::Panic),
        0,
        0,
    );
    assert!(r.snapshot().is_empty());
}
#[test]
fn disk_failure_never_acknowledges_then_recovers() {
    let path = temp();
    fs::write(&path, b"not a directory").unwrap();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    r.begin(1);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Game,
        Some(Cause::MeshReplaced),
        0,
        0,
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(r.status()[8], 1);
    assert_eq!(r.status()[6], 0);
    assert_eq!(r.first(1), Some(Cause::MeshReplaced));
    fs::remove_file(&path).unwrap();
    assert!(r.wait_ack(Duration::from_secs(5)));
    assert!(
        fs::read_to_string(path.join("Helium-Metal-Report.txt"))
            .unwrap()
            .contains("MeshReplaced")
    );
}
#[test]
fn rename_failure_and_recovery_are_explicit() {
    let path = temp();
    fs::create_dir_all(path.join("Helium-Metal-Report.txt")).unwrap();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    r.begin(1);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Game,
        Some(Cause::ColumnUnloaded),
        0,
        0,
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(r.status()[8], 1);
    assert_eq!(r.status()[6], 0);
    fs::remove_dir(path.join("Helium-Metal-Report.txt")).unwrap();
    assert!(r.wait_ack(Duration::from_secs(5)));
}
#[test]
fn previous_boot_and_torn_tail_are_preserved_without_private_text() {
    let path = temp();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    r.begin(1);
    r.record(1, Stage::UploadStarted, Emitter::Worker, None, 1, 3);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Game,
        Some(Cause::LightmapChanged),
        11,
        12,
    );
    assert!(r.wait_ack(Duration::from_secs(5)));
    drop(r);
    std::thread::sleep(Duration::from_millis(300));
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(path.join("helium-metal-events.jsonl"))
        .unwrap()
        .write_all(b"token=SECRET /private/owner 10.0.0.1 PIN999999")
        .unwrap();
    let next = Recorder::persistent_kind(path.clone(), true).unwrap();
    next.record(0, Stage::ModeSelected, Emitter::Game, None, 0, 2);
    assert!(next.wait_ack(Duration::from_secs(5)));
    let text = fs::read_to_string(path.join("Helium-Metal-Report.txt")).unwrap();
    assert!(text.contains("PREVIOUS_BOOT_INCOMPLETE=true"));
    assert!(text.contains("LightmapChanged"));
    assert!(!text.contains("SECRET"));
    assert!(next.status()[14] > 0);
}
#[test]
fn allowlist_roundtrip_and_invalid_records() {
    let r = Recorder::memory();
    r.begin(8);
    r.record(
        8,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::WindowNotKey),
        1,
        0,
    );
    let e = r.snapshot()[0].clone();
    let line = e.json();
    let parsed = Event::parse(line.trim_end()).unwrap();
    assert_eq!(parsed.cause, e.cause);
    assert_eq!(parsed.attempt, 8);
    assert!(Event::parse("account=PRIVATE").is_none());
    assert!(Stage::from_code(u32::MAX).is_none());
    assert_eq!(Cause::reason("/private/token=SECRET"), Cause::Unknown);
}
#[test]
fn retries_require_current_fresh_input_and_retirement() {
    for c in [
        Cause::MaterialChanged,
        Cause::LightmapChanged,
        Cause::MeshReplaced,
    ] {
        assert!(retry_allowed(
            c,
            1,
            true,
            true,
            true,
            Duration::from_secs(5)
        ));
        assert!(!retry_allowed(
            c,
            3,
            true,
            true,
            true,
            Duration::from_secs(5)
        ));
        assert!(!retry_allowed(
            c,
            1,
            false,
            true,
            true,
            Duration::from_secs(5)
        ));
        assert!(!retry_allowed(
            c,
            1,
            true,
            true,
            false,
            Duration::from_secs(5)
        ));
    }
    for c in [
        Cause::MemoryPressure,
        Cause::AppInactive,
        Cause::WorldChanged,
        Cause::DeviceFailure,
    ] {
        assert!(!retry_allowed(
            c,
            1,
            true,
            true,
            true,
            Duration::from_secs(5)
        ));
    }
}

#[test]
fn historical_events_do_not_mutate_current_hud_and_stop_causes_survive_rotation() {
    let r = Recorder::memory();
    r.begin_world(1, 10);
    r.begin_world(2, 11);
    r.record(2, Stage::CameraReady, Emitter::Game, None, 50, 60);
    r.record(1, Stage::CameraReady, Emitter::Game, None, 1, 2);
    r.record(1, Stage::UiRevealed, Emitter::Swift, None, 0, 0);
    assert_eq!(r.status()[18], 50);
    assert_eq!(r.status()[19], 60);
    assert_eq!(r.status()[24], 0);
    r.record(
        2,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::AppInactive),
        0,
        0,
    );
    for n in 0..600 {
        r.record(2, Stage::UploadStarted, Emitter::Worker, None, n, 8);
    }
    assert!(!r.retry_safe(2));
}
#[test]
fn cleanup_and_no_fresh_mesh_cannot_invent_failure_after_reveal() {
    let r = Recorder::memory();
    r.begin(1);
    r.record(1, Stage::UiRevealed, Emitter::Swift, None, 0, 0);
    r.record(
        1,
        Stage::Removed,
        Emitter::Swift,
        Some(Cause::SnapshotExpired),
        0,
        0,
    );
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Ticket,
        Some(Cause::TicketCancelled),
        0,
        0,
    );
    r.record(
        1,
        Stage::RetryWaiting,
        Emitter::Game,
        Some(Cause::NoFreshMesh),
        0,
        0,
    );
    assert_eq!(r.first(1), None);
    assert_eq!(r.status()[24], 1);
}

#[cfg(target_os = "linux")]
#[test]
fn actual_disk_full_is_degraded_without_false_acknowledgement() {
    let path = temp();
    fs::create_dir_all(&path).unwrap();
    std::os::unix::fs::symlink("/dev/full", path.join("helium-metal-events.jsonl.pending"))
        .unwrap();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    r.begin(1);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::ViewDetached),
        0,
        0,
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(r.status()[8], 1);
    assert_eq!(r.status()[6], 0);
    assert!(r.status()[27] > 0);
    assert_eq!(r.first(1), Some(Cause::ViewDetached));
    fs::remove_file(path.join("helium-metal-events.jsonl.pending")).unwrap();
    assert!(r.wait_ack(Duration::from_secs(5)));
    assert!(
        fs::read_to_string(path.join("Helium-Metal-Report.txt"))
            .unwrap()
            .contains("write_errors_total=")
    );
}

#[test]
fn possible_inflight_state_only_changes_at_submission_terminal_or_retirement() {
    let r = Recorder::memory();
    r.begin(1);
    r.record(1, Stage::UploadStarted, Emitter::Worker, None, 0, 1);
    assert_eq!(r.status()[28], 0);
    r.record(1, Stage::UploadSubmitted, Emitter::Worker, None, 0, 4);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::ViewDetached),
        0,
        0,
    );
    assert!(r.snapshot().last().unwrap().gpu_inflight);
    assert_eq!(r.status()[28], 1);
    r.record(1, Stage::Retired, Emitter::Game, None, 0, 0);
    assert_eq!(r.status()[28], 0);
    r.begin(2);
    r.record(2, Stage::UiRevealed, Emitter::Swift, None, 0, 0);
    r.record(
        2,
        Stage::Removed,
        Emitter::Swift,
        Some(Cause::SnapshotExpired),
        0,
        0,
    );
    for n in 0..600 {
        r.record(0, Stage::SelfTest, Emitter::Recorder, None, n, 0);
    }
    r.record(
        2,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::TicketCancelled),
        0,
        0,
    );
    assert_eq!(r.first(2), None);
}

#[test]
fn retired_attempt_is_not_misclassified_by_a_later_retry_note_on_relaunch() {
    let path = temp();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    r.begin(1);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::LightmapChanged),
        11,
        12,
    );
    r.record(1, Stage::Retired, Emitter::Game, None, 0, 0);
    r.record(
        1,
        Stage::RetryWaiting,
        Emitter::Game,
        Some(Cause::NoFreshMesh),
        0,
        0,
    );
    assert!(r.wait_ack(Duration::from_secs(5)));
    drop(r);
    std::thread::sleep(Duration::from_millis(250));
    let next = Recorder::persistent_kind(path, true).unwrap();
    assert!(next.wait_ack(Duration::from_secs(5)));
    assert_eq!(next.status()[13], 0);
}

#[test]
fn repeated_boots_without_a_new_attempt_keep_the_last_first_cause() {
    let path = temp();
    let r = Recorder::persistent_kind(path.clone(), true).unwrap();
    r.begin(1);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::ViewLayoutInvalid),
        2360,
        0,
    );
    assert!(r.wait_ack(Duration::from_secs(5)));
    drop(r);
    std::thread::sleep(Duration::from_millis(250));
    for _ in 0..3 {
        let r = Recorder::persistent_kind(path.clone(), true).unwrap();
        assert!(r.wait_ack(Duration::from_secs(5)));
        assert!(
            fs::read_to_string(path.join("Helium-Metal-Report.txt"))
                .unwrap()
                .contains("ViewLayoutInvalid")
        );
        drop(r);
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[test]
fn capture_age_rejection_is_a_cause_but_normal_reveal_timer_removal_is_not() {
    let r = Recorder::memory();
    r.begin_world(1, 8);
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::SnapshotExpired),
        0,
        0,
    );
    r.record(
        1,
        Stage::Cancelled,
        Emitter::Swift,
        Some(Cause::TicketCancelled),
        0,
        0,
    );
    assert_eq!(r.first(1), Some(Cause::SnapshotExpired));
    r.begin_world(2, 8);
    r.record(
        2,
        Stage::Removed,
        Emitter::Swift,
        Some(Cause::SnapshotExpired),
        0,
        0,
    );
    assert_eq!(r.first(2), None);
}
