//! Audited iOS main-view/worker handoff. UIKit remains in the bundled Swift host.
//! MIT; Copyright (c) 2026 Horizon contributors.
use crate::{attached_protocol::Gate, display::Display, terrain::TerrainRenderer};
use objc2::{ClassType, rc::Retained, runtime::AnyObject};
use objc2_quartz_core::CAMetalLayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ffi::{CString, c_void},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
};
type Work = Box<dyn FnOnce(Layer) + Send>;
struct Entry {
    id: u64,
    cancel: Arc<AtomicBool>,
    status: Arc<Gate>,
    work: Option<Work>,
}
static ENTRY: Mutex<Option<Entry>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);
/// No UIKit/Metal object is transferred through this Send CPU registry.
pub struct Ticket {
    id: u64,
    cancel: Arc<AtomicBool>,
}
pub struct Layer {
    layer: Retained<CAMetalLayer>,
    id: u64,
}
impl Layer {
    pub fn display(&self, renderer: &TerrainRenderer) -> Result<Display, String> {
        renderer.display(self.layer.clone())
    }
    pub fn generation(&self) -> u64 {
        self.id
    }
}
fn symbol(name: &str) -> Result<*mut c_void, String> {
    let path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("no app bundle")?
        .join("Frameworks/HorizonLauncherKit.framework/HorizonLauncherKit");
    let path =
        CString::new(path.to_string_lossy().as_bytes()).map_err(|_| "invalid framework path")?;
    let name = CString::new(name).map_err(|_| "invalid symbol")?;
    // System loader is thread safe. Handles intentionally remain loaded: callbacks
    // cannot point into an unloaded framework. Reuse one handle, not a dlopen loop.
    static HANDLE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let handle =
        *HANDLE.get_or_init(|| unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) as usize });
    if handle == 0 {
        return Err("native display framework unavailable".into());
    }
    let abi = unsafe {
        libc::dlsym(
            handle as *mut c_void,
            c"horizon_native_display_abi".as_ptr(),
        )
    };
    if abi.is_null() {
        return Err("native display ABI unavailable".into());
    }
    let abi: extern "C" fn() -> u32 = unsafe { std::mem::transmute(abi) };
    if abi() != 1 {
        return Err("unsupported native display ABI".into());
    }
    let p = unsafe { libc::dlsym(handle as *mut c_void, name.as_ptr()) };
    if p.is_null() {
        Err("native display host symbol unavailable".into())
    } else {
        Ok(p)
    }
}
pub fn device_test_mode() -> Result<i32, String> {
    let p = symbol("horizon_native_display_mode")?;
    let f: extern "C" fn() -> i32 = unsafe { std::mem::transmute(p) };
    Ok(f())
}
extern "C" fn invalidate(id: u64) {
    if let Ok(entry) = ENTRY.lock() {
        if let Some(e) = entry.as_ref().filter(|e| e.id == id) {
            e.status.cancel();
        }
    }
}
extern "C" fn ready(id: u64) -> bool {
    ENTRY.lock().ok().is_some_and(|e| {
        e.as_ref().is_some_and(|e| {
            e.id == id && !e.cancel.load(Ordering::Acquire) && e.status.published()
        })
    })
}
extern "C" fn run(id: u64, p: *mut AnyObject) {
    let taken = ENTRY.lock().ok().and_then(|mut entry| {
        let e = entry.as_mut().filter(|e| e.id == id)?;
        if e.cancel.load(Ordering::Acquire) || !e.status.start() {
            return None;
        }
        Some((e.work.take()?, e.status.clone(), e.cancel.clone()))
    });
    let Some((work, status, cancel)) = taken else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if objc2_foundation::NSThread::isMainThread_class() || p.is_null() {
            return Err("worker/live CAMetalLayer required");
        }
        // ABI caller owns the actual layer for the synchronous invocation. Swift's
        // dedicated Thread strongly retains the separate layer until this returns.
        let object = unsafe { &*p };
        let valid: bool = unsafe { objc2::msg_send![object,isKindOfClass:CAMetalLayer::class()] };
        if !valid {
            return Err("wrong layer class");
        }
        let layer =
            unsafe { Retained::retain(p.cast::<CAMetalLayer>()) }.ok_or("layer retain failed")?;
        work(Layer { layer, id });
        Ok(())
    }));
    if !matches!(result, Ok(Ok(()))) {
        status.finish(false);
        cancel.store(true, Ordering::Release);
    }
    if cancel.load(Ordering::Acquire) {
        if let Ok(mut e) = ENTRY.lock() {
            if e.as_ref().is_some_and(|e| e.id == id) {
                *e = None;
            }
        }
    }
}
/// Must be called on the winit/UIKit main thread with the ACTUAL game's window.
/// Only CPU-owned data may be captured. Callback is synchronous on a dedicated
/// Swift worker; Layer is constructed there and is never marked Send/Sync.
pub fn submit<T: Send + 'static>(
    window: &impl HasWindowHandle,
    cancel: Arc<AtomicBool>,
    job: impl FnOnce(Layer) -> Result<T, String> + Send + 'static,
) -> Result<(Ticket, Receiver<Result<T, String>>), String> {
    if !objc2_foundation::NSThread::isMainThread_class() {
        return Err("attachment request requires main thread".into());
    }
    let RawWindowHandle::UiKit(view) = window.window_handle().map_err(|e| e.to_string())?.as_raw()
    else {
        return Err("expected actual UIKit window".into());
    };
    let request = symbol("horizon_native_display_request")?;
    let request: extern "C" fn(
        u64,
        *mut c_void,
        extern "C" fn(u64, *mut AnyObject),
        extern "C" fn(u64),
        extern "C" fn(u64) -> bool,
    ) -> bool = unsafe { std::mem::transmute(request) };
    let id = NEXT
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| "native session IDs exhausted")?;
    let (send, recv) = mpsc::sync_channel(1);
    let status = Arc::new(Gate::new(cancel.clone()));
    let state = status.clone();
    {
        let mut e = ENTRY.lock().map_err(|_| "native registry poisoned")?;
        if e.is_some() {
            return Err("native attached session busy/retiring".into());
        }
        *e = Some(Entry {
            id,
            cancel: cancel.clone(),
            status,
            work: Some(Box::new(move |layer| {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(layer)))
                    .unwrap_or_else(|_| Err("native worker panic".into()));
                state.finish(result.is_ok());
                let _ = send.send(result);
            })),
        });
    }
    // The borrowed raw UIView exists during this call; Swift records weak view/
    // window ownership and strongly owns its separate child, never this pointer.
    if !request(id, view.ui_view.as_ptr(), run, invalidate, ready) {
        if let Ok(mut e) = ENTRY.lock() {
            if e.as_ref().is_some_and(|e| e.id == id) {
                *e = None;
            }
        }
        return Err("UIKit target unavailable, covered or zero-sized".into());
    }
    Ok((Ticket { id, cancel }, recv))
}
impl Ticket {
    pub fn publish(&self) {
        let allowed = ENTRY.lock().ok().is_some_and(|e| {
            e.as_ref()
                .is_some_and(|e| e.id == self.id && e.status.publish())
        });
        if !allowed {
            return;
        }
        if let Ok(p) = symbol("horizon_native_display_publish") {
            let f: extern "C" fn(u64) = unsafe { std::mem::transmute(p) };
            f(self.id);
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        if let Ok(p) = symbol("horizon_native_display_cancel") {
            let f: extern "C" fn(u64) = unsafe { std::mem::transmute(p) };
            f(self.id);
        }
        if let Ok(mut e) = ENTRY.lock() {
            if e.as_ref()
                .is_some_and(|e| e.id == self.id && !e.status.running())
            {
                *e = None;
            }
        }
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.cancel();
    }
}
