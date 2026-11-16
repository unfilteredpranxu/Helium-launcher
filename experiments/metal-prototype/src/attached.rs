//! Audited iOS native handoff; MIT, Horizon contributors.
use crate::flight_recorder::{self as recorder, Cause, Emitter, Stage};

use crate::{attached_protocol::Gate, display::Display, terrain::TerrainRenderer};
use objc2::{ClassType, rc::Retained, runtime::AnyObject};
use objc2_quartz_core::CAMetalLayer;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    ffi::{CString, c_void},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
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

/// No UIKit/Metal object is transferred through this Send CPU registry.
pub struct Ticket {
    id: u64,
    cancel: Arc<AtomicBool>,
}
pub struct Layer {
    layer: Retained<CAMetalLayer>,
    id: u64,
    gate: Arc<Gate>,
}
impl Layer {
    pub fn display(&self, renderer: &TerrainRenderer) -> Result<Display, String> {
        if !self.gate.claim_display() {
            return Err("attached target already claimed, cancelled or not running".into());
        }
        Ok(renderer
            .display(self.layer.clone())?
            .with_attachment(self.gate.clone()))
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
static DIAGNOSTICS_ENABLED: AtomicBool = AtomicBool::new(false);
pub fn diagnostics_enabled() -> bool {
    DIAGNOSTICS_ENABLED.load(Ordering::Acquire)
}
fn install_diagnostics() -> Result<bool, String> {
    let p = symbol("horizon_diag_install_v2")?;
    let f: extern "C" fn(
        u32,
        extern "C" fn(u64, u32, u32, u64, u64) -> u32,
        unsafe extern "C" fn(*mut u64, usize) -> u32,
    ) -> u32 = unsafe { std::mem::transmute(p) };
    let enabled = f(
        2,
        diagnostic_event,
        crate::diagnostic_ffi::helium_diag_snapshot_v2,
    ) == 2;
    if enabled {
        crate::diagnostic_ffi::helium_diag_init_v2(0);
        DIAGNOSTICS_ENABLED.store(true, Ordering::Release);
    }
    Ok(enabled)
}
extern "C" fn diagnostic_event(
    id: u64,
    stage: u32,
    code: u32,
    expected: u64,
    observed: u64,
) -> u32 {
    let result = crate::diagnostic_ffi::helium_diag_event_v2(id, stage, code, expected, observed);
    if result == 2 && matches!(stage, 29 | 30) && code != 0 {
        if let Ok(entry) = ENTRY.lock() {
            if let Some(e) = entry.as_ref().filter(|e| e.id == id) {
                e.status.cancel();
            }
        }
    }
    result
}
pub fn hud_window(window: &impl HasWindowHandle) -> Result<(), String> {
    if !objc2_foundation::NSThread::isMainThread_class() {
        return Err("HUD main thread required".into());
    }
    if !install_diagnostics()? {
        return Ok(());
    }
    let RawWindowHandle::UiKit(view) = window.window_handle().map_err(|e| e.to_string())?.as_raw()
    else {
        return Err("HUD UIKit handle required".into());
    };
    let p = symbol("horizon_diag_hud_v2")?;
    let f: extern "C" fn(*mut c_void) = unsafe { std::mem::transmute(p) };
    f(view.ui_view.as_ptr());
    Ok(())
}
pub fn can_start() -> bool {
    if !objc2_foundation::NSThread::isMainThread_class() {
        return false;
    }
    if crate::upload::native_busy() || ENTRY.lock().map(|e| e.is_some()).unwrap_or(true) {
        return false;
    }
    let Ok(p) = symbol("horizon_diag_host_idle_v2") else {
        return false;
    };
    let f: extern "C" fn() -> bool = unsafe { std::mem::transmute(p) };
    f()
}
pub fn device_test_mode() -> Result<i32, String> {
    let _ = install_diagnostics()?;
    let p = symbol("horizon_native_display_mode")?;
    let f: extern "C" fn() -> i32 = unsafe { std::mem::transmute(p) };
    Ok(f())
}
extern "C" fn invalidate(id: u64) {
    recorder::global().record(
        id,
        Stage::Cancelled,
        Emitter::Swift,
        None, // typed v2 Swift event precedes this v1 compatibility cleanup
        0,
        0,
    );
    if let Ok(entry) = ENTRY.lock() {
        if let Some(e) = entry.as_ref().filter(|e| e.id == id) {
            e.status.cancel();
        }
    }
}
pub(crate) fn diagnostic_reveal_allowed(id: u64) -> bool {
    ready(id)
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
        work(Layer {
            layer,
            id,
            gate: status.clone(),
        });
        Ok(())
    }));
    if !matches!(result, Ok(Ok(()))) {
        recorder::global().record(
            id,
            Stage::Cancelled,
            Emitter::Worker,
            Some(if result.is_err() {
                Cause::Panic
            } else {
                Cause::WorkerFailed
            }),
            0,
            0,
        );
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
    let id = recorder::global().allocate(0).ok_or("attempt limit")?;
    submit_attempt(window, cancel, id, job)
}
pub fn submit_attempt<T: Send + 'static>(
    window: &impl HasWindowHandle,
    cancel: Arc<AtomicBool>,
    id: u64,
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
    // Paired publication/release ABI must exist BEFORE UI resources are created.
    let _ = symbol("horizon_native_display_publish")?;
    let _ = symbol("horizon_native_display_cancel")?;
    let request: extern "C" fn(
        u64,
        *mut c_void,
        extern "C" fn(u64, *mut AnyObject),
        extern "C" fn(u64),
        extern "C" fn(u64) -> bool,
    ) -> bool = unsafe { std::mem::transmute(request) };

    let (send, recv) = mpsc::sync_channel(1);
    let status = Arc::new(Gate::new(cancel.clone()));
    let state = status.clone();
    {
        let mut e = ENTRY.lock().map_err(|_| "native registry poisoned")?;
        if e.is_some() {
            return Err("native attached session busy/retiring".into());
        }
        recorder::global().record(id, Stage::HostRequested, Emitter::Game, None, 0, 0);
        *e = Some(Entry {
            id,
            cancel: cancel.clone(),
            status,
            work: Some(Box::new(move |layer| {
                let mut result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(layer)))
                        .unwrap_or_else(|_| Err("native worker panic".into()));
                if result.is_err() {
                    recorder::global().record(
                        id,
                        Stage::Cancelled,
                        Emitter::Worker,
                        Some(Cause::WorkerFailed),
                        0,
                        0,
                    );
                }
                if !state.finish(result.is_ok()) && result.is_ok() {
                    result = Err(
                        "worker returned without a current exact native presentation completion"
                            .into(),
                    );
                }
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
    pub fn diagnostic_id(&self) -> u64 {
        self.id
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
    pub fn publish(&self) {
        recorder::global().record(
            self.id,
            Stage::PublishRequested,
            Emitter::Ticket,
            None,
            0,
            0,
        );
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
        recorder::global().record(
            self.id,
            Stage::Cancelled,
            Emitter::Ticket,
            Some(Cause::TicketCancelled),
            0,
            0,
        );
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
