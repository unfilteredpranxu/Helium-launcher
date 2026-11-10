//! G01: a blocking, one-shot offscreen probe, never called by the game.
//! MIT; see the repository's LICENSE and preserved Horizon attribution.
mod scene;
pub use scene::{Mesh, Vertex, fixture, fixture_camera};

pub mod section;
pub mod terrain;

#[cfg(any(target_os = "macos", target_os = "ios", test))]
mod ffi;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod metal;

pub const SIDE: usize = 256;

#[derive(Debug)]
pub struct Image {
    /// Tightly packed RGBA8Unorm after completion. G01 uses SIDE square;
    /// the terrain renderer uses the caller's Options width/height.
    pub rgba: Vec<u8>,
    /// Entire render+blit command buffer; None if Metal supplies no valid timestamps.
    pub gpu_seconds: Option<f64>,
}

/// Render on a worker/test thread, not UIKit's main thread. This waits for the GPU.
/// Matrices are column-major; depth is ordinary Metal [0,1], NOT reversed-Z.
pub fn render_offscreen(mesh: &Mesh, clip_from_world: [[f32; 4]; 4]) -> Result<Image, String> {
    mesh.validate()?;
    if !clip_from_world.iter().flatten().all(|x| x.is_finite()) {
        return Err("non-finite camera matrix".into());
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    return metal::render(mesh, clip_from_world);
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    Err("direct Metal requires macOS or iOS".into())
}

/// Apple-only execution oracle: nonempty terrain, black background, opaque pixels,
/// and identical output when triangle order is reversed (depth ordering).
pub fn smoke_test() -> Result<Image, String> {
    let mut mesh = fixture();
    let image = render_offscreen(&mesh, fixture_camera())?;
    let colored = image
        .rgba
        .chunks_exact(4)
        .filter(|p| p[..3] != [0, 0, 0])
        .count();
    if !(100..SIDE * SIDE).contains(&colored) || image.rgba.chunks_exact(4).any(|p| p[3] != 255) {
        return Err("offscreen result lacks opaque terrain or clear background".into());
    }
    mesh.indices = mesh
        .indices
        .chunks_exact(3)
        .rev()
        .flatten()
        .copied()
        .collect();
    let reversed = render_offscreen(&mesh, fixture_camera())?;
    if image.rgba != reversed.rgba {
        return Err("depth check failed: triangle order changed the image".into());
    }
    Ok(image)
}

/// Optional standalone Apple test-app entry; call on a worker, never UIKit's main thread.
/// No pointer arguments, no borrowed resources, and Rust panics do not cross the C ABI.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[unsafe(no_mangle)]
pub extern "C" fn helium_metal_smoke_test() -> i32 {
    match std::panic::catch_unwind(smoke_test) {
        Ok(Ok(image)) => {
            eprintln!(
                "metal-prototype: smoke PASS; GPU render+blit seconds={:?}",
                image.gpu_seconds
            );
            0
        }
        Ok(Err(error)) => {
            eprintln!("metal-prototype: smoke FAIL: {error}");
            1
        }
        Err(_) => {
            eprintln!("metal-prototype: smoke FAIL: Rust panic");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn non_finite_camera_is_rejected_before_metal() {
        let mut camera = super::fixture_camera();
        camera[0][0] = f32::NAN;
        assert_eq!(
            super::render_offscreen(&super::fixture(), camera).unwrap_err(),
            "non-finite camera matrix"
        );
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    #[test]
    fn unsupported_host_returns_an_error_instead_of_a_fake_render() {
        assert_eq!(
            super::smoke_test().unwrap_err(),
            "direct Metal requires macOS or iOS"
        );
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "ios")))]
mod apple_tests {
    #[test]
    #[ignore = "requires a real Metal device; invoke explicitly on Apple hardware"]
    fn indexed_terrain_and_depth_readback() {
        super::smoke_test().expect("Metal offscreen terrain smoke test");
    }
}
