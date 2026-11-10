# G02-C: standalone dual-renderer iOS Metal Probe

MIT; the repository LICENSE, Horizon notices and upstream attribution remain intact.
This app is independent of `packaging/ios`, the game workspace and its launcher.
Bundle identifier: **dev.helium.metalprobe**. Physical arm64 iPhone/iPad, iOS 15+;
Simulator is deliberately unsupported. No game resources, worlds, accounts, network
requests, analytics, telemetry or permission prompts are used.

## Architecture and buffer contract

`MetalProbeApp.swift` owns one SwiftUI screen and main-actor UI state. The selector
defaults to **G01 — Original Direct Metal** (the 16-byte stepped hill). **G02 — Production
Terrain** runs the separate 24-byte terrain/material fixture and full-image oracle.
Each mode has its own Not run / Running / PASS / FAIL result, diagnostics and actual
GPU image for the current app session. A previous device result never prepopulates PASS.

Run and selection are disabled while either test is active, with matching model guards.
A dedicated serial Dispatch queue invokes `ProbeRunner.run(mode:)`; that function also
rejects accidental main-thread calls. The immutable mode is captured before dispatch,
carried by every result and used to store completion on the main queue. Switching modes
after completion restores only that mode's result. Rerunning hides its previous image;
a failure replaces it with diagnostics and no image. Sharing snapshots the selected
mode's matching report/image, so the exported image cannot acquire another mode's label.
The existing blocking GPU wait has no timeout or cancellation. A driver stall
can leave Running displayed; close this standalone app rather than launching more work.

The unchanged C entries in `../helium_metal_prototype.h` are:

```c
int32_t helium_metal_smoke_test_rgba(uint8_t *rgba_out, size_t rgba_capacity,
                                     char *message_out, size_t message_capacity);
int32_t helium_metal_terrain_test_rgba(uint8_t *rgba_out, size_t rgba_capacity,
                                     char *message_out, size_t message_capacity);
```

Both buffers belong exclusively to the caller and must remain valid, writable and
disjoint throughout the call. Rust retains neither pointer. Pixel capacity must be
at least 262144 bytes; message capacity at least 1 (the app supplies 4096). Null,
short, overlapping and overflowing ranges are rejected before rendering. C callers
remain responsible for allocation validity; pointer checks cannot detect dangling
storage. See the header for exact failure-write rules.

G01 calls the **unchanged** `smoke_test`: two real renders, with triangle order reversed
for the second. G02 calls the **unchanged** `terrain::smoke_test`: normal and reversed
triangle order with reversed-Z, followed by equivalent forward-depth rendering. All
three images must pass the independent oracle; the first two must also match exactly.
Every image's 65536 pixels are compared (RGB tolerance 2 UNorm codes, alpha exact).
The fixture covers texture-array layers, UV wrapping, lighting/tint, alpha cutout,
opaque overrides, camera translation and overlapping geometry/depth. It remains a
synthetic fixture, not proof of general Minecraft rendering or perspective correctness.

For either entry, only complete success copies the first actual GPU image into caller memory,
256x256 RGBA8Unorm, 1024 bytes/row, Metal texture row 0 first. Pixel output is untouched
on failure. The UTF-8 message is always NUL-terminated when the message allocation is
valid and disjoint, with truncation at a character boundary. Status codes:

| Code | Meaning |
|---|---|
| 0 | All selected test's GPU checks passed; actual pixels copied |
| 1 | Renderer, MSL, command-buffer or smoke-check failure |
| 2 | Caught unwinding Rust panic |
| 3 | Invalid output arguments |

The original status-only entry is preserved. No borrowed Rust data crosses any
ABI. Panics in rendering/result preparation are caught; Objective-C exceptions and
process aborts are not. No production renderer code, Metal shader or dependency changes
are required.

After Rust returns, the worker makes an immutable `Data` copy of the caller's pixels
for `CGDataProvider`; `CGImage` retains that provider. RGBA byte order, 1024-byte row
stride and linear-sRGB color interpretation match the offscreen UNorm shader output.
Alpha is 255 by the selected test's checks, so premultiplied alpha does not change bytes.
The image has no vertical flip, replacement geometry or predetermined pixel result.
The UI displays it without interpolation. Image-construction failures display FAIL.
The share sheet exports the diagnostics text and, on success, the actual image.

**G01 PASS** is a smoke result. It checks non-black coverage, opaque alpha, a black background
and triangle-order invariance; it has no independent expected-color/shape oracle.
Partial geometry or consistently wrong rendering can still pass. Inspect the stepped
3x3 hill (height rows `1 2 1 / 2 3 2 / 1 2 1`), green tops, brown sides and checker texels
for missing geometry and occlusion/color errors. **G02 PASS** additionally requires
every oracle/depth/order check above; errors propagate through the C status to FAIL.
Neither entry substitutes the CPU reference image for GPU readback. Preserve the
mode-labelled image with its report, including any failed run's error text.

An available GPU duration describes the **first single offscreen command buffer**:
G01 render + readback, G02 uploads + render + readback. It excludes pipeline compilation
and the other validation renders. The workloads differ; these durations do not establish
relative renderer performance or gameplay FPS. Completed GPU readback does not prove
physical display presentation.

## macOS prerequisites and exact build commands

These commands are instructions for owner-approved Apple validation; they were NOT
executed in the Linux environment. Use Xcode with its iPhoneOS SDK and device support,
Xcode command-line tools, XcodeGen, Rust 1.95.0 and its `aarch64-apple-ios` standard library.
An Apple signing team/provisioning profile is needed for physical installation.
The generated project uses the Xcode 15.3 project format and Swift 5 language mode.

Install missing components on the Mac only (skip those already installed):

```sh
brew install xcodegen
rustup toolchain install 1.95.0 --profile minimal --component rustfmt
rustup target add --toolchain 1.95.0 aarch64-apple-ios
xcode-select -p
xcrun --sdk iphoneos --show-sdk-path
```

From the repository root:

```sh
cd experiments/metal-prototype/ios
mkdir -p build
xcodegen generate --spec project.yml
# Compile both real shaders separately: Rust type-checking cannot validate MSL.
xcrun --sdk iphoneos metal -std=ios-metal2.0 -c ../src/terrain.metal -o build/g01-terrain.air
xcrun --sdk iphoneos metal -std=ios-metal2.0 -c ../src/terrain/terrain24.metal -o build/g02-terrain.air
# Builds only this test app and its experimental static library, then packages it.
./build-unsigned.sh
```

`build-unsigned.sh` invokes:

```sh
xcodebuild -project HeliumMetalProbe.xcodeproj -scheme HeliumMetalProbe \
  -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
  -derivedDataPath "$PWD/build/DerivedData" \
  CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY='' build
```

The app's pre-build phase runs `build-rust.sh`, which verifies the already-installed
1.95.0 toolchain/target (no automatic rustup installation) and executes:

```sh
rustup run 1.95.0 cargo build --locked --release --lib \
  --target aarch64-apple-ios --manifest-path "$PWD/../Cargo.toml"
```

Cargo output stays in the experiment's `target/`. The project links that static
archive, Metal, UIKit, SwiftUI, Foundation, CoreFoundation, CoreGraphics and native
`iconv`, `System`, `objc` libraries. All Rust build configurations use this isolated
release archive. G01 previously built and executed on the owner's A16; this dual-mode
revision and G02's shader/runtime still require Apple validation. If necessary inspect
the actual native link requirements on the Mac with:

```sh
cargo +1.95.0 rustc --locked --release --target aarch64-apple-ios \
  --manifest-path ../Cargo.toml --lib -- --print native-static-libs
```

The resulting `build/HeliumMetalProbe-unsigned.ipa` has a `Payload/HeliumMetalProbe.app`
layout. An unsigned IPA requires signing/provisioning or a sideloading tool that signs
it before stock iOS can install it. Its distinct bundle identifier permits coexistence
with Helium. The packaging script uses a fresh staging directory; generated projects,
archives and build/staging directories are ignored and must not be committed.

For a signed device run, open `HeliumMetalProbe.xcodeproj`, select this app's Signing &
Capabilities tab, choose your team with automatic signing, select the physical device
and run the **HeliumMetalProbe** scheme. Enable Metal API Validation in scheme Diagnostics;
the scheme also sets `MTL_DEBUG_LAYER=1`. No production project settings need changing.

## Dual-mode device acceptance (NOT RUN for G02-C)

1. Confirm installation beside Helium, iOS 15 deployment, default G01 selection and two
   independent Not run statuses. Run G01 and verify the original hill and smoke result.
2. Select G02: G01's image/PASS must not appear as G02's. Run, capture console/Metal API
   Validation errors and verify all oracle checks succeed before the real chart appears.
3. Switch between completed modes; verify each image, report, status and share-sheet export
   has the correct renderer label. Test sharing on both iPhone and iPad.
4. Rerun; selection and repeated Run taps must not start simultaneous work. Verify scrolling
   stays responsive and old images are hidden while running. If an actual error occurs,
   FAIL/error text must remain visible with no stale success image for that mode.
5. Repeat separately on A11/iPhone X and A15 when available; an A16 result proves neither.
   No device-name allowlist or new family requirement is introduced; existing runtime
   capability checks remain in Rust. iOS deployment stays **15.0**.
6. Record Swift/MSL compilation, native linking and GPU execution separately. A Linux test
   or `aarch64-apple-ios` Rust check establishes none of those outcomes.

Rollback: leave this directory unreferenced and build the normal Helium launcher.
Both C ABIs, G01 draw, G02 oracle and production sources are unchanged by G02-C.
The existing `.github/workflows/helium-metal-probe.yml` compiles both shaders with
`-std=ios-metal2.0`, with separate step outcomes/logs; a G02 compiler error fails the job.
Its restricted experimental-branch/path push trigger, unsigned IPA artifact and read-only
permissions are preserved. This assignment does not publish, start CI or build an IPA;
a future qualifying push can trigger that workflow. Apple MSL/Swift compilation and
G02 GPU execution remain NOT RUN in this Linux workspace.
