# Helium Launcher — S2F handoff for Gemini

**PARTIAL.** The isolated iOS game now submits its existing Rust HUD and in-world menus through the direct Metal compositor. Physical inventory, touch, and world save/quit/reopen still require a qualified device session. Full gameplay linkage still reaches wgpu. The gameplay IPA is held.

## S2F host and save qualification

S2F started at `47286bfc7d0d850c451a11bc968522fd7305c4e7`. The opening integrated server can own the world while the UI still shows the title. `Game::begin_closing_save` now selects owned retirement before the idle-title fast path, even while a world is opening. Quit waits for the retirement receipt. Closing a starting integrated world also preserves its singleplayer return route. Scene suspension queues a checkpoint for the opening server when the active handle has yet to be installed. That still uses the single outstanding checkpoint guard and returns to UIKit before any save. Deferred world-open and connect errors now enter the standard owned error-close path. This matters on iOS because winit ignores a programmatic `exit()`.

The tested S2F source is `7fbb0d55333a1d10b7dac962385de31088a2d179`. The later evidence commit only updates this file. [Exact-source CI run 44865468477](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44865468477)

```bash
cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib shutdown_tests -- --test-threads=1
cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::runtime_exit::tests -- --test-threads=1
cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib save:: -- --test-threads=4
cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib lan::playerdata::tests::
cargo +1.95.0 check --locked -p horizon-client --no-default-features --features helium-native-metal-gameplay
python3 -m unittest discover -s tests -p 'test*ios*.py'
```

Local Linux ran nine shutdown tests and two runtime-return tests. One runtime-return subprocess fixture was invoked by its parent test. The new cases verify that a title-screen close remains pending while the opening world owner is held, an idle title creates no retirement work, and a background notification queues exactly one checkpoint while opening. Local Linux save and player tests passed 93 and 26, with six save fixtures ignored in the ordinary invocation. The native feature check and all 19 package guard tests passed.

The [Apple SDK job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44865468477/job/136728031079) passed all 12 explicitly executed Metal regressions. Synthetic terrain completed 120 consecutive frames and four detached presentations, with 0.341 ms median encoding and 1.028 ms median command GPU time. Synthetic HUD encoding took 113.097 ms cold, then 0.790 and 0.358 ms warm. The standalone fixture completed 12 frames and four presentations, retired all GPU work and passed its wgpu-free iOS ARM64 audit. These are Mac fixtures. No A16 performance or scanout has been measured for S2F. The [Swift/C/Rust diagnostics and UIKit job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44865468477/job/136728031097) also passed.

The pinned [Linux shutdown job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44865468477/job/136728031108) and [macOS shutdown job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44865468477/job/136728031284) each passed eight retirement, 93 save, 26 player durability, five integrated shutdown, four save coordination, two fatal-return and 26 streaming tests. The [production bridge job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44865468477/job/136728030920) passed the production-format Metal worker test and compiled the actual game in the ARM64 iOS configuration. Those jobs ran without prepared game resources or a connected iPad.

All seven execution jobs passed on `7fbb0d55333a1d10b7dac962385de31088a2d179`. The actual optimized Rust game, UIKit host and Swift framework linked. The verifier matched the ARM64 Rust executable with the one embedded in `dev.helium.g06ctest` build `12001` and confirmed the iOS 15.0 minimum and native attachment exports. The executable SHA-256 was `a5ba2334112a944cd66a09b3b9cf48922a504d4b1da609bf9df027e38b2da877`. CI verified the temporary `held.ipa` (SHA-256 `a9b9ecaa619cb115633d73398ad2068fb320a58b42bec9be182e6f82d0adcd3c`) and discarded it. The owner packaging and upload steps were skipped. Device boot, native mode activation with a real world, Save & Quit, and scanout remain unverified. [ARM64 release build and package verification](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44865468477/job/136731064480)

The full iOS graph still includes `wgpu 30.0.1` through `horizon-client`, `horizon-client-lib`, `horizon-client-render` and `horizon-hdr`. The bridge reaches it via `horizon-client-render`. Entity and held-item drawing, outlines, clouds and weather geometry, terrain material animation parity, and optional HUD effects remain missing or incomplete. S2F changes the save/lifecycle paths only. The existing native frame queue, GPU resource path and budgets retain their S2E behavior.

The roadmap's original fatal-return bug predates S1. `game::run_with` runs inside `runtime_exit::run`, which waits until both storage ownership and native GPU retirement are terminal before the executable can reach its controlled process exit. A failed save retains ownership and cannot authorize a successful close. On iOS the ordinary runtime stays inside `UIApplicationMain`. The termination callback initiates retirement and returns. UIKit can still end the process or expire a background task while writes are pending. Published, durably admitted redo can replay. Live changes that never reached admission may be lost. The earlier LiveContainer process-exit watchdog report does not establish a GPU fault or identify the joined worker. The lifetime of a guest during unload remains unqualified.

**G0 remains blocked for owner distribution.** The held CI build validates the game target. A separate bundle identifier alone cannot enforce disposable-only access. A candidate must enforce a new dedicated save namespace, reject world imports and pre-existing worlds from outside it, and allow reopen only for the world created by that candidate. Make its standalone package select `native-gameplay` explicitly and fail closed if that mode or its UIKit host is unavailable. Keep the publication hold while qualifying that boundary and standalone lifecycle. LiveContainer needs independent evidence.

### Prepared-resource UI gate

The resource-backed fixture must run with a complete prepared 26.3 directory containing the current `horizon-resources.json`, gameplay tables, and real GUI textures and fonts. The repository's `horizon-prepare --prepare` takes the tester's owned 26.3 client, launcher assets and pinned server JAR. Verify the result with `horizon-prepare --check --output "$assets"`. The cloud checkout had neither a prepared set nor the owned source installation. `horizon-prepare --check` returned 1, and invoking the UI fixture explicitly returned 101 at the missing `HORIZON_TEST_RESOURCES` setup assertion. No gameplay assertions ran. [Source-build instructions](../../README.md#build-from-source)

Once an authorized local prepared tree is available, use this command from the checkout. It runs the real fixture in an isolated working directory and fails if preparation is incomplete.

```bash
repo=$(pwd -P)
assets=/absolute/path/to/prepared/26.3
profile=$(mktemp -d)
cd "$profile"
env HORIZON_TEST_RESOURCES="$assets" HORIZON_HOME="$profile/game" \
    HORIZON_LAUNCHER_HOME="$profile/launcher" XDG_DATA_HOME="$profile/data" \
    XDG_CONFIG_HOME="$profile/config" HORIZON_LAUNCHER_NO_SELF_UPDATE=1 \
    HORIZON_NO_AUDIO=1 CARGO_TARGET_DIR="$repo/target" \
    cargo +1.95.0 test --locked --manifest-path "$repo/Cargo.toml" \
    -p horizon-client --no-default-features --lib \
    game::native_ui_tests::production_ui_uses_live_vitals_and_inventory_with_no_renderer \
    -- --ignored --exact --test-threads=1
```

### Owner device procedure after explicit clearance

CI can build the unsigned ARM64 game, validate the UIKit bundle and run GPU and save fixtures. The owner must sign the exact approved artifact with their own trusted method, verify its checksum, install it into the standalone test sandbox, and perform the following checks. There is currently no approved archive or checksum to use for installation.

1. Create a disposable world inside the test build. Confirm the Direct Metal gameplay label and moving terrain. Move the camera and player, then check the selected hotbar slot, health and hunger.
2. Open inventory, move one item stack, close it, and verify that world input resumes. Pause and resume. Save & Quit, then reopen that same disposable world and check both the moved stack and a placed block.
3. Background the app once and return. Confirm the input and native view recover, then repeat Save & Quit. Stop on any missing world, save failure, lock conflict, blank native view, or unexpected exit. Retain the disposable world for diagnosis.
4. Record the exact build, host, iPadOS version and visible output. Preserve `Documents/horizon-boot.log` and the test app's `Documents/logs` entries, including `Helium-Host.txt`, `shutdown-save-numeric.txt`, `Helium-Metal-Report.txt`, and game `latest.log` when present. Use the Metal worker summaries for frame pacing and memory warnings. Share evidence privately after removing unrelated personal information.

## S2E HUD, inventory and input

- `Game::native_hud_frame` calls the production `Game::hud` (including `hud_layers` and `draw_inventory`), plus IME, mod hooks, and current map/font atlas patches. Those paths supply the live crosshair, selected hotbar slot, item icons/counts, player health, armor, hunger, XP, effects, text, inventory and menus. Main/world selection stays in the existing UIKit launcher.
- `helium_render_scene::hud` owns the 36-byte `Vertex` also used by `HudBatch`, a physical `Viewport`, the complete frame and ordered triangle runs. `HudAtlas::share_native_pixels` hands the renderer the existing CPU RGBA bake through `Arc`. `Updates` keeps immutable, cumulative 64×64 tile patches so an overwrite in the latest-only request slot cannot lose a glyph or map update.
- `Controller::native_frame_due` applies the existing 16 ms admission cadence before CPU UI construction. `Controller::native_frame`, `runtime/live/ios.rs` and `LiveScene::submit_scene_with_hud` carry one immutable HUD state with the world epoch and camera. The Metal worker composites it after opaque, cutout and translucent terrain and before drawable encoding. Pending frames retain their encoded resources until terminal commands. The worker owns a reusable vertex buffer, texture, and pipeline states. No extra full-screen UI texture is allocated. The UI-only depth runs clear and reuse the existing depth target after terrain finishes.
- `Game::ui_viewport` combines physical window sizing with view-relative safe area from iOS winit. Drawing, cursor mapping, slot hit-testing, controller cursor, IME and touch all use that viewport. The touch inventory and pause controls still synthesize `KeyE` and `Escape`. Pointer events in a screen use that screen's own hit tests. `menu_click`, `menu_ops` and the remote container handler retain authority over slots, carried stacks and server transactions. Cancelling a touch clears a pending drag without finishing quick craft. Suspension clears held touch inputs and revokes the native attachment.
- The ordinary in-game pause menu now owns Resume, Options and Save & Quit. `Game::close_world` requests the existing `begin_closing_save` and the game returns to the launcher only when retirement permits it. The old UIKit placeholder crosshair and Save & Quit controls are gone. An interactive in-world saving-progress overlay remains missing.
- Unsupported HUD effects include enchantment glint, resource-pack text shader overrides, End Portal compositing and blurred menu backgrounds. High-resolution packs whose HUD bake exceeds the budget fail admission. Entity/held-item/world overlays, weather geometry, sprite animation parity, far LOD and MetalFX remain outside this change. Connection screens that need HUD before the first game camera remain a gap.

### S2E resource limits and diagnostics

- One Arc-backed CPU base atlas capped at 96 MiB. The default 4096×5120 packing is 80 MiB. A tile mirror can retain up to 12 MiB per version. The producer, replaceable newest request and worker request share unchanged tiles. Validated vertex capacity is capped at 4 MiB per CPU frame with at most 4,096 runs.
- One reusable GPU atlas and up to 4 MiB shared HUD vertex capacity. The existing 192 MiB live-payload check now charges the HUD and all cached resident geometry even if some sections are offscreen. The existing drawable and terrain upload limits still apply. These are charged allocations and limits. Consult physical testing for iOS process memory and thermal behavior.
- Live metrics include HUD resident bytes, uploaded atlas/tile bytes and HUD preparation in CPU percentiles. The game log emits a separate first completed HUD draw/present for each worker, and requires actual terrain draws for its world receipt. `NativeDisplay.swift` owns the separate UIKit reveal signal. Physical scanout requires device evidence.

### S2E reproduction commands

```bash
cargo +1.95.0 test --locked -p helium-render-scene -p horizon-metal-bridge
cargo +1.95.0 test --locked -p horizon-client-render --lib hud::tests::native_atlas
cargo +1.95.0 test --locked -p horizon-client-render --lib touch::
cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib menu::tests::native_ui_inventory
cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::native_ui_tests
cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml -- --ignored --nocapture --test-threads=1
cargo +1.95.0 build --release --locked --target aarch64-apple-ios -p horizon-client --no-default-features --features audio,helium-native-metal-gameplay
```

`production_ui_uses_live_vitals_and_inventory_with_no_renderer` requires a legally prepared 26.3 pack through `HORIZON_TEST_RESOURCES` and an isolated test profile. To execute it, append `-- --ignored --test-threads=1` to the game-test command. It uses a temporary game instance and does not open a world. The asset-independent menu fixture verifies pickup and placement through the production inventory engine. The macOS Metal readback fixture uses synthetic terrain and HUD bytes. None of those tests exercise device touch input.

### S2E qualification

- Starting SHA `03eb3ede9fb9b6b9d4fdd4e195f51d0e7b15e9b2`. S2E tested implementation `f9fdb9bfba201687c41bc42b6ebebcf26f349639`. The branch includes this later documentation update. [Exact-source CI 44769759160](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44769759160)
- Linux prototype ran 92 unit tests and one integration test. The independent scene and bridge ran 5 and 26, with one bridge fixture ignored for lack of prepared resources. The production atlas, HUD layout and inventory assertions passed one each. Existing CPU touch tests passed 38, and the Python iOS verifiers passed 19. Default and native-feature Linux game checks passed. The resource-backed game test compiled and was ignored. It remains a gap in end-to-end testing.
- The Apple SDK job used Xcode 26.6, compiled the HUD shader for iOS 15, and ran 89 unit tests plus two integration tests in the regular suite. The separate GPU invocation passed all 12 tests, including HUD readback of terrain blending, safe-area scissor, zero reupload of unchanged atlas contents and terminal cancellation. The synthetic HUD test charged 516 bytes for its 1×1 atlas and reused vertex buffer. It uploaded 4, 4 and 0 atlas bytes over three frames. The measured encoding was 160.282 ms for cold setup and 0.901 and 0.946 ms for the two warm frames. These are Mac fixture numbers. [Apple SDK and real Metal job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44769759160/job/136404167869)
- Swift/C/Rust ran 19 recorder tests, the report and recovery round-trip, standalone evidence storage and the ARM64 UIKit framework build. The standalone native example still passes the wgpu-free ARM64 Mach-O audit. The full game Cargo graph still includes `wgpu 30.0.1` through `horizon-client`, `horizon-client-lib`, `horizon-client-render` and `horizon-hdr`, with the bridge reaching it through `horizon-client-render`. The new `helium-render-scene::hud` path uses `bytemuck` only. [Diagnostics and UIKit job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44769759160/job/136404167543)
- The macOS production bridge passed 26 tests with two ignored by default. It explicitly executed the Metal worker's production-format test successfully. The other ignored test requires prepared 26.3 resources. The bridge, CPU terrain input regressions, native game feature and iOS ARM64 game `cargo check` passed. [Production bridge and iOS check](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44769759160/job/136404166839)
- Linux passed 93 save tests, 26 player durability tests, eight retirement tests, four integrated shutdown tests, two save coordinator tests, two fatal-return tests, two launcher termination tests and 26 streaming tests. The detailed job retains the child fixture and prepared-resource ignore reasons. [Linux save, recovery, and shutdown](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44769759160/job/136404167927)
- macOS passed the same save, retirement and streaming counts and ran three launcher termination tests. The full Game UI fixture remained ignored for lack of prepared resources. [macOS save, recovery, and shutdown](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44769759160/job/136404167283)
- The Linux mesher passed 59 tests with 18 ignored. The material snapshot suite passed 12, texture animation passed 12 with one ignored, and mipmaps passed two. The first S2E run exposed an untyped integer in the Apple HUD readback fixture. After typing the readback indices, later runs executed the real GPU fixture successfully. The inventory test originally used a helper that silently returned when game assets were absent. The final asset-independent fixture executes its assertions on both CI platforms. [Initial S2E run](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44767227148)
- All seven execution jobs passed on `f9fdb9bfba201687c41bc42b6ebebcf26f349639`. The release build linked the actual Rust game, the UIKit app and the launcher framework. The bundle verifier matched the embedded executable to the Rust build and verified thin ARM64, device iOS 15.0 minimum, the direct Metal attachment ABI, and diagnostics ABI v2. CI validated and discarded the temporary archive and uploaded the evidence logs. The owner publication step was skipped. [ARM64 game, UIKit and package job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44769759160/job/136411524362)

| S2E CI-only package evidence | Value |
| --- | --- |
| Bundle and build | `dev.helium.g06ctest`, `12001` |
| Game executable bytes | 84,305,432 |
| Game executable SHA-256 | `9339586a76343038c2835549aafca6c1592b93d441528db1ea972f3fe15e085f` |
| Swift launcher framework SHA-256 | `4a9d73f62269420abc26a76e64f5cab18b3215df983ba804d0970f240a36eafa` |
| Temporary held archive SHA-256 | `be25700f8abc4fe440d7b4e58f6fb45950b7d1ee57f44c5d7a4c004db0d99b24` |
| Physical device | NOT TESTED for S2E |

## Source and evidence

- Repository `unfilteredpranxu/Helium-launcher`, branch `feature/helium-g07-immutable-capture`. Run `git fetch origin feature/helium-g07-immutable-capture`, `git status --short`, and `git rev-parse HEAD` in a fresh checkout. The head can include this evidence update after the code commit.
- S2D started from `812d8306d2778df534b654e6246e599a54918822`.
- S2D implementation `ee903d618e9e4bec1fa98baf3e270ae9e7744e8c` includes the two source commits starting with `ca31c92382ac60e97f5d868dd67308ee71e65280`. Check the exact-SHA qualification below before treating a job as passed.
- The S2C reference is `ea81dad30433d9e8e168115845ca0013fc6c8f1e`. [S2C qualification](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44475365740)
- The roadmap is [HELIUM_MASTER_ROADMAP_2026-11-18.md](HELIUM_MASTER_ROADMAP_2026-11-18.md), including the November 20 cutover directive. S2C source and measurements are in [G07-A-S2C.md](../../experiments/metal-prototype/docs/G07-A-S2C.md).

## Native gameplay path at S2D baseline

1. `horizon-client` feature `helium-native-metal-gameplay` includes `helium-direct-metal`. Early `run_game` selection resolves the explicit `native-gameplay` mode before the first graphics factory. It requires iOS, the isolated UIKit launcher, and a visible interactive client. The gated host checks bundle `dev.helium.g06ctest` and diagnostics marker 2. Failed preflight exits through the normal startup error path. The normal package and the existing historical and inset modes keep their existing selection.
2. `Loading::resumed`, `Loading::set_ui`, and `Loading::finish` bypass reference GPU splash, title, and world configuration in this mode. The normal Swift launcher still chooses, creates, or connects to a world through its existing bridge. Generic Play has no native title panorama, so the launcher asks for an explicit world selection instead. `load_resources_for_game` builds native CPU material banks on the existing resource-loader worker. The legacy `load_resources` signature remains available for over 70 game fixtures and the synchronous caller.
3. The real `Game`, integrated server or network client, world/chunk decoding, input, and simulation remain authoritative. `Game::update_world` calls `Streaming::update_and_observe`. The callback hands the accepted `SectionMesh`, origin and stream revision to `Controller::native_sections` before mesh recycling. It forwards unloads to native residency. `Geometry::capture_passes` validates the raw production vertex/sprite ABI, preserves opaque/cutout and translucent index ranges, and tags the world epoch and material generation. The historical and S2B inset paths retain opaque/cutout admission.
4. `Game::render_frame` computes the production interpolated camera, biome atmosphere and day/weather lighting. `Game::render_native_terrain` combines reverse-Z projection, bob/spin, physical window size, and the shared production 16×16 RGBA lightmap with the actual material generation. CPU capture produces no draw-completion claim. It skips assembly of legacy wgpu render passes. Touch movement and look use the existing input layout mapped from the physical winit window. Interaction routed through the normal playing screen remains source-integrated and requires device qualification.
5. `Controller::native_frame` submits `helium_render_scene::Frame` through the existing latest-only worker. The scene owns material snapshot references and an immutable selection of near section geometry. `runtime/live/ios.rs` reuses resident geometry by world, material generation and mesh revision, prioritizes visible sections, and uploads with backpressure. The worker draws solids and cutout first, then draws texture-alpha translucent meshes in farthest-section-first order with opaque-depth testing and translucent depth writes disabled. Within each section the producer order remains. The sky background uses the production atmosphere colour as a clear.
6. `NativeDisplay.swift` attaches a full-size CAMetalLayer to the game view, with a crosshair and Save & Quit button. Other touches reach the winit view. UIKit reveals only after the exact attachment's native drawable command completes and the host is still valid. A completed command and UIKit reveal are distinct from physical iPad pixels. Scene suspension, modal coverage, resize, world replacement and memory pressure revoke the attachment. After the native pressure pause, the game can requeue up to 125 nearby loaded sections once.
7. Both reference graphics factories reject requests while native gameplay is selected. Split-screen startup and in-session resource reload are disabled in that mode. This prevents those paths from creating a second renderer or applying mismatched sprite mappings. Legacy builds retain their normal paths.

### Source map

| Code | Entry points and ownership |
| --- | --- |
| `crates/horizon-client/src/game.rs`, `game/{startup_menu,launcher,native_gameplay,touch,packs,split}.rs` | Mode, asset loader, real game handoff, camera/light frame, input and Save & Quit |
| `crates/horizon-client-render/src/gfx.rs`, `gfx/material_snapshot.rs` | `prepare_native_terrain`, `NativeTerrainMaterials`, `terrain_clip`, `terrain_lightmap`, immutable source banks |
| `crates/helium-render-scene/src/{lib,geometry,materials}.rs` | CPU `Frame`, `SectionVersion`, `Section`, `Camera`, `Lightmap`, `MaterialSet` |
| `crates/horizon-metal-bridge/src/{adapter,runtime}.rs`, `runtime/live.rs`, `runtime/live/ios.rs` | Production geometry admission, epoch/generation validation, bounded cache and latest-request delivery |
| `experiments/metal-prototype/src/terrain/{metal.rs,metal/live.rs,terrain_banked.metal}` | Device and queue, persistent pipelines, atlas bindings, reverse-Z and blending, draw+present receipts |
| `experiments/metal-prototype/src/{attached,section}.rs`, `packaging/ios/Launcher/{NativeDisplay,GameLink,MetalDiagnosticsModel}.swift` | Owned attachment ticket and registry, quad validation, isolated host, display readiness and launcher |
| `.github/workflows/helium-metal-upload.yml`, `tools/ci/{verify-attached-ios-game.py,ios_package.py}` | Exact SHA ARM64 game link, Swift/UIKit, macOS GPU tests, Mach-O/ABI/archive checks and publication hold |

## Render pass coverage

| Pass or function | S2D state |
| --- | --- |
| Opaque and cutout terrain | Real section source wired to multi-section direct Metal. Production vertex layout, sprite banks, original mip data, alpha test and reverse-Z retained. |
| Translucent terrain, water and glass | Real translucent section stream wired to a basic texture-alpha pass. Section-level ordering is approximate. Per-quad ordering, underwater fog, fluid effects and visual parity remain open. |
| Sprite animation | Native gameplay uses initial material chains. Port production texture animation before visual parity. Animated water and other sprites can remain static in this mode. |
| Day/night and sky | Production atmosphere background and dynamic lightmap. Celestial bodies, cloud/weather geometry and fog rendering are missing. |
| Entities, held items, breaking and outline | Gameplay simulation retained. Visual render passes are missing and can make interactions invisible. |
| Launcher, controls and HUD | Existing Swift launcher for world selection and creation. Real input routed with physical view sizing, plus a UIKit crosshair and Save & Quit. Full touch HUD, hotbar, health/hunger, chat, pause/settings and inventory screens are missing. Opening a legacy gameplay GUI can hide its screen controls while the game remains in that UI state. |
| Far LOD, shader packs, MetalFX, interpolation | Deferred to the roadmap after playable native cutover. |

## Memory, pacing and retirement

- One current plus one replaceable latest request, one attachment owner, and one process-wide native GPU permit. Input offers at a 16 ms interval and no event-thread GPU waits. The worker uses real command completion and the exact drawable gate. Metal's retaining command buffers hold the encoded resources through terminal completion after cancellation too.
- Up to 64 CPU sections, 12 MiB high and 8 MiB low eviction watermarks, 16 MiB hard charged CPU geometry cap including stale request pins. Up to 2 MiB per section and per CPU/GPU admission batch, plus up to 16 MiB resident GPU geometry. Nearby candidates take priority and one draw set is capped at 4,096 runs.
- Up to 64 MiB charged resident CPU material snapshot capacity, 112 MiB Metal material payload and 16 MiB upload staging. Game resource source pixels, temporary mip construction and allocations shared with the game have separate lifetimes. The frame validator applies a 192 MiB payload budget, a 6 Mi-pixel output cap, and the UIKit host enforces a separate 64 MiB analytical three-drawable reservation. The counters cover the named allocations. iOS process RSS and jetsam allowances remain unmeasured.
- Summaries on the worker report completed/skipped frames, draw count, upload and eviction counts, retained CPU/GPU geometry, material, staging, targets, available encode/completion/GPU timing and CPU quantiles. The S2D game log labels actual direct device/queue creation and first completed world draw/present, and Swift logs reveal separately. There is no S2D device timing, footprint, stall or thermal measurement.

## Dependency and runtime evidence

- The isolated `experiments/metal-prototype/examples/native_scene.rs` target is still wgpu-free in its Cargo and iOS binary audit. It exercises fixtures. Its detached presentations do not exercise UIKit gameplay.
- The full iOS `horizon-client` target with `audio,helium-native-metal-gameplay` still has `wgpu 30.0.1` reachable from `horizon-client`, `horizon-client-lib`, `horizon-client-render` and `horizon-hdr`. `horizon-metal-bridge` also reaches it via `horizon-client-render`. The runtime control flow isolates the opted-in terrain renderer. Removing linked dependencies requires gating or replacing reference renderer types and the remaining graphics consumers at compile time.
- S1H physically qualified the standalone no-world probe on the owner's A16 iPad. Real S2D iOS game startup, loaded-world pixels, physical input, and save/quit/reopen have no physical evidence yet. All macOS rendering fixtures are synthetic or production-format fixtures.

| Requested runtime evidence | Verified scope |
| --- | --- |
| `NATIVE_GAME_BOOT`, `REAL_WORLD_LOADED`, `PLAYER_INPUT_ACTIVE` | Code wired to the existing game and input. A real S2D gameplay session is unexecuted. |
| `NATIVE_SCENE_PRODUCER` | Source trace from the existing streaming callback and camera. CPU regression validates the production mesh format, separate passes, banks and lightmap. |
| `WGPU_DEVICE_CREATED`, `WGPU_SURFACE_CREATED`, `WGPU_RENDER_SUBMISSIONS` | Native selection rejects the reference constructors by source. Physical counters are unobserved. |
| `METAL_DEVICE_CREATED`, `METAL_COMMAND_QUEUE_CREATED` | Both executed in the Apple Metal tests. The gameplay worker uses the same constructor. |
| `REAL_WORLD_METAL_DRAWS`, `METAL_PRESENT_COMPLETIONS` | GPU and detached presentation fixtures passed. Real loaded-world draws and UIKit presents in S2D remain unobserved. |
| `NATIVE_IOS_DEPENDENCY_AUDIT` | The independent example passes. The full game retains the wgpu dependency graph. |
| `REQUIRED_RENDER_PASSES` | See the coverage table above. |
| `GAMEPLAY_SAVE_GATE`, `PHYSICAL_DEVICE_STATUS` | Gameplay release held. S2D real device gameplay untested. |

## Build and qualification

Use pinned Rust 1.95.0 and the committed lockfiles.

```bash
cargo +1.95.0 test --locked -p helium-render-scene -p horizon-metal-bridge
cargo +1.95.0 test --locked -p horizon-client-render --lib gfx::tests::native_terrain
cargo +1.95.0 test --locked -p horizon-client-render --lib gfx::material_snapshot::
cargo +1.95.0 test --locked -p horizon-client-render --lib mesher::
cargo +1.95.0 test --locked -p horizon-client-render --lib streaming::
cargo +1.95.0 check --locked -p horizon-client --no-default-features --features helium-native-metal-gameplay
cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib save:: -- --test-threads=4
cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib lan::playerdata::tests::
cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::integrated::shutdown_tests -- --test-threads=1
cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::saving::shutdown_tests -- --test-threads=1
cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::runtime_exit::tests -- --test-threads=1
cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml -- --ignored --nocapture --test-threads=1
cargo +1.95.0 build --release --locked --target aarch64-apple-ios -p horizon-client --no-default-features --features audio,helium-native-metal-gameplay
cargo +1.95.0 tree --locked --target aarch64-apple-ios -p horizon-client --no-default-features --features audio,helium-native-metal-gameplay -i wgpu
python3 -m unittest discover -s tests -p 'test*ios*.py'
```

On an Apple runner, generate the Xcode project with `xcodegen generate` from `packaging/ios`, set `HORIZON_GAME_PREBUILT=1`, and build `HorizonGame` for `iphoneos` arm64 with minimum iOS 15 and signing disabled. The current Actions job runs `verify-attached-ios-game.py <app> <Rust executable> --native-gameplay` then inspects a temporary unsigned archive. The workflow keeps owner gameplay publication disabled. The explicit isolated runtime selection is `native-gameplay` in `Documents/helium-metal-test.txt` or `HELIUM_DIRECT_METAL=native-gameplay` when supplied by a development launch. The bundle marker or mode flag is no authorization to install.

[S2D qualification](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44746911766)

- Linux scene and bridge tests passed with 2 scene and 26 bridge tests. The prepared-resource bridge test was ignored because no 26.3 pack was available. Native CPU inputs and material snapshots passed 2 and 12. Production mesher tests passed 59 with 18 ignored, and streaming passed 26 with 2 ignored. Default and feature-enabled Linux game checks and all 19 Python package and executable guard tests passed. The default Linux check used matching Debian ALSA and udev development packages.
- The real Linux `horizon-client` binary returned exit status 1 in a temporary, empty game directory for `HELIUM_DIRECT_METAL=native-gameplay`, with the explicit isolated iOS host error. The native mode cannot silently start the Linux compatibility renderer.
- Apple SDK job `136324480223` passed with Xcode 26.6 and the iPhoneOS 26.5 SDK. The regular prototype suite passed 89 tests and 2 integration tests. A separate ignored-GPU invocation executed all 11 GPU tests successfully, including the new translucent depth/blend readback. The S2B synthetic 120-frame run completed four detached presentations with 0.320 ms median encode and 1.123 ms median GPU command time. The S2C synthetic executable completed 12 frames and four detached presentations and passed the ARM64 wgpu-free Mach-O audit. The separate production bridge job passed 26 tests and explicitly ran the ignored production-format Metal worker test. The prepared-resource regression remained ignored.
- Swift/C/Rust recorder round-trip and UIKit compile passed in job `136324480291`. The Linux shutdown summary logged 93 save, 26 player, 4 integrated shutdown, 2 save coordinator, and 2 runtime exit passes. macOS passed the same counts and a three-test launcher-retirement suite. The shutdown tests used temporary directories and controlled child processes.
- All seven execution jobs passed on code SHA `ee903d618e9e4bec1fa98baf3e270ae9e7744e8c`. Linux diagnostics, both Linux and macOS save/replay jobs, the Apple SDK job, production bridge checks, Swift/UIKit diagnostics, and the final ARM64 game link all succeeded. [Workflow 44746911766](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44746911766)

The ARM64 job verified the real Rust game executable in the real UIKit bundle, device iOS 15.0 minimum, the native gameplay selector and Swift attachment exports, and the temporary archive. CI published evidence logs and removed the game archive. The owner-IPA step was skipped. [iOS game link and package verification job 136330528669](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44746911766/job/136330528669)

| CI-only game package evidence | Value |
| --- | --- |
| Bundle and build | `dev.helium.g06ctest`, build `12001` |
| Game executable bytes | 84,228,512 |
| Game executable SHA-256 | `0317655091b650620008fdeb4694d9d7bf71ac864f322677d804140507f08ab1` |
| Swift launcher framework SHA-256 | `61785d185a165c50352dc038293e835c6b45831eb3e3eba6df60db2d07947477` |
| Temporary `held.ipa` SHA-256 | `323a2b9fa06d227c9ed826bd8bfd9647f081c1fd098ece033f7a24c6977a091d` |
| Signing | Unsigned or linker-only ad-hoc; no distribution signature or provisioning profile |

The initial S2D run caught a changed loader signature used by legacy test callers. The next source commit restored the original wrapper and kept native-specific loading at its dedicated call site. [Initial S2D run](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44745414103)

## Save safety and device restrictions

The in-game Save & Quit action requests the existing owned retirement and only reopens the launcher after completion. The existing background checkpoint, locks, durable redo admission/replay and sticky error paths still govern storage. Force-kill may lose live mutations that have not reached durable admission. Apple runner save tests use synthetic temporary worlds and cannot qualify the iPad host shutdown path. Gameplay owner IPA remains **HELD** under G0. Keep the Actions hold and never use owner worlds for qualification.

## HOW GEMINI SHOULD CONTINUE HELIUM WITHOUT REPEATING PREVIOUS WORK

First run the resource-backed `Game` inventory/vitals regression in an isolated profile with prepared 26.3 data. Then enforce a standalone-only disposable save namespace at world creation, open, and import entry points and qualify the G0 lifecycle on it. Keep the publication guard active until those gates pass. Verify slot pickup/placement, modal input, background recovery and Save & Quit/reopen using the existing S2E HUD and S2D Metal terrain. Preserve upstream MIT attribution.
