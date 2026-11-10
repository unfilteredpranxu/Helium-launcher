# G02-E: first Apple build of the dual-mode Metal Probe

MIT; preserve the repository LICENSE, Horizon notices and upstream attribution.

## Scope and source identity

This document intentionally triggers one push build of the standalone probe on
`feature/helium-g01-metal-prototype`. The reviewed implementation baseline is
`a65ab42d1022ac4402e9dec924e9fc34fde0019b`; the triggering commit adds only this
document. The workflow checks out that exact triggering SHA. No production
Minecraft client, launcher, world, account or gameplay code is part of this build.

Build outcomes are pending when this document is committed. Record the run URL,
triggering SHA and actual logs in the task report. Do not retry or repair a failed
build during G02-E. Compilation does not establish Metal GPU correctness.

## Planned Apple validation

Use the existing `.github/workflows/helium-metal-probe.yml`, one `macos-26` job
with its 40-minute timeout and read-only repository permissions. Record the actual
runner image, Xcode build and iPhoneOS SDK versions from the logs.

1. Compile G01 `src/terrain.metal` and G02 `src/terrain/terrain24.metal` separately
   with the real iPhoneOS Metal compiler and `-std=ios-metal2.0`. Each compilation
   must succeed and retain its own diagnostic log.
2. Build the independent Rust static library with Rust 1.95.0 for
   `aarch64-apple-ios`. Preserve the experimental lockfile and production workspace.
3. Generate `ios/HeliumMetalProbe.xcodeproj` with XcodeGen. Build the SwiftUI host
   and link the actual Rust archive and Apple frameworks using the existing scripts.
4. Package only the unsigned standalone IPA. Inspect compiler/linker warnings and
   errors, the archive CRCs, app structure, Info.plist and ARM64 executable.
5. Verify artifact upload and record the IPA byte size, SHA-256, artifact ID and
   exact run URL. Keep the build-log artifact, including failure evidence.

Expected output identity:

| Item | Required value |
|---|---|
| IPA | `experiments/metal-prototype/ios/build/HeliumMetalProbe-unsigned.ipa` |
| Uploaded IPA artifact | `helium-metal-probe-unsigned` |
| Build-log artifact | `helium-metal-probe-build-logs` |
| App directory inside IPA | `Payload/HeliumMetalProbe.app/` |
| Bundle identifier | `dev.helium.metalprobe` |
| Executable | `HeliumMetalProbe`, ARM64 |
| Minimum iOS deployment | `15.0` |

The IPA is unsigned. Physical installation requires appropriate signing and
provisioning or an owner-selected signing/sideloading tool. Do not publish a release
or replace the production launcher. The distinct bundle identifier permits both
apps to coexist. An iOS 15 deployment target alone does not prove A11 compatibility.

## Physical-device acceptance after build review

**G02 physical GPU execution: NOT RUN.** G01 previously passed on the owner's iPad
A16. Neither that result nor this build establishes G02, A11 or A15 correctness.
Do not execute these device steps as part of the CI build.

1. After owner review, sign/install the standalone app beside Helium on the A16.
   Record device, OS, build SHA and any Metal API Validation messages. Do not open
   Minecraft assets, player worlds or accounts.
2. Confirm **G01 — Original Direct Metal** is the default and both current-session
   statuses initially read Not run. Run G01; inspect the real 16-byte stepped-hill
   GPU image, result and error diagnostics. Preserve the report and image.
3. Select **G02 — Production Terrain**. G01's PASS or hill must not be relabelled
   as G02. Run the 24-byte fixture with texture arrays, lighting/tint, cutout,
   opaque overrides and overlapping depth geometry.
4. Require the unchanged G02 full-image oracle to pass all three GPU renders:
   normal/reversed triangle order under reversed-Z and equivalent forward depth.
   Check every pixel with the existing RGB tolerance and exact alpha; normal and
   reversed triangle-order images must also match exactly. The displayed image
   must be the actual first GPU readback, never a CPU reference substitution.
5. Confirm the UI remains responsive, selection/repeated Run taps cannot overlap
   GPU work, and each mode retains its own status, image and report. Rerunning must
   hide its old image; a failure must show FAIL and meaningful diagnostics without
   a stale success image. Verify matching diagnostics/image sharing on iPad/iPhone.
6. Preserve the result exports and any failures for review. Label timing as one
   offscreen command-buffer measurement; G02 includes uploads, rendering and
   readback. It is not gameplay FPS or physical display-presentation completion.
7. Repeat separately on iPhone X/A11 and A15 devices when available. Keep their
   compatibility UNVERIFIED until those physical tests execute successfully.

Stop after reporting the single build attempt. No automatic retry, renderer
integration, optimization or next milestone is authorized by this document.
