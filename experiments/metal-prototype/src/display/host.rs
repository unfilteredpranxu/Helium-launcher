//! Dedicated-worker diagnostic driver, not a gameplay scheduling API.
use crate::{
    frame,
    terrain::{Options, TerrainRenderer, validation},
    upload::{self, Description, Identity, Source},
};
use objc2::{ClassType, rc::Retained, runtime::AnyObject};
use objc2_quartz_core::CAMetalLayer;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
};
struct Token {
    cancelled: AtomicBool,
    status: AtomicI32,
    running: AtomicBool,
}
static TOKEN: Mutex<Option<(u64, Arc<Token>)>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);
fn token(id: u64) -> Option<Arc<Token>> {
    TOKEN
        .lock()
        .ok()?
        .as_ref()
        .filter(|t| t.0 == id)
        .map(|t| t.1.clone())
}
#[unsafe(no_mangle)]
pub extern "C" fn helium_display_create() -> u64 {
    let Ok(mut slot) = TOKEN.lock() else { return 0 };
    if slot.is_some() {
        return 0;
    }
    let Ok(id) = NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
    else {
        return 0;
    };
    *slot = Some((
        id,
        Arc::new(Token {
            cancelled: AtomicBool::new(false),
            status: AtomicI32::new(1),
            running: AtomicBool::new(false),
        }),
    ));
    id
}
#[unsafe(no_mangle)]
pub extern "C" fn helium_display_status(id: u64) -> i32 {
    token(id).map_or(-1, |t| t.status.load(Ordering::Acquire))
}
#[unsafe(no_mangle)]
pub extern "C" fn helium_display_cancel(id: u64) {
    if let Some(t) = token(id) {
        t.cancelled.store(true, Ordering::Release);
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn helium_display_release(id: u64) {
    if let Ok(mut slot) = TOKEN.lock() {
        if let Some((key, t)) = slot.as_ref() {
            if *key == id {
                t.cancelled.store(true, Ordering::Release);
                if !t.running.load(Ordering::Acquire) {
                    *slot = None;
                }
            }
        }
    }
}
struct Fixture {
    pixels: Vec<u8>,
}
impl Source for Fixture {
    type Generation = u64;
    type Authority = AtomicBool;
    fn description(&self) -> Description<u64> {
        Description {
            version: 1,
            identity: Identity {
                generation: 1,
                revision: 1,
            },
            bank: 0,
            split: 4,
            sprites: 3,
            used: 3,
            allocated: 3,
            size: 2,
            levels: 2,
            format: crate::material::Format::Rgba8Unorm,
            layout: upload::Layout::LayerMajorMipMajor,
        }
    }
    fn pixels(&self) -> &[u8] {
        &self.pixels
    }
    fn retained_bytes(&self) -> usize {
        self.pixels.capacity()
    }
    fn check_current(&self, a: &AtomicBool) -> Result<(), String> {
        if a.load(Ordering::Acquire) {
            Err("probe cancelled".into())
        } else {
            Ok(())
        }
    }
}
fn drive(layer: Retained<CAMetalLayer>, t: &Token) -> Result<(), String> {
    let r = TerrainRenderer::new()?;
    let mut display = r.display(layer)?;
    let chart = validation::fixture();
    let mut pixels = Vec::with_capacity(60);
    for layer in chart.sprites.chunks_exact(16) {
        pixels.extend_from_slice(layer);
        pixels.extend_from_slice(&layer[..4]);
    }
    let s = Fixture { pixels };
    let budget = upload::Budget {
        payload_bytes: 64 * 1024 * 1024,
        staging_bytes: 1024 * 1024,
    };
    let mut material = r.create_material(&s, &t.cancelled, budget)?;
    material.begin_upload(&r, s, &t.cancelled, upload::Update::Full, budget)?;
    loop {
        match material.poll_upload(&r, &t.cancelled) {
            upload::Poll::Completed(_) => break,
            upload::Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(2)),
            upload::Poll::Failed(e) => return Err(e),
            _ => return Err("missing upload".into()),
        }
    }
    // Callback retirement can briefly outlive terminal observation. Worker driver
    // defers; individual frame/presentation operations never spin/wait internally.
    while r.upload_retention().busy {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let mut frames = r.create_frames()?;
    frames.begin(
        &r,
        &material,
        &t.cancelled,
        &chart.mesh(),
        &chart.lightmap,
        validation::fixture_view(),
        Options::default(),
    )?;
    let mut receipt = loop {
        match frames.poll(&r, &material, &t.cancelled) {
            frame::Poll::Completed(t) => break t,
            frame::Poll::Pending => std::thread::sleep(std::time::Duration::from_millis(2)),
            frame::Poll::Failed(e) => return Err(e),
            _ => return Err("missing frame".into()),
        }
    };
    while r.upload_retention().busy {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let output = frames.take_completed(&r, &material, &t.cancelled, &mut receipt)?;
    let Some(mut pending) = display.present(output, 64 * 1024 * 1024, &t.cancelled)? else {
        t.status.store(4, Ordering::Release);
        return Ok(());
    };
    t.status.store(2, Ordering::Release);
    while !pending.poll()? {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    if t.cancelled.load(Ordering::Acquire) {
        return Err("presentation revoked".into());
    }
    t.status.store(3, Ordering::Release);
    Ok(())
}
/// Caller supplies a live CAMetalLayer for the synchronous call. Rust retains it.
/// The driver may wait/poll on its dedicated worker, never on a UIKit thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn helium_display_run(id: u64, layer: *mut AnyObject) -> i32 {
    // Claim under the registry lock: release cannot remove this issued handle
    // between lookup and the running transition, then admit a replacement worker.
    let t = {
        let Ok(slot) = TOKEN.lock() else { return -1 };
        let Some((key, t)) = slot.as_ref() else {
            return -1;
        };
        if *key != id
            || t.status.load(Ordering::Acquire) != 1
            || t.running.swap(true, Ordering::AcqRel)
        {
            return -1;
        }
        t.clone()
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if objc2_foundation::NSThread::isMainThread_class() || layer.is_null() {
            return Err("worker/live layer required".into());
        }
        let obj = unsafe { &*layer };
        if !unsafe { objc2::msg_send![obj, isKindOfClass: CAMetalLayer::class()] } {
            return Err("expected CAMetalLayer".into());
        }
        let layer = unsafe { Retained::retain(layer.cast::<CAMetalLayer>()) }
            .ok_or("layer retain failed")?;
        drive(layer, &t)
    }));
    let ok = matches!(result, Ok(Ok(())));
    if !ok {
        t.status.store(5, Ordering::Release);
    }
    t.running.store(false, Ordering::Release);
    if t.cancelled.load(Ordering::Acquire) {
        helium_display_release(id);
    }
    if ok { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handles_are_bounded_stale_and_repeated_release_is_safe() {
        let id = helium_display_create();
        assert_ne!(id, 0);
        assert_eq!(helium_display_create(), 0);
        assert_eq!(helium_display_status(id), 1);
        helium_display_cancel(id);
        helium_display_release(id);
        helium_display_release(id);
        assert_eq!(helium_display_status(id), -1);
        let new = helium_display_create();
        assert!(new > id);
        assert_eq!(helium_display_status(id), -1);
        helium_display_release(new);
    }
    #[test]
    #[ignore = "requires actual Apple Metal and CAMetalLayer; reports acquisition unavailable explicitly"]
    fn actual_cametal_drawable_submission() {
        use objc2_metal::*;
        use objc2_quartz_core::CAMetalLayer;
        let layer = CAMetalLayer::new();
        layer.setDevice(MTLCreateSystemDefaultDevice().as_deref());
        layer.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        layer.setFramebufferOnly(true);
        layer.setAllowsNextDrawableTimeout(true);
        layer.setPresentsWithTransaction(false);
        let mut size = layer.drawableSize();
        size.width = 256.;
        size.height = 256.;
        layer.setDrawableSize(size);
        let color = objc2_core_graphics::CGColorSpace::with_name(Some(unsafe {
            objc2_core_graphics::kCGColorSpaceLinearSRGB
        }))
        .unwrap();
        layer.setColorspace(Some(&color));
        let id = helium_display_create();
        assert_ne!(id, 0);
        let code = unsafe { helium_display_run(id, Retained::as_ptr(&layer).cast_mut().cast()) };
        let status = helium_display_status(id);
        helium_display_release(id);
        assert_eq!(
            code, 0,
            "real CAMetalLayer driver failed with status {status}"
        );
        if status == 4 {
            eprintln!(
                "G05-D DRAWABLE RUNTIME UNAVAILABLE: no CAMetalDrawable acquired; no presentation pass claimed"
            );
        } else {
            assert_eq!(status, 3);
            eprintln!(
                "G05-D acquired genuine CAMetalDrawable, encoded BGRA conversion, submitted presentDrawable, observed successful GPU completion; unattached layer, scanout NOT verified"
            );
        }
    }
}
