use super::{
    Audio, Game, Options, Progress, Screen, Settings, execution_role::ExecutionRole,
    load_resources, tutorial,
};
use horizon_client::{hud::HudViewport, items::ItemStack, menu::Input, motion::PlayerMotion};
use horizon_core::geometry::Vec3;
use std::{path::PathBuf, time::Instant};
use winit::keyboard::KeyCode;

#[test]
#[ignore = "needs prepared resources in HORIZON_TEST_RESOURCES"]
fn production_ui_uses_live_vitals_and_inventory_with_no_renderer() {
    let root = PathBuf::from(
        std::env::var_os("HORIZON_TEST_RESOURCES")
            .expect("set HORIZON_TEST_RESOURCES to prepared resources"),
    );
    let settings = Settings::default();
    let loaded = load_resources(
        root.clone(),
        None,
        &root,
        &settings,
        Instant::now(),
        false,
        false,
        &Progress::new(),
        &mut |_, _| unreachable!("no menu handoff"),
    )
    .unwrap();
    let mut options = Options::parse(std::iter::empty()).unwrap().unwrap();
    options.resources = root.clone();
    options.execution_role = ExecutionRole::Client;
    options.native_gameplay = true;
    let mut game = Game::new(
        &loaded.resources,
        loaded.pack,
        loaded.gui.unwrap(),
        options,
        settings,
        Audio::silent(&root),
        None,
    )
    .unwrap();
    game.player = Some(PlayerMotion::new(game.motion, Vec3::ZERO).unwrap());
    game.title = None;
    game.screen = Screen::Playing;
    game.creative = false;
    game.tutorial = tutorial::Tutorial::new(tutorial::Step::None);
    assert!(game.renderer.is_none());
    assert!(game.touch_playing());

    let viewport = HudViewport::new([2360, 1640], [24, 0, 24, 20]);
    let full = game.hud(viewport.size());
    assert!(!full.vertices.is_empty());
    let player = game.player.as_mut().unwrap();
    player.set_health(7.0, false).unwrap();
    player.health.food.food = 6;
    let hurt = game.hud(viewport.size());
    assert_ne!(
        bytemuck::cast_slice::<_, u8>(&full.vertices),
        bytemuck::cast_slice::<_, u8>(&hurt.vertices),
    );

    game.inventory[0] = Some(ItemStack {
        item: "minecraft:stone".into(),
        count: 13,
        damage: 0,
        extra: None,
    });
    game.key_pressed(KeyCode::KeyE, KeyCode::KeyE).unwrap();
    assert!(game.screen == Screen::Inventory);
    assert!(!game.touch_playing());
    let batch = game.hud(viewport.size());
    for slot in [0, 1] {
        let (ox, oy) = game.inventory_origin(&batch);
        let (x, y) = game.slot_position(slot);
        let (px, py) = viewport.from_gui((ox + x + 8.0, oy + y + 8.0), batch.scale);
        let (gx, gy) = viewport.to_gui([px, py], batch.scale);
        let target = game.slot_at(&batch, gx, gy);
        assert_eq!(target, Some(slot));
        assert!(game.menu_click(target, 0, Input::Pickup));
    }
    assert!(game.inventory[0].is_none());
    assert_eq!(game.inventory[1].as_ref().unwrap().count, 13);
    assert!(game.carried.is_none());
    game.key_pressed(KeyCode::KeyE, KeyCode::KeyE).unwrap();
    assert!(game.touch_playing());
    game.key_pressed(KeyCode::Escape, KeyCode::Escape).unwrap();
    assert!(game.screen == Screen::Paused);
    assert!(!game.touch_playing());
    game.key_pressed(KeyCode::Escape, KeyCode::Escape).unwrap();
    assert!(game.touch_playing());
    game.key_pressed(KeyCode::KeyE, KeyCode::KeyE).unwrap();
    assert_eq!(game.inventory[1].as_ref().unwrap().count, 13);
    game.key_pressed(KeyCode::KeyE, KeyCode::KeyE).unwrap();
    assert!(game.touch_playing());
}
