# Helium Launcher — durable AI-engineer handoff (20 Nov 2026)

> **Status: Native-Metal gameplay cutover NOT COMPLETE.** This file is deliberately truthful: it is a checkpoint for Gemini / Codex / future engineers, not a claim of a playable wgpu-free Minecraft app. Update with exact verified evidence after each implementation.

## Mission and priority (owner-approved)

Implement a fully native Rust Minecraft-compatible iOS/iPadOS game renderer using Helium's **own direct Apple Metal** GPU API path, not a wgpu/WebGPU rendering layer. The final native-only iOS gameplay executable must run real Minecraft world and input, continuously display terrain, essential visible gameplay objects/effects and UI, and contain no active or hidden wgpu graphics fallback. Preserve original gameplay/networking/world semantics, resource fidelity, saves and licensing. Optimize measured memory and frame-pacing on iPad A16 6 GB. **Defer** fancy shader packs, distant LOD expansion, MetalFX upscaling/frame interpolation and cosmetic launcher polish until the playable renderer is independent, correct and safe.

The binding roadmap is `docs/ios/HELIUM_MASTER_ROADMAP_2026-11-18.md` including its **20 Nov owner-approved final-session directive**. Do not infer unsupported completion from old phase documents.

## Source identity

- Repository: `unfilteredpranxu/Helium-launcher`.
- Working branch: `feature/helium-g07-immutable-capture`.
- Last fully tested implementation at handoff creation: `ea81dad30433d9e8e168115845ca0013fc6c8f1e`.
- Last qualifying CI: https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44475365740 — seven execution jobs successful on exactly that tested implementation.
- S2C report: `experiments/metal-prototype/docs/G07-A-S2C.md`.
- The 20 November roadmap documentation update: `9aee51f7175221f8603f9f755f17112c58705ec6`.
- **Before changes** read `git rev-parse HEAD`, `git status --short`, latest remote branch, workflow runs, and any subsequent reports. Preserve uncommitted work. Never reset blindly.

## What actually works (qualified evidence)

1. S1H standalone, no-world native Metal diagnostic IPA ran on the owner's iPad A16/iPadOS 27.3 via their installer. GPU readback G01/G02 and UIKit/CAMetalLayer native diagnostic presentation/resume were observed. This does **not** demonstrate live Minecraft world rendering.
2. S2A actual production `SectionMesh` capture bridge and real camera/material/lightmap data handoff are implemented, while source-world runtime pixels on iPad remain unverified.
3. S2B live native Metal resident terrain GPU path, multi-section budgeting, latest-only frames and an Apple GPU **synthetic** 120-frame fixture are implemented. Real Minecraft gameplay remains on wgpu.
4. S2C `crates/helium-render-scene` is a backend-independent, `#![forbid(unsafe_code)]` CPU terrain/scene crate. Existing S2B Metal consumes it. `experiments/metal-prototype/examples/native_scene.rs` is an independent no-world native-Metal qualification example. Its **own isolated dependency graph and iOS arm64 executable** were verified wgpu-free; an Apple GPU synthetic run completed 12 frames, four detached presentations. **This is not a wgpu-free full Minecraft client.**

Important source:
- `crates/helium-render-scene/src/{lib,geometry,materials}.rs`
- `crates/horizon-client-render/src/streaming.rs`
- `crates/horizon-client-render/src/gfx/material_snapshot.rs`
- `crates/horizon-metal-bridge/src/runtime.rs`
- `crates/horizon-metal-bridge/src/runtime/live.rs`
- `crates/horizon-metal-bridge/src/runtime/live/ios.rs`
- `experiments/metal-prototype/src/terrain/metal/live.rs`
- `experiments/metal-prototype/src/attached.rs`
- `packaging/ios/Launcher/NativeDisplay.swift`

## What is explicitly NOT done

- The full `horizon-client` / `horizon-client-render` game startup, game camera, materials, UI, HUD and other rendering still rely on wgpu; dependency graph also includes `horizon-client-lib` and `horizon-hdr`.
- No fully independent, authentic-world, full-screen, interactive native-Metal gameplay session has been qualified on the iPad.
- Complete near terrain + translucent fluids + entities + sky/weather + essential UI render parity is unqualified.
- Continuous A16 real-game FPS, P95/P99 frame times, peak footprint, memory warnings, battery and thermal load have NOT been measured.
- Shader packs, distant terrain integration, MetalFX and frame generation are NOT implemented/verified on this new native-only game path.
- **Gameplay IPA owner publication is HELD** until G0 safe host lifetime, device disposable-world save/quit/reopen and relevant release gates pass. A CI-linked archive verifier is not permission to install unsafe gameplay builds.

## Next prioritized implementation — actual game cutover

1. Reuse existing production chunk/world decode and mesher without requiring a wgpu device. Feed genuine scene inputs through `helium-render-scene`; preserve correct material generation, lightmaps, camera, world epoch and ownership.
2. Wire an explicit native-only **gameplay mode** that boots engine/network/world, uses real input and a full-screen direct Metal layer, does **not** create a wgpu GPU device/surface/pipeline, and fails closed without native Metal.
3. Implement required gameplay-visible passes through native Metal or native UIKit: opaque/cutout, translucent fluids, entity/block-entity visuals needed to play, sky and essential HUD/menus. Record parity gaps honestly.
4. Preserve and improve bounded resource cache, async upload/retirement, latest-only frame inputs, no main-thread waits, memory accounting and frame-time metrics. Do not claim 0 ms lag or unrealistic guaranteed FPS.
5. Enforce exact native-only game target binary/Cargo dependency audits (not just the existing `native_scene` example); classify runtime and device proof separately.
6. Maintain saving/shutdown guarantees; only release a gameplay IPA after independent safe physical tests. Use disposable worlds; never owner's real saves.

## Reproduction and verification starting points

Pinned toolchain Rust 1.95.0; committed lockfiles; check exact current manifests before commands.

```bash
git status --short
git rev-parse HEAD
cargo +1.95.0 test --locked -p helium-render-scene
cargo +1.95.0 test --locked -p horizon-metal-bridge
cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml
cargo +1.95.0 tree --locked -p horizon-client --target aarch64-apple-ios --no-default-features --features audio,helium-direct-metal -i wgpu --depth 2
```

Read `.github/workflows/helium-metal-upload.yml` for supported macOS Metal GPU, iOS ARM64 linked-game, Swift/UIKit, binary-guard, save/shutdown and package-verifier jobs; use Actions macOS runner for Apple-only execution. Never claim macOS GPU synthetic measurements are real A16 performance.

## Handoff checklist at session end

Update this file with:
- Exact latest pushed source SHA, and which SHA CI actually tested.
- Current `native-only-game` (or equivalent) build target and precise Cargo features.
- Files changed, code architecture, remaining wgpu routes, pass-by-pass parity and actual build commands.
- Evidence classification: synthetic CPU, synthetic macOS GPU, authentic Minecraft world in macOS GPU, real iPad physical pixels, full gameplay on iPad.
- CI run URL, executed job conclusions, ignored/failed tests, artifact URLs and device-install qualifications.
- Memory budgets, GPU work ownership, measured frame statistics with hardware/preset, reproducible failing scenario.
- G0 gameplay IPA safety HOLD/PASS reason and safe next task.
- Any required legal/attribution/licensing notes.

**Do not mark the task complete simply because an isolated Metal example links. Real, playable, verifiably wgpu-free Minecraft gameplay is the acceptance target.**
