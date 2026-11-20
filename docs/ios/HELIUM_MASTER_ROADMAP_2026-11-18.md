# Helium Launcher — locked master roadmap
**Plan version:** 1.0 · **Created:** 18 November 2026 (Asia/Kolkata)  
**Working branch at plan creation:** `feature/helium-g07-immutable-capture` at `a2357ac9cef89c41563b3d66697ba7b957b2544d`  
**Repository:** `unfilteredpranxu/Helium-launcher`  
**Status:** G07-A Revision 3 source/CI passed; **P0 OWNER RELEASE BLOCKED**. Prompt 12 not complete, Prompt 13 not started.  
**This is the single source of truth for planning.** Source and device evidence supersede guesses; do not silently change priorities or acceptance gates. Changes to this plan require a clearly documented reason and the owner's approval.

## 0. Non-negotiable product charter

Build an iOS/iPadOS Minecraft: Java Edition-compatible native Rust client named **Helium Launcher**, with its **final gameplay rendering backend calling Apple Metal directly**, not wgpu/WebGPU. The existing Horizon-derived Rust simulation, networking, chunk/data pipelines, and prepared resources are reused and properly attributed; no JVM is required. SwiftUI/UIKit are the Apple-native launcher/host layer.

Final-state requirements:
- **Direct Metal**: own CAMetalLayer/drawable, MTLDevice, command queues/buffers, GPU texture/buffer and render-pipeline lifetimes, MSL shaders, explicit depth/blend/ordering, and native presentation. No wgpu drawing dependency in the final iOS gameplay executable.
- **Correctness**: gameplay parity with the upstream version within documented supported features; real terrain, entities, UI, fluids, animation, sky/weather and multiplayer where supported.
- **Visual quality**: native shader packs and an honestly scoped compatibility/translation path for third-party shader formats, realistic water/lighting and distant terrain/LOD. Existing Iris/OptiFine packs are **not assumed** to work just because Metal is used.
- **Performance**: prioritize low jitter and stable frame times on iPad A16/6 GB; 60 FPS is a target, not an unmeasured claim. Avoid blurriness and preserve visible quality; test thermal behavior.
- **Durability**: no falsely acknowledged writes or unsafe release of a world/session owner. Quitting and reopening must preserve acknowledged progress; unexpected terminations can lose uncheckpointed live ticks, which must not be misrepresented as guaranteed durable.
- **Polished launcher**: iPad-first native UI, safe accounts/imports/updates, accessible touch/controller/keyboard interactions, mods/packs appropriate to Rust, correct attribution/licensing.
- **Preserve existing worlds**: never change, delete, migrate or stress the owner's actual saves to qualify a prototype. Disposable test worlds only; backup before any new IPA test.

**Temporary wgpu allowance:** only as a known-good comparison/fallback while building parity; it is **not** a completion milestone and must be removed from the final iOS gameplay render graph and dependencies after direct Metal passes feature/correctness gates. Do not remove it prematurely and make the game unplayable.

## 1. Verified starting evidence (not progress estimates)
- G07-A Revision 3 is on source commit `a2357ac9cef89c41563b3d66697ba7b957b2544d`.
- [CI 44068032930](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44068032930): complete, success; all seven execution jobs passed, including iOS build/link and Metal fixture tests.
- Revision 3 implemented owned nonblocking save shutdown for ordinary close/error, redo/replay for **durably admitted** region/player/metadata writes, sticky errors, lock retention and killed-process/reopen tests.
- **Remaining P0**: fatal `run_app` / `run_loading` return still reaches `std::process::exit(...)`; a process/guest unload can end outstanding retirement before dirty live state is admitted. The passing regression `fatal_runtime_exit_exposure_remains_a_release_blocker` demonstrates the hazard, not a safety pass. LiveContainer-specific host semantics remain unproven. **No owner IPA authorized.**
- G07 native Metal is an immutable *historical one-section capture*, with real Apple GPU fixture completion but **no verified actual-game iPad scanout** and no continuous Metal-rendered gameplay. Normal gameplay is still wgpu-backed.
- A previous LiveContainer OS report shows FRONTBOARD `0x8BADF00D`: failure to exit gracefully in five seconds, with the main thread blocked in `pthread_join`. It is *not* proof of a Metal crash or OOM. Different incidents must not be merged.
- Reference technical report: `experiments/metal-prototype/docs/G07-A.md`; owner hold: `G07-A-owner.md`.

## 2. The 48-hour critical sprint (18–20 Nov 2026, IST)

The sprint is an **execution budget**, not a promise to finish all of Helium in two days. The primary target is **one honest native-Metal scene milestone**, gated by safe world lifecycle. Two available Codex/model usage windows should be reserved for substantive implementation, not repeated audits or cosmetic documents.

### S0 — Fast baseline (maximum 30 minutes, no broad re-audit)
- Fetch exact current branch HEAD and G07 Revision 3 report; record any new commits.
- Verify already-passing CI; do **not** rerun green jobs without a code or configuration change.
- Keep branch/commit/build metadata exact. Save this plan in repo.
- **Exit:** evidence hash and next coding task fixed.

### S1 — P0 fatal return/lifetime (first coding window; timebox ~6–8 active engineering hours)
**Single coding objective:** eliminate the **unsafe fatal event-loop return / process exit** path without allowing world owners to be destroyed, their locks released, or unadmitted dirty state silently discarded.
- Trace `run_loading`, `game::run_with`, `entry.rs::main` and iOS/winit/LiveContainer lifecycle. Make a minimal host/runtime ownership contract that allows nonblocking UI/host servicing and positive durable handoff before controlled exit. Never block the main thread in a `join`, busy-loop or pretend a timeout means success.
- Preserve current redo/journal, sticky errors, lock ownership, background checkpoint request and G07 capture. Do not replace `process::exit` with an equally unsafe early return or an unowned detached worker.
- Add **red-before-green** tests for fatal-return with dirty state, early-error paths, suspended/ordinary close, outstanding job/lifetime, replay/failed admission, lock ownership, idempotent reopen, and exactly what is acknowledged. Include host-specific limitations explicitly.
- Run targeted Linux/macOS and actual iOS arm64 link/bridge/verifier jobs. Keep release package HOLD until exact tested source and host safety gate pass.
- **PASS gate:** fatal-return no longer prematurely kills/unloads owned dirty work; event loop stays responsive and honors terminal state; proven durability semantics and error propagation; no hidden `process::exit` in a live-world path; green exact CI. Only then qualify **one unsigned isolated IPA** for disposable-world iPad testing.
- **BLOCKED gate:** preserve the HOLD and publish exact failing code path. **Stop after one coding window**; do not consume both resets in the same shutdown rabbit hole. Continue S2 in isolated CI/macOS native-Metal mode, with no owner installation.

### S2 — Direct Metal physical scanout of real world terrain (second priority)
**Aim:** visible **real Minecraft-generated terrain** rendered by Helium's own direct Metal, clearly distinguished from wgpu background and from a synthetic test triangle.
- Reuse G07's captured real SectionMesh, copied materials/lightmap and current Metal command/presentation infrastructure.
- Fix actual render-client lifetime, drawable placement, depth/transform/material/UV and exact layer ownership only as needed. No wholesale renderer rewrite in this step.
- Build a diagnostically isolated, short and distinct native view; evidence must show actual physical pixels in an owner video, plus same-build logs linking capture → native GPU draw → presentation completion → reveal. Do not equate logs or 60 FPS wgpu HUD with native scanout.
- If S1 PASS: qualified isolated unsigned IPA → owner signs → **disposable** world, stable 30–60-second test. No real save stress. If S1 BLOCKED: run macOS Metal + iOS compilation and build tooling tests only; do not distribute an owner IPA.
- **PASS gate:** visually identifiable, correctly oriented/UV-textured Minecraft terrain from direct Metal, video + GPU/host event evidence. Label it `NATIVE SINGLE SCENE PASS`, **not** continuous gameplay.

### S3 — First continuous native gameplay renderer (remaining available second window / stretch)
- Replace historical one-off snapshots with a real per-frame render input from the owning render client: live camera, visible sections, material/light revisions and life-cycle boundaries. Preserve owned GPU resources until completion.
- Present multiple consecutive frames at refresh cadence without wgpu presenting those terrain pixels; implement minimum needed opaque terrain, depth and basic lighting/material correctness. Can use a separate test mode until independent parity is reached.
- Start with 10–30 seconds of movement; log frame count, presentation count, CPU/GPU frame timings, stale chunks, allocations and cancellation causes.
- **PASS gate:** a continuous moving **native Metal** scene with unambiguous video evidence; native frame count and actual terrain visible. `wgpu FPS` is never used as native proof.
- **If incomplete:** commit the truthful partial code, source SHA, CI/test evidence and one precise blocker. Never claim full renderer parity.

### S4 — Deadline handoff (reserve final 2–3 hours, before subscription access ends)
- Commit tested changes, engineering ledger and owner guide. Keep successful branch build artifacts and verification logs accessible from Actions.
- Record last good SHA, nonworking branches, exact commands, tests, known hardware results, reproduction steps, next single task and unresolved risks; do not commit private Minecraft game assets, saves, credentials or signing keys.
- Complete a single status ledger with gate states `PASS / BLOCKED / NOT TESTED` and exact refs.
- **If nothing qualifies for an IPA**, say so and preserve reusable direct Metal code; do not bypass release controls.

**If sprint falls behind:** Preserve S1 security/durability, S2 native Metal hardware proof (only if safe), and S4 handoff. Cut S3 features or polish first. No time estimates presented as guaranteed delivery.

## 3. Entire roadmap to 100% Helium (phases in strict dependency order)

| Gate | Deliverable | Acceptance criteria / required evidence |
| --- | --- | --- |
| G0 P0 safe lifecycle | Nonblocking fatal-exit + Save & Quit + recoverable published writes | No event-thread joins; no false durable success; safe known exit/lifetime on supported host; deterministic crash/replay and device disposable-world quit/relaunch; honest limitation for pre-checkpoint ticks |
| G1 Native scene | Direct Metal real world terrain reaches physical iPad | Distinct native pixels in recorded video; verified GPU completion/presentation/reveal; no reliance on wgpu HUD |
| G2 Continuous terrain | Own live camera, visible sections, per-frame materials/light, native depth and presentation | Moving real world with multiple sections for 10–30s, correct UV/depth/materials, stable frame ownership; wgpu not drawing scene in native-only diagnostic mode |
| G3 Rendering parity | Opaque/cutout/translucent passes, fluid/water, entities and block entities, particles/animations, sky, weather, UI/HUD, shadows/basic light/fog | Paired screenshot and behavioral parity matrix against wgpu reference; all significant scene modes demonstrated; screen rotation, resize, app background/foreground tested |
| G4 Streaming/LOD | Native chunk uploads/eviction, occlusion and far terrain, fast travel, memory budgets | Elytra stress in disposable world, no persistent missing chunks; bounded allocation, test at different render/LOD distances; saved data verified |
| G5 Shader ecosystem | Native MSL shader pipeline, quality presets and scoped Iris/OptiFine compatibility/translation; water/atmosphere/shadows | Named shader packs verified individually; explicit unsupported effects; cold/hot compilation behavior, fallbacks and crash-safe resource retirement; no blanket “all GLSL works” claims |
| G6 Apple-focused optimization | Texture streaming, GPU upload batching, drawable/frame pacing, MetalFX or other verified upscaling if appropriate; thermal/memory handling | A16 physical benchmarks: at least 30-minute session, P50/P95 frame times/1% lows, memory warning count, thermal/battery observations; 60 FPS is a target for tested preset, not guarantee |
| G7 Remove wgpu from final iOS rendering | Full direct-Metal graphics path; cut all iOS-game wgpu render imports/dependencies, disable fallback and remove its GPU resource duplication | Static dependency/link audit, runtime instrumentation, no wgpu created/used during normal game; no regressions against complete parity suite; desktop builds unaffected unless explicitly scoped |
| G8 Launcher product quality | Apple-native iPad/iPhone launcher, Microsoft ownership/login, resource setup/downloads, packs/mods, controller/touch/accessibility, error recovery | Real device functional matrix, account recovery, download retry, import/export and settings persistence; no untested UI promises |
| G9 Release readiness | Reproducible versioned IPA and signed installation workflow; attribution/licences, privacy, support and rollback | Fresh-device and upgrade/backup test, multiplayer and world restore, crash-free long sessions, reproducible exact SHA, downloadable artifacts, release notes; beta then stable |

**G0–G3** correctness first; **G4–G6** beauty and performance; **G7** complete wgpu removal only after direct-Metal parity; **G8–G9** polished product and release. The long-range gates are **not all achievable within the 48-hour sprint**.

## 4. Quantitative acceptance (do not invent measurements)

Use the owner iPad15,7 / A16 / 6 GB; iPadOS 27.3 beta should be identified in every report; compare LiveContainer guest and standalone host results separately.
- Native renderer proof: real native frame/present counters, screen-recorded pixels and matching source/build; no offscreen completion counted as screen success.
- Performance: capture median, P95, 1% low, stalls > 50ms, draw counts, CPU/GPU memory, warnings and optional thermal state at defined presets. Aim for 60 FPS at suitable non-shader settings and viable lower-FPS shader presets; accept/reject based on observed data.
- Reliability: disposable-world Save & Quit/relaunch, 30-minute ordinary session after renderer parity, background/resume, controlled error paths, world metadata and region integrity; never provoke a watchdog on owner worlds.
- Visual: baseline comparison reference, geometry/water/sky/UI/alpha/culling and camera parity; avoid decreasing resolution merely to produce impressive FPS.
- Compatibility: Java JAR mods are **not natively executable** without a Java runtime; do not promise Fabric compatibility in a no-JVM product. Distinguish Rust-native, content/data packs and shader translation.

## 5. Operating rules for all future Codex/model sessions

1. **Read this file first**, then the latest relevant `G07-A.md` evidence and exact branch HEAD. Continue source; never reset/rewrite the project.
2. Only **one implementation assignment at a time**. After each completion inspect exact Git SHA, changed paths, CI results, crash/device evidence, acceptance states and next blocker before generating another.
3. Reserve expensive model use for **code changes and verified debugging**. Avoid repeated audits, prompt-number theater, UI polish and duplicate CI unless a source change requires it.
4. Keep tests **red first, green after** where practicable. An intentionally passing regression demonstrating a flaw is a **BLOCKED** gate, not a fix.
5. Do not promote a prototype that merely submits GPU commands. Make explicit distinctions among mock/synthetic GPU proof, real-world source capture, native physical scanout, continuous scene and complete gameplay.
6. Do not erase/shutdown-save ownership to satisfy a watchdog. No early session-lock release, data loss hidden by warnings, missing redo cleanup safety, unbounded UI joins or false success toasts.
7. Stop work on a narrow blocker at its **timebox**: publish the source-level reason, retain tests and switch to the next permitted isolated task without violating the IPA hold.
8. Branch & evidence hygiene: no code commits for mere planning if avoidable, exact source SHA in every status, repo docs survive model/session resets. Do not commit world files, commercial assets, device/user logs or signing secrets.
9. Do not silently switch the final architecture back to wgpu. **Native Metal direct API is required in the final iOS game.** Retain wgpu as a temporary comparator only until genuine parity.
10. **Changes to roadmap milestones or acceptance gates require owner approval.** State expected benefit/tradeoff. No fabricated percent-complete or claimed “100%” without tests.

## 6. Current scoreboard (update with evidence; no automatic assumptions)

| Gate | State as of 18 Nov 2026 | Evidence |
| --- | --- | --- |
| G0 ordinary save/shutdown and admitted redo replay | COMPONENT PASS / DEVICE NOT TESTED | Revision 3 report + CI 44068032930 |
| G0 fatal runtime return and LiveContainer unload | **P0 BLOCKED** | `fatal_runtime_exit_exposure_remains_a_release_blocker` |
| G1 native real Minecraft terrain physical scanout | **NOT TESTED / UNVERIFIED** | historical capture only |
| G2 continuous native scene | NOT IMPLEMENTED/NOT VERIFIED | prototype is one-section historical |
| G3–G9 | NOT ACCEPTED | phase gates above |
| Owner installable experimental IPA | **HELD; NONE APPROVED** | G07-A owner guide |

### Report template (every handoff)
```text
PLAN: docs/ios/HELIUM_MASTER_ROADMAP_2026-11-18.md v1.0
BRANCH / EXACT HEAD:
ONE OBJECTIVE:
CHANGED FILES:
TESTS: local + exact CI run:
DEVICE: build id / iPad / host / evidence / not tested:
GATE: PASS | BLOCKED | NOT TESTED
OWNER IPA: APPROVED | HELD (reason):
NEXT SINGLE IMPLEMENTATION TASK:
UNCHANGED PRODUCT GOAL: direct native Metal; no final iOS wgpu rendering.
```

**Last imperative:** Deliver one verified milestone at a time. Do not repeat the whole plan every turn; use this file as the stable source of truth.

---
## Owner-approved performance-first execution addendum — 18 Nov 2026 (v1.1)

**This addendum clarifies and strengthens G2–G7; it does not bypass G0 safeguards, replace the original gate order, or authorize a gameplay IPA.** Owner requirement: direct Metal must be deeply optimized for smoothness, low memory, visual quality, distant LOD and Apple GPU features, not merely technically functional. Remaining coding budget is approximately **two days and limited high-effort AI usage**; prioritize integrated working code over isolated features.

### New device milestone accepted
- **S1H no-world standalone diagnostic build 13001** successfully installed/launched on the owner's iPad A16/iPadOS 27.3 via their installation process, and G01/G02 actual-device Metal GPU readback reports passed.
- User screenshots visibly show the block-hill and color/depth fixture, including the separate upper native display chart. Uploaded `Helium-Host.txt` records `executableEntered → uiKitInitialized → sceneConnected → swiftLauncherAttached → metalDeviceReady → metalLayerAttached → metalWorkerEntered → metalRendererReady → metalFrameCompleted → drawableAttempted → presentSubmitted → presentCompleted`, followed by another full successful native presentation after scene reactivation.
- Accept **S1H diagnostic native Metal display / lifecycle milestone** as PASS on this particular physical iPad. This proves **synthetic GPU/display**, NOT native real Minecraft terrain, continuous gameplay, save/quit runtime on device, authentic shaders, frame rate or frame interpolation.
- S1H code-qualified CI `44239790131` at `34f775a66852a29c801a1e8dc9d18cb620d48358`; docs HEAD before addendum `286768ff375d344ff5268b9cdc5da74545503d47`. **Gameplay IPA publication remains HELD.**

### Performance charter — measurable, never magical
- Target **60 authentic simulation/rendered FPS** on A16 at a realistically selected high-clarity preset (16.67 ms presentation cadence); smooth frame pacing and 1%-low are more important than peak FPS. Seek CPU/GPU work **below** cadence with headroom (initial engineering aim ~12–14 ms per side, measured rather than guaranteed). Higher-quality shaders may need measured 30 FPS modes. Never promise literally zero stutter/crashes.
- Before claiming performance success report: native frame/present counts, median/P95/P99 CPU and GPU frame time, true rendered FPS vs generated display frames, 1% low, stalls >50/100 ms, chunk upload/eviction spikes, memory RSS/physical footprint and Metal resource residency, peak pressure and warnings, dropped or skipped frames, thermal trend, and session duration. Include exact iPad/OS/preset/build/scene.
- **Memory and ownership first:** iOS unified memory means Metal GPU allocations, decoded assets, LOD, worker queues and world state compete in the same constrained footprint. Explicit budgets for CPU meshes, staging buffers, texture atlases, live and pinned material snapshots, far LOD, in-flight frames and command completions; one owner per resource, bounded queues and automatic eviction. Use real iPad pressure telemetry/OS memory warnings; do not choose an unverified universal RAM cutoff. Use high/low watermarks, avoid double-resident wgpu+Metal assets in production, avoid cloning entire atlas per frame, shrink upload quality/LOD before termination. S1H probe's standalone memory cannot represent gameplay load.
- **Frame pacing:** decouple world simulation, meshing, streaming/upload and render-thread submission. Use asynchronous GPU uploads, completion handlers and max in-flight frame backpressure; no main-thread GPU wait, mesh generation or writer join. Cache/precompile Metal pipelines, avoid per-frame allocations and shader compilation. Handle slow chunk generation without blocking a frame.
- **Chunk & LOD:** visible frustum/face/occlusion culling when suitable for Metal/A16, near-chunk quality, demand-based priority, budgeted geometry/texture upload, distance-based far meshes/impostors, bounded far-terrain generation and cancellation, horizon fading/morph to suppress popping, lazy unload and under-pressure shrink. Implement the existing native far-terrain pipeline if suitable; no unsupported GPU-culling claims.
- **Metal hardware aware:** exploit Apple tile-based deferred rendering, avoid unnecessary attachment load/store and offscreen copies, use memoryless transient attachments when correct, minimize bandwidth, prefer batched draws and stable material binding, validate correctness and thermal impact.
- **MetalFX:** capability probe `MTLFXSpatialScalerDescriptor`, `MTLFXTemporalScalerDescriptor.supportsDevice`, `MTLFXFrameInterpolatorDescriptor.supportsDevice` / `supportsMetal4FX` and actual iPad SDK/runtime. Spatial quality fallback is feasible with scene color; temporal requires correct jitter, per-pixel motion vectors and depth; dynamic render resolution should remain quality-preserving with UI/HUD composited at native resolution. Expose sharpness/scale and compare against native pixels. Frame interpolation needs actual consecutive real frames, correct depth/motion and latency control; do not implement fake frame counting or claim it fixes simulation stalls. It is *optional*, and should be enabled only if supported, tested and better on the A16. Real rendered vs interpolated FPS must be shown separately.
- **Shader vision:** native MSL quality presets for soft sun, atmospheric weather, cloud/sky, cozy warm lighting, blue shallow/reflective water, soft rain, convincing grass droplets and fog, plus lightweight SEUS/Photon/Complementary-like *visual goals* without copying shader pack source or asserting drop-in Iris/OptiFine compatibility. Maintain distinct performance presets (fast/balanced/cinematic), offline/cached PSO compilation, graceful fallback and texture/lighting fidelity. Shader pack compatibility needs real format translation and individual tests; not granted by Metal.
- **Stability:** enforce scene/resource epoch IDs, immutable snapshot lifetimes, completed command buffers before resource release, worker cancellation and native host ownership; preserve original world saving and recovery gates. Test fast movement/Elytra, background/foreground, thermal and repeated world entry on disposable saves only.

### AI-reset execution budget and mandatory gate ordering
1. **Next Codex S2A only:** **real Minecraft section → authentic material/light/camera → actual direct-Metal presentation path**. At the same time add *minimal real frame-time, memory/upload counters and bounded queue protections* needed to prevent designing performance debt; do not bolt on a giant profiling subsystem. No new physical gameplay IPA without applicable G0 safety release.
2. **Following S2B only after inspected S2A evidence:** move from one-off snapshot to multi-section live camera/continuous frames with in-flight backpressure, resource budgets, streaming and eviction. Target authentic frame-time and memory measurements first. For an AI-constrained schedule, **S2B is more valuable than jumping to advanced shaders/MetalFX immediately**.
3. **Remaining available time:** if S2B is device/host blocked, qualify isolated Apple GPU/CI, preserve source and measured blockers, and produce exact handoff docs. Do not spend remaining coding resets on endless LiveContainer investigation, speculative frame generation, cosmetic UI work or re-running green tests without code changes.
4. After authentic continuous Metal exists, optimize geometry/streaming/LOD; next integrate quality-preserving MetalFX; then shaders/lighting and optional frame interpolation. Optimizations may be implemented earlier only when they unblock or measurably improve the currently active gate.
5. **Keep the master one-prompt/one-review workflow:** Codex delivers source + exact CI + evidence; independent review determines the next prompt. No automatic leap to S2B or G5. The existing general gameplay IPA hold remains in effect.

**Success means proven gameplay smoothness, frame-time stability, bounded memory, visual fidelity and reliable saves—not rendering a test triangle, reporting synthetic GPU FPS or superficially turning on a feature.**

---
## Owner-approved final-session directive — 20 November 2026
### COMPLETE THE NATIVE-METAL GAMEPLAY CUTOVER BEFORE OPTIONAL FEATURES

**Owner change request:** The remaining paid-AI coding window is to be spent on making Minecraft genuinely run on Helium's own direct-Metal backend, **not** on more isolated synthetic graphics tests, a persistent wgpu overlay, optional shader packs, MetalFX or visual polish. Preserve the earlier G0 world/save release hold and release qualifications. Following the paid session, Gemini or other AI may continue enhancement work from a reproducible GitHub handoff.

**Definition of the target, not a claim of current status:** A playable iOS game session where real Minecraft world loading/simulation, textured near terrain, camera, input, indispensable overlays/menus and enough core gameplay visuals run with **no wgpu or WebGPU GPU object/device/surface/submission**, using Rust scene data and Helium-owned direct MTLDevice/MTLCommandQueue/MTLRenderPipelineState/MTLBuffer/MTLTexture/CAMetalLayer. Ordinary world and multiplayer code may be reused. A missing rendering pass that makes normal gameplay effectively blind or impossible is a blocker; no hidden wgpu fallback is allowed in a native-only build. A native UIKit/SwiftUI interface is acceptable; not everything needs to be drawn in Metal if there is no wgpu fallback.

**Acceptance gates:**
1. Actual production world and resource loading run through backend-independent game/scene producers without instantiating a wgpu Renderer, queue or adapter.
2. The native Metal path owns the entire active gameplay display; demonstrate continuous real-world frames and moving camera on actual Apple hardware, not just synthetic CI.
3. Required visible passes for a usable play session include near opaque/cutout terrain, transparent fluids, entities/player-relevant visuals, sky and essential HUD/menus. Track additional missing/parity effects explicitly. Keep shaders, advanced LOD, MetalFX and frame generation out of the critical cutover unless they are required to unblock this core.
4. Native-only configuration fails closed on Metal initialization; no hidden wgpu/graphics fallback; audit compiled iOS dependency graph, linked executable, runtime instrumentation and actual native presents. Full game is **not complete** if only the isolated no-world example is wgpu-free.
5. Preserve chunk geometry cache, bounded CPU and GPU residency, scene/world epoch invalidation, latest-only frame input, resource retirement after GPU completion, async texture uploads, budget accounting, and no blocking UI operations. Test frame-time percentiles and real device memory/thermal trends where available; never promise impossible zero stutter/crash.
6. Protect save/journal/host lifecycle. Do not publish an ordinary owner gameplay IPA until the G0 safety gate and disposable-world reopen are independently qualified. A game compiled in CI is not a physically verified safe playable game.
7. Commit and push coherent checkpoints before usage expires, require exact-SHA macOS Metal + iOS ARM64 qualification, and produce a durable `docs/ios/HELIUM_GEMINI_HANDOFF.md` with branch/commits, exact build steps, current native-only status, working/prohibited operations, limitations, missing passes, verified tests and physical device requirements.

**Priority order for time-limited coding:** (i) real native-only game boot/world/render frame; (ii) required scene passes and input/UI; (iii) stability, streaming, memory, GPU frame pacing; (iv) exact CI/device validation; (v) immutable Gemini handoff. If time expires, **report partial honestly** and prioritize a compilable, testable native-only gameplay vertical slice over unintegrated subsystem breadth. Full correctness/performance cannot be certified without running the actual game on the actual iPad.

**Supersedes the earlier prompt-specific S2A/S2B stop rules for the final paid session:** Codex may implement as many tightly related cutover steps as can be coherently validated in one execution. It does **not** supersede world-save release controls or the requirement to clearly distinguish implementation, CI, physical verification and product completion.
