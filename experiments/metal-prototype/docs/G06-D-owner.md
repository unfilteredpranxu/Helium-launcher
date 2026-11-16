# G06-D one-session owner check — Prompt 11/25

Use only the qualified `Helium-G06D-instrumented-game-unsigned` Actions artifact.
It is the actual Rust game, not Metal Probe. No Minecraft resources are bundled.

1. Before upgrading, back up `resources`, `saves`, settings and `helium-metal-test.txt` from the existing test app's Documents. The bundle remains `dev.helium.g06ctest` / **Helium G06C Test** so the Files name and imports stay familiar. An upgrade with the same signer may preserve Documents; a signer or tool that uninstalls first can erase it. Do not uninstall without a backup.
2. Sign with your trusted method and install. Open **Helium G06C Test**. Build **11002** has the G06-D HUD and isolated diagnostics marker v2. No new configuration file is required. A pre-existing file containing exactly `off` (optionally a newline) still disables attempts; remove it to restore isolated auto-arm. Malformed files fail closed and the HUD says INVALID. Normal game builds have no marker and remain unchanged.
3. Open the existing disposable world with the owner's already imported Java 26.3 resources. Look toward nearby solid terrain for 30–60 seconds. Keep the app foreground, launcher dismissed and orientation stable during an attempt. The independent HUD persists even when native output is hidden or cancelled. Up to three fresh, sequential attempts can occur, subject to current producer authority and safe retirement; no fresh accepted section means no retry.
4. In Files, open **On My iPad → Helium G06C Test → logs → Helium-Metal-Report.txt** and Share that ONE file back. The JSONL beside it is optional forensic detail. No Mac, console or manual log splicing is needed. If the app exits unexpectedly, reopen once before sharing; the report preserves normalized prior-boot checkpoints and marks incompleteness without inventing a crash cause.
5. A screenshot/recording during `NATIVE REVEAL: YES` is optional visual corroboration. GPU completion or UIKit reveal does not prove scanout. Only the owner can confirm visible authentic native terrain. Ordinary wgpu terrain remains the fallback and is not a native success.

The HUD reports exact first causes, bank progress, generation/light revision, recorder acknowledgement, losses/degradation and fallback. `TEST_ONLY` CI reports are synthetic infrastructure tests and are never physical-device evidence. If recorder storage fails, the HUD says DEGRADED; do not regard an older disk report as a complete timeline.

Prepared resources remain private and are neither committed nor uploaded by this diagnostic. Existing imports/worlds are reused; the diagnostic does not alter saves, artwork, shader settings or geometry. No continuous direct-Metal gameplay/FPS claim.

Qualified build: source `f709821676d4fc2c0ccf9f9b49ce40d0fc1a0aab`,
[green CI run](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/43927386632),
[one final unsigned-game artifact](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/43927386632/artifacts/14260916522).
The artifact includes `verified.log`; the unsigned executable SHA-256 is
`f495c89bbfe60709a79796a1e1d331bdc71b370e1f4fffa4df20a148abe425f2`.
Physical native terrain and the original first cause remain pending your run.
