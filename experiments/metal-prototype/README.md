# G01: direct Metal offscreen terrain probe

G02 adds a separate [production-format terrain path and engineering assessment](docs/G02-A.md).
The G01 draw, shader and smoke checks remain unchanged. The standalone
[iOS probe](ios/README.md) now selects G01 (default) or G02 with separate results;
the production launcher is untouched.

Experimental, opt-in, and not wired into Helium. Independent Cargo workspace/lockfile;
normal game builds neither resolve nor compile it. Only macOS/iOS compile `metal.rs`
or depend on objc2. Linux builds test the fixture and validation, and return an explicit
unsupported-platform error if asked to render. No wgpu, Vulkan, OpenGL, game, or world dependencies.
The repository's MIT license and Horizon copyright/attribution remain unchanged.

## What runs

`render_offscreen` takes owned-format mesh slices via `Mesh` and a column-major clip matrix.
It uses objc2-metal **0.3.2** (Rust minimum 1.71; this package requires 1.95), objc2 0.6.5,
and Foundation 0.3.2. Features are limited to the classic Metal device, resources, encoding,
and timestamp APIs. No Metal 4, mesh shaders, ray tracing, or GPU-family requirement.

The fixture is a three-by-three stepped block hill: 46 exposed quads, 184 compact
16-byte vertices, and 276 uint32 indices. The vertex MSL expands short positions,
normalized ushort UVs and RGBA8 tint. An original 2x2 RGBA8 checker supplies texels.
`terrain.metal` is compiled by Metal at runtime; no Minecraft assets are distributed.

One render pass clears a 256x256 RGBA8Unorm color texture and Depth32Float texture,
draws indexed triangles with depth writes and `Less`, then blits to a shared CPU buffer.
`commit`/`waitUntilCompleted` and command status/error checks precede readback. Optional
GPU timestamps cover render **and blit**, exclude pipeline creation, and are diagnostics,
not FPS or an A16 benchmark. `smoke_test` renders twice, reverses triangle order, checks
identical pixels, and requires both non-black terrain and opaque black background.

Resources use `Retained`, copied uploads, normal retaining command buffers, and an
autorelease pool. All CPU owners live through completion; no raw ownership transfers or
unretained command references. This deliberately blocking probe belongs on a worker thread.
It has no timeout; a stalled driver requires terminating the standalone test app.

## Integration boundary (identified, not implemented)

- `crates/horizon-client-render/src/mesher.rs`: `TerrainVertex` (24 bytes), `SectionMesh`
  solid/translucent quads and packed records. Their layout differs from this 16-byte fixture.
  A future explicit adapter can consume CPU meshes; do not reinterpret them or quantize
  non-grid model positions silently. No production mesher or simulation was copied here.
- `gfx/view.rs`: `View::view_proj`, column-major matrices; production uses **reversed-Z**.
  This probe uses ordinary [0,1] depth. Reuse would require explicit depth-state/clear changes
  and camera-relative section transforms, not a blind matrix substitution.
- Block atlases in `gfx.rs`/`horizon-assets` already provide decoded RGBA texels, UVs and
  layers. A later adapter must preserve layer, tint, light and sampling/color-space rules.
- `gfx/timing.rs`, `spans.rs`, and `horizon-bench` are timing/reporting boundaries. Their
  wgpu queries cannot be reused directly; command-buffer timestamps can feed a later adapter.

## Validation and execution commands

From the repository root, with Rust 1.95.0 activated:

```sh
cargo +1.95.0 fmt --manifest-path experiments/metal-prototype/Cargo.toml -- --check
cargo +1.95.0 metadata --locked --no-deps --format-version 1 --manifest-path experiments/metal-prototype/Cargo.toml
cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml
cargo +1.95.0 check --locked --all-targets --target aarch64-apple-ios --manifest-path experiments/metal-prototype/Cargo.toml
```

On a Metal-capable Mac with Xcode command-line tools (NOT RUN in Linux):

```sh
MTL_DEBUG_LAYER=1 cargo +1.95.0 run --locked --manifest-path experiments/metal-prototype/Cargo.toml --example offscreen -- /tmp/helium-g01.ppm
cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml -- --ignored
xcrun -sdk iphoneos metal -std=ios-metal2.0 -c experiments/metal-prototype/src/terrain.metal -o /tmp/helium-g01.air
cargo +1.95.0 build --release --locked --target aarch64-apple-ios --manifest-path experiments/metal-prototype/Cargo.toml --lib
```

The example creates a new PPM file and refuses to overwrite an existing one. The separate
[G01-C iOS test app](ios/README.md) provides a SwiftUI host, caller-owned RGBA output bridge,
and unsigned IPA packaging instructions. It does not link or launch Minecraft. The original
status-only `helium_metal_smoke_test()` remains available unchanged. Xcode linking/signing,
MSL compilation, GPU execution and A16 correctness are NOT established by Rust type checks.
Objective-C exceptions/device removal are not caught by the Rust panic boundary.

Rollback: leave this workspace unreferenced and return to the normal launcher build.
No production source/configuration needs reverting. Preserve any local changes
before switching branches; do not use reset/clean. Next action after owner review: execute the standalone
smoke test on an Apple GPU and capture its status, validation output and readback image.
