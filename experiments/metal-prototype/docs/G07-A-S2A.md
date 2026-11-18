# S2A · Production terrain capture for Apple Metal

| Source | Identity |
| --- | --- |
| Baseline | `8be13e8f4d42d254b030f8e0778cd0796a4c22e5` |
| Tested code | `bfeaefc2216530cbd3d166f7c3a87dea51ff01a9` |
| Branch | `feature/helium-g07-immutable-capture` |
| S2A qualification | [Actions run 44250197579](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44250197579) |

## Production handoff

1. The render streaming worker runs `Mesher::mesh_reusing` and sorts the section's original expanded vertices. `Streaming::update` accepts the result only if its job sequence still matches and the column still exists. `Streaming::update_and_observe` pairs the returned mesh with the streamer's recorded compile revision. It excludes columns lost during the same update.
2. The game borrows the batch through `Controller::sections` before handing meshes to wgpu or recycling their storage. The controller picks at most one section to copy per update. It uses already-computed mesh bounds and six Metal clip planes. The bounds test allows crossing geometry and model overhang. Selection favors sections with more solid geometry without claiming anything about occlusion.
3. After the real renderer updates the frame, `Controller::frame` checks the camera, drawable dimensions, renderer generation, live world epoch, and required bank provenance. The job captures original 24-byte expanded opaque/cutout vertices, the production quad indices, a camera-relative section origin, the game's primary view/projection with bob and spin, and the exact 1,024 lightmap bytes and revision. All required banks pin one immutable snapshot with its original mip data and global sprite IDs. Strict detached mode keeps current-revision validation. Attached historical mode permits later animation/lightmap content in the same generation.
4. The existing Metal worker passes issued bank uploads through terminal receipts before drawing with reverse-Z. The render encoder batches consecutive triangles that share a sprite bank. This retains the original triangle order, depth ties and index bytes without creating another mesh or index buffer. Drawing completes before the worker acquires a drawable and submits presentation. The native completion gate, matching attachment, and current UIKit host checks must all agree before the main thread reveals the historical snapshot. wgpu remains active.
5. The selected section's replacement, either local or server-forced column unload, and hard world/renderer/host changes revoke the capture. GPU completion owners retain submitted resources after cancellation. A shown historical frame keeps a lightweight capture identity and host ticket. The historical CPU snapshot and geometry belong to the worker and its in-flight owners.

The snapshot covers the existing opaque/cutout pass. The recorder counts omitted translucent vertices in mixed sections. Water and other blended draws, independent packed geometry, entities, far LOD, shader packs, and live camera updates are outside this capture. Authored mip chains and mapped UVs reach the GPU with their original bytes. The diagnostic shader retains its original point sampling and disabled backface culling. Production mip/filter and culling parity remain outside the qualified visual subset.

## Accounting and diagnostic evidence

The geometry cap is 2 MiB of retained vertex/index capacity. The existing 64 MiB snapshot budget charges one producer's live and copy-on-write payload allocations. The existing native policy has a 192 MiB frame/upload payload ceiling, 16 MiB upload staging ceiling, a 6 Mi-pixel render-target cap, and up to 64 MiB of analytical drawable reservation. These are scope-specific bounds. There is one production capture worker in the process and the existing Metal permit continues to guard outstanding command resources. The render deadline is 5 seconds and a capture expires after 10 seconds. The existing host TTL is 35 seconds, with a separate five-second reveal timer. None of these bounds is a measurement of iPad RSS.

Schema v2 retains its 32-word Swift/C ABI. `WorldSource` records the producer origin and omitted translucent vertices. It cannot report a test-only attempt as `REAL`. `CaptureMetrics` records game-thread selection/copy and final validation microseconds with geometry capacity. `UploadPreparation` records per-bank worker-side preparation through submission. Existing timed stages record terminal upload, draw, drawable and presentation operations, and the UIKit reveal independently. The DrawSubmitted values are triangle count and actual indexed Metal draw count. These stage timestamps reflect host observation. The report includes staging peak, retained source peak, possible in-flight GPU work and typed first causes. Synthetic fixtures do not establish Minecraft performance or physical pixels.

## Physical boundary and release

The owner verified the S1H standalone no-world probe, build 13001, on the iPad A16 with iPadOS 27.3 beta. That previously qualified device session displayed synthetic Metal charts and resumed presentation after scene reactivation. S2A still requires a qualified standalone gameplay host, disposable-world testing with legitimately prepared resources, an authentic upload/draw/present trace, and video or a screenshot of the labeled terrain. The workflow continues to hold the gameplay IPA. Probe package verification remains in CI and publishing a new no-world artifact requires explicit workflow dispatch.

## Reproduction commands

Commands use Rust 1.95.0 and the workspace lockfiles. The workflow also verifies the exact unsigned archives and keeps its log artifacts distinct from installable artifacts.

| Command | Linux result |
| --- | --- |
| `cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml` | 91 tests and 1 doctest passed. |
| `cargo +1.95.0 test --locked -p horizon-metal-bridge` | 21 passed, 1 ignored. The ignored mesher oracle requires prepared Minecraft resources. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib streaming::` | 26 passed, 2 ignored benchmark cases. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib mesher::` | 59 passed, 18 ignored. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib gfx::animate::` | 12 passed, 1 ignored. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib mipmap::` | 2 passed. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib gfx::material_snapshot::` | 11 passed. |
| `cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib save:: -- --test-threads=4` | 93 passed, 6 ignored. |
| `cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib lan::playerdata::tests::` | 26 passed. |
| `cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::integrated::shutdown_tests -- --test-threads=1` | 4 passed. |
| `cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::saving::shutdown_tests -- --test-threads=1` | 2 passed. |
| `cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::runtime_exit::tests -- --test-threads=1` | 2 passed. The child helper is ignored in normal test discovery and launched from the parent regression. |
| `python3 experiments/metal-prototype/tools/test_section_bridge.py` | 7 passed, 1 ignored for prepared resources. |
| `python3 -m unittest discover -s tests -p 'test_helium_ios_package.py'` | 10 passed. |
| `python3 -m unittest discover -s tests -p 'test_attached_ios_binary.py'` | 4 passed. |

The bridge's production-format fixtures use handcrafted geometry or pixels. The CI host has no prepared Minecraft assets. The workflow runs CPU checks, Apple GPU readback, iOS builds, and package verification independently.

## Apple GPU and ABI evidence

- The macOS Apple SDK job passed `cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml` with 88 tests and 2 doctests. The nine hardware cases were run separately using `-- --ignored --nocapture --test-threads=1` and all passed. This includes 512-slice bank selection, clipping/depth and readback, BGRA conversion, drawable submission, and retirement of the layer and GPU resources after cancellation. The 512-bank oracle drew the same 14 ordered triangles in four indexed Metal calls on both passes and verified each pixel against its expected value. Its timing and count belong to the fixture. [Apple SDK job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44250197579/job/134618873951) · [Native GPU evidence](https://github.com/unfilteredpranxu/Helium-launcher/runs/134619681189)
- The same job compiled the iOS-targeted Metal shaders and standalone probe and verified the exact resulting unsigned probe archive. This run did not publish an installable probe. [Apple SDK job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44250197579/job/134618873951)
- Recorder, recovery and HUD-model round trips passed across Swift, C ABI and Rust. The Apple job also compiled the real UIKit framework. The round trip's events are test-only. [Diagnostics job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44250197579/job/134618873898) · [Diagnostics evidence](https://github.com/unfilteredpranxu/Helium-launcher/runs/134619716832)
- The production bridge job passed 21 unit tests with two ignored in normal macOS discovery. It separately executed `HORIZON_SPRITE_SPLIT=512 cargo +1.95.0 test --locked -p horizon-metal-bridge --lib runtime::ownership_tests::native_worker_from_production_owned_bank_and_typed_section -- --ignored --exact --nocapture` and passed the real Metal worker case. The remaining ignored test needs prepared Minecraft assets. The handcrafted worker fixture completed three uploads, three indexed draws in one command, and one detached presentation. It recorded 20,540 uploaded bytes, 393,216 peak staging bytes, 21,092 peak retained source bytes, and zero pending upload/frame work on completion. The test took 1.75 seconds wall clock and used a synthetic camera and lightmap. Both the bridge and the opt-in game passed the arm64 iOS target checks. [Bridge job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44250197579/job/134618873490) · [Bridge evidence](https://github.com/unfilteredpranxu/Helium-launcher/runs/134623247897)
- The Ubuntu and macOS shutdown jobs each passed eight worker, 93 save, 26 player, eight integrated/save/fatal-exit, and 26 streaming tests. Six save cases, the separately invoked fatal-exit child helper, and two streaming benchmarks were ignored in normal discovery. The host unit cases passed on Ubuntu (two cases) and macOS (three cases). [Linux shutdown evidence](https://github.com/unfilteredpranxu/Helium-launcher/runs/134621213798) · [macOS shutdown evidence](https://github.com/unfilteredpranxu/Helium-launcher/runs/134623332562)

## iOS release link and publication

All seven execution jobs completed successfully for the tested source. The release job used `cargo +1.95.0 build --release --locked --target aarch64-apple-ios -p horizon-client --no-default-features --features audio,helium-direct-metal` and linked the real `HorizonGame` with the `HorizonLauncherKit` framework using Xcode. The default dependency check excludes both experimental Metal crates. The executable/bundle verifier matched the 84,039,296-byte arm64 iOS 15 executable exactly, verified the v1 host exports and diagnostics v2 marker, and checked the absence of Java/JVM, distribution signatures, provisioning profiles, embedded saves, private game assets and configuration.

The final archive verifier passed for `dev.helium.g06ctest`, version `0.2.8`, build `12001`.

| Verified digest | SHA-256 |
| --- | --- |
| Exact held gameplay archive, discarded after verification | `9c20ca6819778edfb1eee18a1a3868e875c94a3d32015f25da64b4e617b8531b` |
| Embedded Rust game executable | `c336a063e4ed883a7cb6677b58ce74c8afaec2c0f6bbd50664ec643bc224294d` |
| Embedded UIKit launcher framework | `a500cec54371036674db2c617dc5c3b98f096182e833b4d2436f8c15e1269879` |

The workflow published evidence logs only. It did not publish a gameplay or no-world IPA. The gameplay hold remains active. [Game link job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44250197579/job/134623367044) · [Package verification output](https://github.com/unfilteredpranxu/Helium-launcher/runs/134631551316) · [Qualification artifacts](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44250197579#artifacts)
