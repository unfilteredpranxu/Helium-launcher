# G01-C: standalone iOS Metal Probe

MIT; the repository LICENSE, Horizon notices and upstream attribution remain intact.
This app is independent of `packaging/ios`, the game workspace and its launcher.
Bundle identifier: **dev.helium.metalprobe**. Physical arm64 iPhone/iPad, iOS 15+;
Simulator is deliberately unsupported. No game resources, worlds, accounts, network
requests, analytics, telemetry or permission prompts are used.

## Architecture and buffer contract

`MetalProbeApp.swift` owns one SwiftUI screen and main-actor UI state. Run is disabled
while a test is active. A dedicated serial Dispatch queue invokes `ProbeRunner.run()`;
that function also rejects accidental main-thread calls. Results return on the main
queue. The existing blocking GPU wait has no timeout or cancellation. A driver stall
can leave Running displayed; close this standalone app rather than launching more work.

The added C entry in `../helium_metal_prototype.h` is:

```c
int32_t helium_metal_smoke_test_rgba(uint8_t *rgba_out, size_t rgba_capacity,
                                     char *message_out, size_t message_capacity);
```

Both buffers belong exclusively to the caller and must remain valid, writable and
disjoint throughout the call. Rust retains neither pointer. Pixel capacity must be
at least 262144 bytes; message capacity at least 1 (the app supplies 4096). Null,
short, overlapping and overflowing ranges are rejected before rendering. C callers
remain responsible for allocation validity; pointer checks cannot detect dangling
storage. See the header for exact failure-write rules.

The entry calls the **unchanged** `smoke_test`: two real renders, with triangle order
reversed for the second. Only success copies the first image into caller memory,
256x256 RGBA8Unorm, 1024 bytes/row, Metal texture row 0 first. Pixel output is untouched
on failure. The UTF-8 message is always NUL-terminated when the message allocation is
valid and disjoint, with truncation at a character boundary. Status codes:

| Code | Meaning |
|---|---|
| 0 | Existing GPU smoke checks passed; actual pixels copied |
| 1 | Renderer, MSL, command-buffer or smoke-check failure |
| 2 | Caught unwinding Rust panic |
| 3 | Invalid output arguments |

The original status-only entry is preserved. No borrowed Rust data crosses either
ABI. Panics in rendering/result preparation are caught; Objective-C exceptions and
process aborts are not. No production renderer code, Metal shader or dependency changes
are required.

After Rust returns, the worker makes an immutable `Data` copy of the caller's pixels
for `CGDataProvider`; `CGImage` retains that provider. RGBA byte order, 1024-byte row
stride and linear-sRGB color interpretation match the offscreen UNorm shader output.
Alpha is 255 by the existing smoke check, so premultiplied alpha does not change bytes.
The image has no vertical flip, replacement geometry or predetermined pixel result.
The UI displays it without interpolation. Image-construction failures display FAIL.
The share sheet exports the diagnostics text and, on success, the actual image.

PASS is a smoke result. It checks non-black coverage, opaque alpha, a black background
and triangle-order invariance; it has no independent expected-color/shape oracle.
Partial geometry or consistently wrong rendering can still pass. Inspect the stepped
3x3 hill (height rows `1 2 1 / 2 3 2 / 1 2 1`), green tops, brown sides and checker texels
for missing geometry and occlusion/color errors. Preserve the image with the report.

An available GPU duration describes the **first single offscreen render+blit command
buffer** only. It excludes compilation and does not include the second render. It is
not gameplay FPS. Completed GPU readback does not prove physical display presentation.

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
# Compile the real shader separately: Rust type-checking cannot validate MSL.
xcrun --sdk iphoneos metal -std=ios-metal2.0 -c ../src/terrain.metal -o build/terrain.air
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
release archive. Apple linking is still unverified; if necessary inspect the actual
native link requirements on the Mac with:

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

## Device acceptance (not yet run)

1. Confirm app installation beside Helium and a responsive Run/Running screen.
2. Run on the A16, capture console/validation errors and the in-app diagnostic status.
3. Inspect and share the actual hill image, checking color, geometry and depth ordering.
4. Run again and verify single-run exclusion, UI updates, image ownership and sharing
   on both iPhone and iPad. Verify meaningful FAIL reporting if an actual error occurs.
5. Record MSL compilation, native linking and GPU execution separately. A Linux test
   or `aarch64-apple-ios` Rust check establishes none of those outcomes.

Rollback: leave this directory unreferenced and build the normal Helium launcher.
The new ABI is additive; the old entry, G01 draw, smoke checks and production sources
are unchanged. No CI workflow was added and no build runs automatically from GitHub.
