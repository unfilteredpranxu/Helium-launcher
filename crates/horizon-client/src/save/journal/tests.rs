use super::*;
fn temporary() -> PathBuf {
    std::env::temp_dir().join(format!(
        "horizon-redo-fixture-{}-{}",
        std::process::id(),
        shutdown::new_storage_id()
    ))
}
#[test]
fn published_file_replays_before_reads_and_reopening_is_idempotent() {
    let root = temporary();
    fs::create_dir_all(&root).unwrap();
    let path = root.join("level.dat");
    fs::write(&path, b"old exact bytes").unwrap();
    let _ticket = file(&path, b"new exact bytes").unwrap();
    let lock = SessionLock::acquire(&root).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new exact bytes");
    drop(lock);
    let _lock = SessionLock::acquire(&root).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new exact bytes");
    drop(_lock);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn older_completion_cannot_delete_a_newer_published_handoff() {
    let root = temporary();
    fs::create_dir_all(&root).unwrap();
    let path = root.join("player.dat");
    let old = file(&path, b"old").unwrap();
    let _new = file(&path, b"new").unwrap();
    acknowledge(old).unwrap();
    let _lock = SessionLock::acquire(&root).unwrap();
    assert_eq!(fs::read(path).unwrap(), b"new");
    drop(_lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deleting_an_absent_record_retires_its_exact_redo() {
    let root = temporary();
    let lock = SessionLock::acquire(&root).unwrap();
    let save = SaveDir::new(root.clone(), crate::chunk::LevelShape::OVERWORLD);
    save.save_entities_later(vec![((0, 0), vec![])]).unwrap();
    save.flush().unwrap();
    assert_eq!(
        fs::read_dir(root.join(".helium-redo-v3")).unwrap().count(),
        0
    );
    drop(save);
    drop(lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn denied_region_deletion_is_failure_and_stays_recoverable() {
    use std::os::unix::fs::PermissionsExt;
    let root = temporary();
    let lock = SessionLock::acquire(&root).unwrap();
    let save = SaveDir::new(root.clone(), crate::chunk::LevelShape::OVERWORLD);
    save.save_entities_later(vec![(
        (0, 0),
        vec![Tag::compound([("fixture", Tag::Int(1))])],
    )])
    .unwrap();
    save.flush().unwrap();
    drop(save);
    let region = root.join("entities/r.0.0.mca");
    fs::set_permissions(&region, fs::Permissions::from_mode(0)).unwrap();
    let save = SaveDir::new(root.clone(), crate::chunk::LevelShape::OVERWORLD);
    save.save_entities_later(vec![((0, 0), vec![])]).unwrap();
    let result = save.flush();
    // Restore fixture permissions even if an assertion fails.
    fs::set_permissions(&region, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        result.is_err(),
        "permission failure must not masquerade as an absent region"
    );
    assert!(!save.try_write_status().unwrap().finished());
    drop(save);
    drop(lock);
    let lock = SessionLock::acquire(&root).unwrap();
    let save = SaveDir::new(root.clone(), crate::chunk::LevelShape::OVERWORLD);
    assert!(save.load_entities(0, 0).is_empty());
    drop(save);
    drop(lock);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn incomplete_publication_is_not_admitted_and_corrupt_committed_redo_refuses_open() {
    let root = temporary();
    fs::create_dir_all(&root).unwrap();
    let path = root.join("level.dat");
    fs::write(&path, b"old").unwrap();
    fs::write(root.join("level.dat.helium-redo-building"), b"torn").unwrap();
    drop(SessionLock::acquire(&root).unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"old");
    let ticket = file(&path, b"recover me").unwrap();
    fs::write(&ticket.path, b"HLJ3truncated").unwrap();
    assert!(SessionLock::acquire(&root).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"old");
    assert!(ticket.path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_player_handoff_survives_loss_of_its_memory_owner() {
    let root = temporary();
    let path = root.join("players/data/fixture.dat");
    let handoff = prepare_dat(path.clone(), Tag::compound([("fixture", Tag::Int(41))])).unwrap();
    drop(handoff); // No destination writer ever ran.
    let lock = SessionLock::acquire(&root).unwrap();
    assert_eq!(read_dat(&path).unwrap().get("fixture"), Some(&Tag::Int(41)));
    drop(lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_player_destination_keeps_recoverable_handoff_and_old_file() {
    let root = temporary();
    let path = root.join("players/data/fixture.dat");
    write_dat_durable(&path, Tag::compound([("fixture", Tag::Int(1))])).unwrap();
    let handoff = prepare_dat(path.clone(), Tag::compound([("fixture", Tag::Int(2))])).unwrap();
    let blocked = path.with_file_name("fixture.dat_new");
    fs::create_dir(&blocked).unwrap();
    assert!(handoff.commit().is_err());
    assert_eq!(read_dat(&path).unwrap().get("fixture"), Some(&Tag::Int(1)));
    fs::remove_dir(blocked).unwrap();
    let lock = SessionLock::acquire(&root).unwrap();
    assert_eq!(read_dat(&path).unwrap().get("fixture"), Some(&Tag::Int(2)));
    drop(lock);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn superseded_player_completion_cannot_overwrite_newer_bytes_or_claim_destination_commit() {
    let root = temporary();
    let path = root.join("players/data/fixture.dat");
    let old = prepare_dat(path.clone(), Tag::compound([("fixture", Tag::Int(1))])).unwrap();
    let new = prepare_dat(path.clone(), Tag::compound([("fixture", Tag::Int(2))])).unwrap();
    assert!(new.commit().unwrap());
    assert!(!old.commit().unwrap());
    assert_eq!(read_dat(&path).unwrap().get("fixture"), Some(&Tag::Int(2)));
    drop(SessionLock::acquire(&root).unwrap());
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "subprocess fixture explicitly invoked by killed_replay_child_recovers_again"]
fn replay_child() {
    let Some(root) = std::env::var_os("HORIZON_REPLAY_CHILD_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    assert!(
        root.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("horizon-redo-fixture-")
    );
    let _lock = SessionLock::acquire(&root).unwrap();
    panic!("replay fault-injection fixture was not reached");
}
#[test]
fn killed_replay_child_recovers_again_without_losing_or_duplicating_commit() {
    let root = temporary();
    let path = root.join("players/data/fixture.dat");
    drop(prepare_dat(path.clone(), Tag::compound([("fixture", Tag::Int(99))])).unwrap());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "save::journal::tests::replay_child",
            "--ignored",
            "--nocapture",
        ])
        .env("HORIZON_REPLAY_CHILD_ROOT", &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let marker = root.join("replay-synced-before-ack");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !marker.exists() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if !marker.exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("replay child did not reach durable replacement");
    }
    assert!(SessionLock::acquire(&root).is_err());
    child.kill().unwrap();
    child.wait().unwrap();
    let lock = SessionLock::acquire(&root).unwrap();
    assert_eq!(read_dat(&path).unwrap().get("fixture"), Some(&Tag::Int(99)));
    drop(lock);
    drop(SessionLock::acquire(&root).unwrap());
    assert_eq!(read_dat(&path).unwrap().get("fixture"), Some(&Tag::Int(99)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_after_destination_replace_before_ack_is_idempotent_and_preserves_other_region_records() {
    let root = temporary();
    let save = SaveDir::new(root.clone(), crate::chunk::LevelShape::OVERWORLD);
    let other = Tag::compound([("fixture", Tag::Int(10))]);
    save.save_entities_later(vec![((1, 0), vec![other.clone()])])
        .unwrap();
    save.flush().unwrap();
    let tag = NamedTag {
        name: "".into(),
        tag: Tag::compound([("fixture", Tag::Int(21))]),
    };
    let record = Record::Tag(Arc::new(tag));
    let _ticket = region(&root, Folder::Region, (0, 0), &record).unwrap();
    let cache = RegionCache::default();
    write_region_records(&root, &cache, "region", &[((0, 0), &record)]).unwrap();
    // Simulate interruption AFTER destination sync but BEFORE log cleanup.
    drop(save);
    let lock = SessionLock::acquire(&root).unwrap();
    let reopened = SaveDir::new(root.clone(), crate::chunk::LevelShape::OVERWORLD);
    assert_eq!(reopened.load_entities(1, 0), vec![other]);
    let saved = read_region_record(&root, &cache, "region", 0, 0)
        .unwrap()
        .unwrap();
    assert_eq!(saved.tag.get("fixture"), Some(&Tag::Int(21)));
    drop(reopened);
    drop(lock);
    drop(SessionLock::acquire(&root).unwrap());
    fs::remove_dir_all(root).unwrap();
}
