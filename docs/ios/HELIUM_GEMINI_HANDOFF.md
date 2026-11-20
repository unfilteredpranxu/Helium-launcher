# Helium Launcher — S2D handoff for Gemini

**Status: PARTIAL.** The isolated iOS build has a direct Metal gameplay terrain path in the real Rust game. Full playable rendering, wgpu-free gameplay linkage, and physical gameplay qualification remain open. The gameplay IPA is held.

## Source and evidence

- Repository `unfilteredpranxu/Helium-launcher`, branch `feature/helium-g07-immutable-capture`. Run `git fetch origin feature/helium-g07-immutable-capture`, `git status --short`, and `git rev-parse HEAD` in a fresh checkout. The head can include this evidence update after the code commit.
- S2D started from `812d8306d2778df534b654e6246e599a54918822`.
- S2D implementation `ee903d618e9e4bec1fa98baf3e270ae9e7744e8c` includes the two source commits starting with `ca31c92382ac60e97f5d868dd67308ee71e65280`. Check the exact-SHA qualification below before treating a job as passed.
- The S2C reference is `ea81dad30433d9e8e168115845ca0013fc6c8f1e`. [S2C qualification](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44475365740)
- The roadmap is [HELIUM_MASTER_ROADMAP_2026-11-18.md](HELIUM_MASTER_ROADMAP_2026-11-18.md), including the November 20 cutover directive. S2C source and measurements are in [G07-A-S2C.md](../../experiments/metal-prototype/docs/G07-A-S2C.md).

## Native gameplay path

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

The UIKit Save & Quit action requests the existing owned retirement and only reopens the launcher after completion. A pending reply names a saving phase. The existing background checkpoint, locks, durable redo admission/replay and sticky error paths still govern storage. Force-kill may lose live mutations that have not reached durable admission. Apple runner save tests use synthetic temporary worlds and cannot qualify the iPad host shutdown path. Gameplay owner IPA remains **HELD** under G0. Keep the Actions hold and never use owner worlds for qualification.

## HOW GEMINI SHOULD CONTINUE HELIUM WITHOUT REPEATING PREVIOUS WORK

Port the essential in-world HUD and inventory input surface from the existing CPU `HudBatch` and game screen state to a native UIKit or Metal compositor. Use the current game and attachment rather than adding another no-world probe. Start with one real inventory open, select and close regression. Maintain existing gameplay control and save ownership. Keep wgpu dependency work, animation, entity passes and device qualification additional open gates. Use exact source SHA and genuine engine/GPU/device evidence for every later claim, and preserve upstream MIT attribution.
