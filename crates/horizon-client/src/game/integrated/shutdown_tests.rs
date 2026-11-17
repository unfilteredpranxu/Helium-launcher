//! Synthetic temporary storage and real production Integrated ownership;
//! no Minecraft assets, UIKit or owner-device runtime claim.
use super::*;
use horizon_client::save::{LevelShape, SaveDir, SessionLock};
use horizon_world::nbt::Tag;

fn blocked_server() -> (Integrated, Sender<()>, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "horizon-integrated-exit-fixture-{}-{}",
        std::process::id(),
        horizon_client::save::shutdown::new_storage_id()
    ));
    let lock = SessionLock::acquire(&root).unwrap();
    let save = SaveDir::new(root.clone(), LevelShape::OVERWORLD);
    save.save_entities_later(vec![(
        (0, 0),
        vec![Tag::compound([("fixture", Tag::Int(17))])],
    )])
    .unwrap();
    let player = horizon_client::save::prepare_dat(
        root.join("players/data/fixture.dat"),
        Tag::compound([("fixture", Tag::Int(18))]),
    )
    .unwrap();
    let flags = Arc::new(Flags::default());
    let worker_flags = flags.clone();
    let (release, blocked) = channel();
    let thread = std::thread::spawn(move || {
        blocked.recv().unwrap();
        save.flush().unwrap();
        assert!(player.commit().unwrap());
        drop(save);
        *worker_flags.retired_lock.lock().unwrap() = Some(lock);
    });
    let (tasks, _receive) = channel();
    (
        Integrated {
            flags,
            thread: Some(thread),
            tasks,
            world_name: String::new(),
        },
        release,
        root,
    )
}

#[test]
fn owned_shutdown_drop_returns_without_join_and_real_handoffs_remain_owned() {
    let (server, release, root) = blocked_server();
    let start = Instant::now();
    drop(server); // The actual early-error/Starting destructor path.
    assert!(start.elapsed() < Duration::from_secs(1));
    for _ in 0..100 {
        horizon_threads::retirement::global().poll();
        assert!(SessionLock::acquire(&root).is_err());
    }
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while horizon_threads::retirement::global().pending() && Instant::now() < until {
        std::thread::yield_now();
    }
    assert!(!horizon_threads::retirement::global().pending());
    let lock = SessionLock::acquire(&root).unwrap();
    let save = SaveDir::new(root.clone(), LevelShape::OVERWORLD);
    assert_eq!(save.load_entities(0, 0).len(), 1);
    assert_eq!(
        horizon_client::save::read_dat(&root.join("players/data/fixture.dat"))
            .unwrap()
            .get("fixture"),
        Some(&Tag::Int(18))
    );
    drop(save);
    drop(lock);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn synchronous_halt_demonstrates_why_it_must_be_worker_only() {
    let (mut server, release, root) = blocked_server();
    let (done, receive) = channel();
    let probe = std::thread::spawn(move || {
        let result = server.halt().map_err(|e| e.to_string());
        drop(server);
        done.send(result).unwrap();
    });
    assert!(receive.recv_timeout(Duration::from_millis(100)).is_err());
    assert!(SessionLock::acquire(&root).is_err());
    release.send(()).unwrap();
    receive
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    probe.join().unwrap();
    drop(SessionLock::acquire(&root).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}
