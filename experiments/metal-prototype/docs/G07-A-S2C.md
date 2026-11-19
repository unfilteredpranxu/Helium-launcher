# S2C · CPU scene boundary and independent native Metal

| Source | Identity |
| --- | --- |
| Starting source | `c2f8a9e6dc9d0ac556ed12e9af8a0c4c96d21180` |
| Tested implementation | `ea81dad30433d9e8e168115845ca0013fc6c8f1e` |
| Branch | `feature/helium-g07-immutable-capture` |
| Qualification | [Actions run 44475365740](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44475365740) |

## Recovery and source boundary

The recovered checkout had no tracked edits and no interrupted scene crate. The existing untracked S2B report was preserved. The coherent CPU types were committed before integration and GPU qualification work.

`helium-render-scene` provides the expanded 24-byte terrain ABI, an owned section with epoch, material generation, key and mesh revision, a double-precision camera position and clip matrix, revisioned lightmap, and an immutable material-set owner. The crate forbids unsafe Rust and depends on bytemuck. It does not import Horizon's renderer, wgpu, or native graphics APIs.

The existing game section adapter exports authentic accepted mesh vertices and indices. Its allocated vectors move into the CPU section at the live-mode boundary. `State::scene_materials` validates the exact prepared snapshot revision and uses the original `Arc<Snapshot>` as backing storage for all banks, with no pixel copy. The CPU snapshot tests exercise this adapter using production mip preparation without constructing a renderer or device. The legacy game still supplies the frame's camera and lightmap through its renderer-backed adapter.

The S2B worker now receives one latest-only owned scene frame. It validates the world epoch and material generation, keeps resident Metal buffers for matching section revisions, and removes replaced or unselected sections. Native scene admission checks the material identity and world epoch again before a draw. The existing bank residency, frame completion permit, CAMetalLayer path, and terminal ownership are used unchanged. The historical S2A mode remains separate.

## Scope and bounds

Existing production limits still apply. The latest-only mailbox has one queued and one worker-owned request. It shares up to 64 sections, uses 12/8 MiB CPU cache watermarks, and charges up to 16 MiB of geometry including pinned evictions. The GPU geometry cap is 16 MiB. Material-source retention remains within the 64 MiB snapshot budget, and each Metal texture upload has a 16 MiB staging ceiling. The existing process-wide GPU permit holds submitted resources until terminal completion. These are scoped allocations and policies. A16 memory use and frame pacing have no new measurements.

The no-world `native_scene` executable uses the same CPU scene boundary and S2B Metal admission and submission APIs. Its workload uses original synthetic data in the production format, changes the camera, replaces and unloads a section, validates epoch rejection and cancels with pending GPU work. It has no world loader or save path. Its local locked build graphs for Linux, macOS arm64 and iOS arm64 exclude wgpu and all Horizon gameplay crates. The full app's renderer, startup, UI and other passes still depend on wgpu. No shader, MetalFX, LOD, game UI or save-path changes were made.

## Reproduction

All Rust checks use Rust 1.95.0 and committed lockfiles.

| Command | Local result |
| --- | --- |
| `cargo +1.95.0 test --locked -p helium-render-scene` | 2 passed. |
| `cargo +1.95.0 test --locked -p horizon-metal-bridge` | 24 passed, 1 ignored prepared-resource oracle. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib gfx::material_snapshot::` | 12 passed. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib streaming::` | 26 passed, 2 ignored. |
| `cargo +1.95.0 test --locked -p horizon-client-render --lib mesher::` | 59 passed, 18 ignored. |
| `cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml` | 92 tests and 1 doctest passed. |
| `python3 experiments/metal-prototype/tools/test_section_bridge.py` | 7 passed, 1 ignored prepared-resource oracle. |
| `cargo +1.95.0 check --locked --manifest-path experiments/metal-prototype/Cargo.toml --all-targets --target aarch64-apple-darwin` and `--target aarch64-apple-ios` | Rust cross-checks passed locally. |
| `cargo +1.95.0 tree --locked --manifest-path crates/horizon-android/Cargo.toml --target aarch64-linux-android -i helium-render-scene` | Locked graph resolved. The existing Android lockfile required synchronization with the already-updated in-tree gameplay manifests. |

The gameplay IPA remains **held**. Existing S1H synthetic device results and S2B fixture results do not establish authentic world scanout or safe standalone gameplay saves on iPad. S2D was not started.

## Independent Apple target evidence

`cargo +1.95.0 run --locked --manifest-path experiments/metal-prototype/Cargo.toml --example native_scene` executed the synthetic scene on a macOS Apple GPU. It submitted 13 commands, observed 12 valid frame completions and four detached CAMetalLayer presentation completions, and cancelled the last submitted frame. Three section uploads covered two initial sections and one replacement. The fixture recorded 336 bytes of peak native section payload plus draw-run metadata, 572 µs median CPU encode time, and 1,690 µs median GPU command time. The shared GPU permit retired fully. These measurements describe a 64×64 synthetic workload.

All ten existing real Metal GPU tests passed separately, including the S2B 120-frame fixture. The default macOS prototype suite passed 89 tests and two doctests. Its ten ignored hardware cases were exercised in the separate serial run. The Swift/UIKit and diagnostics checks also passed.

`cargo +1.95.0 build --locked --manifest-path experiments/metal-prototype/Cargo.toml --target aarch64-apple-ios --release --example native_scene` linked the exact native-only binary. The existing Mach-O verifier established a thin ARM64 iOS 15.0 device executable and entry point. The dependency graphs and linked symbol and library audit reported no wgpu and confirmed the direct Metal device API. The qualification binary SHA-256 was `d379ac86daf5f47543f9d33b4766e0ef7c49d9c8498b51a6329339b110db3e09`. It is a standalone qualification executable without device deployment evidence. [Apple SDK job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44475365740/job/135394637426) · [Native GPU and link logs](https://github.com/unfilteredpranxu/Helium-launcher/runs/135395983969)

The first candidate passed the native runtime workload and iOS link. Its audit failed because that macOS runner lacked ripgrep and `lipo` misparsed the flag order. The final run uses portable grep and the repository's Mach-O verifier. [Prior audit log](https://github.com/unfilteredpranxu/Helium-launcher/runs/135393981824)

`cargo +1.95.0 tree --locked -p horizon-client --target aarch64-apple-ios --no-default-features --features audio,helium-direct-metal -i wgpu --depth 2` still finds wgpu 30.0.1 under `horizon-client`, `horizon-client-lib`, `horizon-client-render` and `horizon-hdr`. The remaining dependency is on the game side of the scene boundary.

## Gameplay compatibility regression

The production bridge job passed 24 macOS unit cases and separately ran the existing native production-format worker fixture. The real-resource case is still ignored because CI has no prepared Minecraft resources. The native opt-in game compiled for macOS and iOS, and the iOS bridge compiled against the production renderer. Both shutdown/save jobs passed 8 worker, 93 save, 26 player durability, 8 integrated/save/fatal exit and 26 streaming tests each. Ordinary test discovery ignored six save cases, the child-process fatal-exit helper, and two streaming benchmarks. The parent harness launches its child helper explicitly. [Production bridge](https://github.com/unfilteredpranxu/Helium-launcher/runs/135398437292) · [Linux retirement](https://github.com/unfilteredpranxu/Helium-launcher/runs/135396829378) · [macOS retirement](https://github.com/unfilteredpranxu/Helium-launcher/runs/135398560415)

All seven execution jobs passed on the tested implementation. The full opt-in gameplay executable linked with `cargo +1.95.0 build --release --locked --target aarch64-apple-ios -p horizon-client --no-default-features --features audio,helium-direct-metal`. Xcode built the UIKit package. The verifier matched the package executable to the linked Rust image and passed the isolated `dev.helium.g06ctest` structure check. The held archive was discarded. CI published three evidence-log artifacts and no installable IPA. [Gameplay package job](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44475365740/job/135398855709) · [Package verification evidence](https://github.com/unfilteredpranxu/Helium-launcher/runs/135407796902)
