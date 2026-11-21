use super::*;
use horizon_client::save::{Level, LevelShape, SaveDir, prepare_dat, read_dat};
use horizon_world::nbt::Tag;
use std::os::unix::fs::symlink;

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "helium-disposable-{}-{}",
            std::process::id(),
            horizon_client::save::shutdown::new_storage_id(),
        ));
        fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn standalone_layout_rejects_guest_app_and_nested_container_paths() {
    let id = "01234567-0000-0000-0000-abcdef123456";
    let image = PathBuf::from(format!(
        "/private/var/containers/Bundle/Application/{id}/Helium.app/Horizon"
    ));
    let home = PathBuf::from(format!(
        "/private/var/mobile/Containers/Data/Application/{id}"
    ));
    let guest = home.join("Documents/Applications/dev.helium.g06ctest.app/Horizon");
    assert!(standalone_layout(&image, &home));
    assert!(!standalone_layout(&guest, &home));
    assert!(!standalone_layout(&image, &home.join("Documents/guest")));
    assert!(!standalone_layout(
        &image.with_file_name("LiveContainer"),
        &home
    ));
    assert!(!standalone_layout(
        &image,
        &home.with_file_name("user-home")
    ));
}

#[test]
fn real_level_player_and_regions_reopen_with_replay_only_inside_admitted_slot() {
    let home = Temp::new();
    let namespace = Namespace::open(&home.0).unwrap();
    let ordinary = namespace.documents.join("saves/Ordinary");
    fs::create_dir_all(&ordinary).unwrap();
    fs::write(ordinary.join("level.dat"), b"ordinary sentinel").unwrap();
    assert!(namespace.list().unwrap().is_empty());
    let world = namespace.prepare_world(SLOT, true).unwrap();
    assert_eq!(world, namespace.root.join("saves").join(SLOT));
    assert!(namespace.prepare_world(SLOT, true).is_err());
    assert!(Namespace::open(&home.0).is_err());
    assert!(require_level_data(&world).is_err());

    let lock = SessionLock::acquire(&world).unwrap();
    let mut level = Level {
        name: "Disposable Test".into(),
        seed: 417,
        ..Default::default()
    };
    level.inventory.push((0, "minecraft:stone".into(), 13, 0));
    level.write(&world).unwrap();
    let save = SaveDir::new(world.clone(), LevelShape::OVERWORLD);
    let entity = Tag::compound([("fixture", Tag::Int(27))]);
    save.save_entities_later(vec![((0, 0), vec![entity.clone()])])
        .unwrap();
    save.flush().unwrap();
    let pending = Tag::compound([("fixture", Tag::Int(28))]);
    let receipt = prepare_dat(world.join("data/disposable-replay.dat"), pending.clone()).unwrap();
    assert!(read_dat(&world.join("data/disposable-replay.dat")).is_none());
    assert!(SessionLock::acquire(&world).is_err());
    assert!(namespace.backup(SLOT).is_err());
    drop((receipt, save, lock, namespace));

    let namespace = Namespace::open(&home.0).unwrap();
    let listed = namespace.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].folder, SLOT);
    for _ in 0..2 {
        let reopened = namespace.prepare_world(SLOT, false).unwrap();
        let lock = SessionLock::acquire(&reopened).unwrap();
        let level = Level::read(&reopened).unwrap();
        assert_eq!(level.seed, 417);
        assert_eq!(level.inventory, [(0, "minecraft:stone".into(), 13, 0)]);
        assert_eq!(
            read_dat(&world.join("data/disposable-replay.dat")),
            Some(pending.clone())
        );
        let save = SaveDir::new(reopened, LevelShape::OVERWORLD);
        assert_eq!(save.load_entities(0, 0), [entity.clone()]);
        drop((save, lock));
    }
    assert!(namespace.backup(SLOT).unwrap() > 0);
    assert_eq!(
        fs::read_dir(namespace.root.join("backups"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        fs::read(ordinary.join("level.dat")).unwrap(),
        b"ordinary sentinel"
    );
    assert!(!ordinary.join("session.lock").exists());
}

#[test]
fn incomplete_admitted_slot_rejects_recreation_and_can_replay_its_first_level() {
    let home = Temp::new();
    let namespace = Namespace::open(&home.0).unwrap();
    let world = namespace.prepare_world(SLOT, true).unwrap();
    let lock = SessionLock::acquire(&world).unwrap();
    assert!(namespace.list().is_err());
    assert!(namespace.prepare_world(SLOT, true).is_err());
    let mut level = Level {
        name: "Replay first level".into(),
        ..Default::default()
    };
    let scratch = home.0.join("test-snapshot");
    level.write_without_player(&scratch).unwrap();
    let metadata = read_dat(&scratch.join("level.dat")).unwrap();
    let receipt = prepare_dat(world.join("level.dat"), metadata).unwrap();
    drop((receipt, lock, namespace));
    let reopened = Namespace::open(&home.0).unwrap();
    assert!(!world.join("level.dat").exists());
    assert_eq!(reopened.list().unwrap()[0].name, "Replay first level");
    assert!(world.join("level.dat").exists());
}

#[test]
#[ignore = "child invoked by forced_exit_reopens_the_disposable_slot_and_replays"]
fn disposable_exit_child() {
    let home = PathBuf::from(std::env::var_os("HELIUM_DISPOSABLE_FIXTURE").unwrap());
    assert!(
        home.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("helium-disposable-")
    );
    let namespace = Namespace::open(&home).unwrap();
    let world = namespace.prepare_world(SLOT, true).unwrap();
    let _lock = SessionLock::acquire(&world).unwrap();
    let mut level = Level {
        seed: 281,
        ..Default::default()
    };
    level.write(&world).unwrap();
    let _pending = prepare_dat(
        world.join("data/killed.dat"),
        Tag::compound([("admitted", Tag::Int(73))]),
    )
    .unwrap();
    fs::write(home.join("test-child-admitted"), b"ready").unwrap();
    loop {
        std::thread::park();
    }
}

#[test]
fn forced_exit_reopens_the_disposable_slot_and_replays() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let home = Temp::new();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "game::disposable::tests::disposable_exit_child",
            "--ignored",
        ])
        .env("HELIUM_DISPOSABLE_FIXTURE", &home.0)
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !home.0.join("test-child-admitted").is_file() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "fixture exited before durable admission"
        );
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("fixture never reached durable admission");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let second_namespace = Namespace::open(&home.0);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(second_namespace.is_err());
    let namespace = Namespace::open(&home.0).unwrap();
    let world = namespace.prepare_world(SLOT, false).unwrap();
    let _lock = SessionLock::acquire(&world).unwrap();
    assert_eq!(Level::read(&world).unwrap().seed, 281);
    assert_eq!(
        read_dat(&world.join("data/killed.dat")),
        Some(Tag::compound([("admitted", Tag::Int(73))]))
    );
}

#[test]
fn selectors_symlinks_hardlinks_and_replaced_world_are_rejected() {
    let home = Temp::new();
    let namespace = Namespace::open(&home.0).unwrap();
    for selector in [
        "",
        ".",
        "..",
        "../saves/disposable-world",
        "/disposable-world",
        "disposable-world/",
        "Disposable-World",
        "disposable-world\\other",
        "ordinary",
    ] {
        assert!(
            namespace.prepare_world(selector, true).is_err(),
            "{selector}"
        );
    }
    let ordinary = home.0.join("ordinary-world");
    fs::create_dir(&ordinary).unwrap();
    let sentinel = ordinary.join("level.dat");
    fs::write(&sentinel, b"unmodified").unwrap();
    let slot = namespace.saves().join(SLOT);
    symlink(&ordinary, &slot).unwrap();
    assert!(namespace.prepare_world(SLOT, false).is_err());
    assert!(namespace.prepare_world(SLOT, true).is_err());
    fs::remove_file(&slot).unwrap();
    let world = namespace.prepare_world(SLOT, true).unwrap();
    let nested = world.join("region");
    fs::create_dir(&nested).unwrap();
    symlink(&ordinary, nested.join("external")).unwrap();
    assert!(namespace.prepare_world(SLOT, false).is_err());
    fs::remove_file(nested.join("external")).unwrap();
    fs::hard_link(&sentinel, world.join("level.dat")).unwrap();
    assert!(namespace.list().is_err());
    fs::rename(world, home.0.join("moved")).unwrap();
    fs::rename(&ordinary, &slot).unwrap();
    assert!(namespace.prepare_world(SLOT, false).is_err());
    assert!(namespace.backup(SLOT).is_err());
    assert_eq!(fs::read(slot.join("level.dat")).unwrap(), b"unmodified");
    assert!(!slot.join("session.lock").exists());
}

#[test]
fn preexisting_slot_and_private_root_aliases_fail_closed() {
    let home = Temp::new();
    let namespace = Namespace::open(&home.0).unwrap();
    let forged_world = namespace.saves().join(SLOT);
    fs::create_dir(&forged_world).unwrap();
    fs::write(forged_world.join("level.dat"), b"untouched").unwrap();
    assert!(namespace.prepare_world(SLOT, false).is_err());
    assert!(namespace.list().is_err());
    let root = namespace.root.clone();
    drop(namespace);
    assert!(Namespace::open(&home.0).is_err());
    fs::rename(&root, home.0.join("moved-root")).unwrap();
    symlink(home.0.join("moved-root"), &root).unwrap();
    assert!(Namespace::open(&home.0).is_err());
    assert_eq!(
        fs::read(forged_world.join("level.dat")).unwrap(),
        b"untouched"
    );
}

#[test]
fn document_import_accepts_only_prepared_archive_and_leaves_sources_intact() {
    fn zip(path: &Path, names: &[&str]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for name in names {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"fixture").unwrap();
        }
        zip.finish().unwrap();
    }
    let home = Temp::new();
    let namespace = Namespace::open(&home.0).unwrap();
    let outside = home.0.join("outside.zip");
    zip(&outside, &["horizon-resources.json"]);
    assert!(namespace.import_resources(&outside).is_err());
    let archive = namespace.documents.join("import.zip");
    symlink(&outside, &archive).unwrap();
    assert!(namespace.import_resources(&archive).is_err());
    fs::remove_file(&archive).unwrap();
    fs::hard_link(&outside, &archive).unwrap();
    assert!(namespace.import_resources(&archive).is_err());
    fs::remove_file(&archive).unwrap();
    for names in [
        vec!["level.dat"],
        vec!["horizon-resources.json", "world/level.dat"],
    ] {
        zip(&archive, &names);
        assert!(namespace.import_resources(&archive).is_err());
        assert!(archive.is_file());
        assert!(namespace.list().unwrap().is_empty());
        assert!(!namespace.root.join("resources").exists());
    }
    zip(&archive, &["horizon-resources.json", "facts.json"]);
    let installed = namespace.import_resources(&archive).unwrap();
    assert_eq!(installed.path, namespace.root.join("resources"));
    assert!(archive.is_file());
    assert!(!namespace.documents.join("saves").exists());
    for command in [
        "open",
        "world-backup",
        "world-delete",
        "connect",
        "screenshot",
        "repro",
    ] {
        assert!(!allows_control(command));
    }
}
