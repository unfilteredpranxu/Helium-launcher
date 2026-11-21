//! The phone's launcher, in the game's own bundle. The game owns the app (winit runs UIKit's main loop and
//! never hands it back), so the launcher cannot run before it: Xcode embeds the SwiftUI launcher as
//! `Frameworks/HorizonLauncherKit.framework`, and once the game's window is up [`present`] loads it and asks
//! it to cover the game. Its Play button uncovers the game, at its title screen.
//!
//! Loaded at run time rather than linked, so a bundle without the framework (the workflow's hand-assembled
//! one, a desktop build) is the game as before. The game forbids `unsafe`, so the loading lives here.
//!
//! The launcher talks to the game in the game's own control language (docs/horizon-ctl-commands.md): it
//! hands [`present`]'s caller a line such as `worlds` or `open My World`, and the game answers with the JSON
//! the control socket would. The two share one thread, the main one, so nothing here waits: a line is queued
//! ([`take`]), the game answers it on its next frame, and the answer goes back to Swift through the callback
//! that came with it.

use std::{
    collections::VecDeque,
    sync::{Mutex, PoisonError},
};

/// Exact standalone layouts shared by the writable candidate and no-world executable.
pub fn standalone_layout(image: &std::path::Path, home: &std::path::Path) -> bool {
    fn parts(path: &std::path::Path, prefix: &str, count: usize) -> bool {
        let Ok(path) = path.strip_prefix(prefix) else {
            return false;
        };
        let Some(parts) = path.iter().map(|s| s.to_str()).collect::<Option<Vec<_>>>() else {
            return false;
        };
        parts.len() == count
            && parts[0].len() == 36
            && parts[0].bytes().enumerate().all(|(i, b)| match i {
                8 | 13 | 18 | 23 => b == b'-',
                _ => b.is_ascii_hexdigit(),
            })
            && (count == 1 || (parts[1].ends_with(".app") && parts[2] == "Horizon"))
    }
    parts(image, "/private/var/containers/Bundle/Application", 3)
        && parts(home, "/private/var/mobile/Containers/Data/Application", 1)
}

/// Compare dyld's process main image, the on-disk layout, and Foundation's container.
#[cfg(target_os = "ios")]
pub fn standalone_test_image(home: &std::path::Path) -> Option<std::path::PathBuf> {
    standalone_image(home, c"horizon_disposable_container_matches")
}

#[cfg(target_os = "ios")]
fn standalone_image(
    home: &std::path::Path,
    verifier: &std::ffi::CStr,
) -> Option<std::path::PathBuf> {
    use std::{
        ffi::{CStr, CString, c_char},
        os::unix::ffi::OsStrExt,
        path::Path,
    };
    // Public iOS declaration from <mach-o/dyld.h>; libc only exposes it on macOS.
    unsafe extern "C" {
        fn _dyld_get_image_name(image_index: u32) -> *const c_char;
    }
    // SAFETY: index zero is dyld's process main image. Copy its static C string before use.
    let raw = unsafe { _dyld_get_image_name(0) };
    if raw.is_null() {
        return None;
    }
    let image = Path::new(std::ffi::OsStr::from_bytes(
        unsafe { CStr::from_ptr(raw) }.to_bytes(),
    ))
    .canonicalize()
    .ok()?;
    if std::env::current_exe().ok()?.canonicalize().ok()? != image
        || !standalone_layout(&image, home)
    {
        return None;
    }
    let path = CString::new(home.as_os_str().as_bytes()).ok()?;
    let symbol = ios::load_symbol(verifier, false)?;
    // SAFETY: fixed Swift C ABI, and path remains NUL terminated for the call.
    let matches: extern "C" fn(*const c_char) -> bool = unsafe { std::mem::transmute(symbol) };
    matches(path.as_ptr()).then_some(image)
}

#[cfg(any(test, feature = "host-qualification"))]
pub mod qualification;

/// Service the current Apple run loop after the gameplay event handler has
/// gone away. The scoped timer keeps the default mode runnable even with no
/// open windows. A timer turn is a status probe, never an exit deadline.
#[cfg(target_vendor = "apple")]
pub fn service_retirement(mut complete: impl FnMut() -> bool) {
    use core_foundation::{
        date::CFDate,
        runloop::{CFRunLoop, CFRunLoopTimer, CFRunLoopTimerRef, kCFRunLoopDefaultMode},
    };
    use std::time::Duration;

    extern "C" fn tick(_: CFRunLoopTimerRef, _: *mut std::ffi::c_void) {}
    let run_loop = CFRunLoop::get_current();
    let interval = Duration::from_millis(50);
    let seconds = interval.as_secs_f64();
    let timer = CFRunLoopTimer::new(
        CFDate::now().abs_time() + seconds,
        seconds,
        0,
        0,
        tick,
        std::ptr::null_mut(),
    );
    // SAFETY: the system-defined default mode has process lifetime. The
    // timer callback borrows no Rust state and is valid until removal.
    let mode = unsafe { kCFRunLoopDefaultMode };
    run_loop.add_timer(&timer, mode);
    while !complete() {
        CFRunLoop::run_in_mode(mode, interval, false);
    }
    run_loop.remove_timer(&timer, mode);
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn retirement_services_native_callbacks_on_its_calling_thread() {
    use core_foundation::{
        date::CFDate,
        runloop::{CFRunLoop, CFRunLoopTimer, CFRunLoopTimerRef, kCFRunLoopDefaultMode},
    };
    use std::cell::Cell;
    thread_local! { static FIRED: Cell<bool> = const { Cell::new(false) }; }
    extern "C" fn mark(_: CFRunLoopTimerRef, _: *mut std::ffi::c_void) {
        FIRED.set(true);
    }

    let run_loop = CFRunLoop::get_current();
    let timer = CFRunLoopTimer::new(
        CFDate::now().abs_time() + 0.01,
        0.0,
        0,
        0,
        mark,
        std::ptr::null_mut(),
    );
    // SAFETY: the default run-loop mode is a process-lifetime system constant.
    let mode = unsafe { kCFRunLoopDefaultMode };
    run_loop.add_timer(&timer, mode);
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(3);
    service_retirement(|| FIRED.get() || std::time::Instant::now() >= limit);
    run_loop.remove_timer(&timer, mode);
    assert!(FIRED.get());
}

/// One line from the launcher, and the way back to it.
pub struct Request {
    pub line: String,
    reply: Option<Box<dyn FnOnce(&str) + Send>>,
}

impl Request {
    /// A request answered by `reply`; the launcher's own requests are made by [`present`]'s bridge, and
    /// tests make theirs here.
    pub fn new(line: impl Into<String>, reply: impl FnOnce(&str) + Send + 'static) -> Self {
        Self {
            line: line.into(),
            reply: Some(Box::new(reply)),
        }
    }

    /// Send the JSON answer back to the launcher.
    pub fn answer(mut self, json: &str) {
        if let Some(reply) = self.reply.take() {
            reply(json);
        }
    }
}

impl Drop for Request {
    /// A request nobody answered still answers, so the launcher never waits on a line the game dropped.
    fn drop(&mut self) {
        if let Some(reply) = self.reply.take() {
            reply(r#"{"error":"the game dropped this request"}"#);
        }
    }
}

static QUEUE: Mutex<VecDeque<Request>> = Mutex::new(VecDeque::new());

#[cfg(all(target_os = "ios", feature = "host-qualification"))]
static QUALIFICATION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Bundled iOS builds expose accounts and game management before Play.
#[cfg(target_os = "ios")]
const ENABLED: bool = true;

/// The native launcher fully covers the game's surface. Its bridge still needs polling.
static COVERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_covered(covered: bool) {
    COVERED.store(covered, std::sync::atomic::Ordering::Relaxed);
}

pub fn covered() -> bool {
    COVERED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Queue a request for the game's next frame.
pub fn push(request: Request) {
    #[cfg(all(target_os = "ios", feature = "host-qualification"))]
    if QUALIFICATION.load(std::sync::atomic::Ordering::Acquire) {
        qualification::deny(request);
        return;
    }
    QUEUE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push_back(request);
}

/// The requests the launcher sent since the last call, oldest first. Empty everywhere but a phone.
pub fn take() -> Vec<Request> {
    QUEUE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain(..)
        .collect()
}

/// Whether the game may start loading: not until the player starts it from the launcher (Play, a world, a
/// server), so opening the app costs nothing but the launcher. Open from the first where no launcher is bundled.
static STARTED: (Mutex<Option<bool>>, std::sync::Condvar) =
    (Mutex::new(None), std::sync::Condvar::new());

/// Wait for the player to start the game from the launcher; at once in a build or bundle without one. The
/// resource loader's thread waits here, so the window and the launcher come up while nothing loads. True when
/// it waited, so the options may have changed meanwhile (the launcher's Settings and Packs).
pub fn wait_for_start() -> bool {
    let (started, signal) = &STARTED;
    let mut started = started.lock().unwrap_or_else(PoisonError::into_inner);
    let open = *started.get_or_insert_with(|| !bundled());
    if open {
        return false;
    }
    while *started != Some(true) {
        started = signal.wait(started).unwrap_or_else(PoisonError::into_inner);
    }
    true
}

/// Let the game start loading (the launcher's `launcher-start`, or a launcher that could not be shown).
pub fn start() {
    let (started, signal) = &STARTED;
    *started.lock().unwrap_or_else(PoisonError::into_inner) = Some(true);
    signal.notify_all();
}

/// Return to the launcher-only state after the engine has shut down.
pub fn reset() {
    *STARTED.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(!bundled());
}

/// Whether the game is still waiting to be started.
pub fn waiting() -> bool {
    let started = STARTED.0.lock().unwrap_or_else(PoisonError::into_inner);
    started.is_none_or(|open| !open) && bundled()
}

/// Whether this bundle carries the launcher, which holds the game until it is started.
fn bundled() -> bool {
    #[cfg(target_os = "ios")]
    return ENABLED && ios::framework().is_some();
    #[cfg(not(target_os = "ios"))]
    false
}

/// Load the launcher and show it over the game, once per run: the loading screen's window asks, and so does
/// the game's when it makes its own. False when this build or bundle has none, or it was already shown. A
/// launcher that can't be shown can't start the game either, so the game starts by itself then.
pub fn present() -> bool {
    static SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if SHOWN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let shown = show();
    if !shown {
        start();
    }
    shown
}

/// Show the launcher over the game again (Quit Game, which an iPhone app does not do itself). False when this
/// build or bundle has none; the launcher ignores a second request while it is up.
pub fn show() -> bool {
    #[cfg(target_os = "ios")]
    return ENABLED && ios::present();
    #[cfg(not(target_os = "ios"))]
    false
}

/// Show the game files download over the game, in SwiftUI (packaging/ios/Launcher/DownloadView.swift): upright,
/// with its progress, speed and time left, asked of the game with `download-state`. Shown whether or not the
/// launcher is switched on, since it only needs the framework. False when this build or bundle has none, and the
/// game's own download screen is all there is.
pub fn present_download() -> bool {
    #[cfg(target_os = "ios")]
    return ios::call(ios::DOWNLOAD);
    #[cfg(not(target_os = "ios"))]
    false
}

/// Show `path` in the system's file browser: the Files app on a phone. False on a desktop, or when the
/// bundle has no launcher framework, and the caller opens it with the desktop's file manager instead.
pub fn open_folder(path: &std::path::Path) -> bool {
    #[cfg(target_os = "ios")]
    return ios::call_with_path(ios::OPEN_FOLDER, path);
    #[cfg(not(target_os = "ios"))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "ios")]
fn generated_boot_event(event: &'static str) {
    use std::io::Write;
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let path = std::path::PathBuf::from(home).join("Documents/horizon-boot.log");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        if file.metadata().is_ok_and(|m| m.len() < 4096) {
            let _ = writeln!(file, "{event}");
        }
    }
}

/// Optional OS time for a checkpoint. Drop ends an abandoned request, including
/// a server queue destroyed before it dispatched the save. No UIKit objects
/// cross the thread boundary.
pub struct BackgroundCheckpoint {
    generation: u64,
    end: extern "C" fn(u64, u8),
}

impl BackgroundCheckpoint {
    pub fn begin() -> Option<Self> {
        #[cfg(target_os = "ios")]
        return ios::begin_checkpoint();
        #[cfg(not(target_os = "ios"))]
        None
    }

    pub fn finish(mut self, durable: bool) {
        (self.end)(self.generation, if durable { 1 } else { 2 });
        self.generation = 0;
    }
}

impl Drop for BackgroundCheckpoint {
    fn drop(&mut self) {
        if self.generation != 0 {
            (self.end)(self.generation, 0);
        }
    }
}

#[cfg(target_os = "ios")]
mod ios {
    use std::ffi::{CStr, CString, c_char, c_void};

    /// `horizon_launcher_present` in packaging/ios/Launcher/Present.swift.
    const SYMBOL: &CStr = c"horizon_launcher_present";
    /// `horizon_download_present` in packaging/ios/Launcher/DownloadView.swift.
    pub const DOWNLOAD: &CStr = c"horizon_download_present";
    /// `horizon_open_folder` in packaging/ios/Launcher/Mods.swift.
    pub const OPEN_FOLDER: &CStr = c"horizon_open_folder";

    /// How the launcher hears back: its context, and the answer as NUL-terminated UTF-8 JSON that lives
    /// only for the call.
    type Done = extern "C" fn(ctx: *mut c_void, reply: *const c_char);
    /// How the launcher asks: a NUL-terminated line, and where its answer goes.
    type Submit = extern "C" fn(line: *const c_char, ctx: *mut c_void, done: Done);

    extern "C" fn submit(line: *const c_char, ctx: *mut c_void, done: Done) {
        let line = if line.is_null() {
            String::new()
        } else {
            // SAFETY: the launcher passes a NUL-terminated string it keeps alive for this call
            // (`withCString` in Bridge.swift), and nothing here keeps the pointer past it.
            unsafe { CStr::from_ptr(line) }
                .to_string_lossy()
                .into_owned()
        };
        // The context is the launcher's: carried back untouched, as an address so the reply is Send.
        let ctx = ctx as usize;
        super::push(super::Request::new(line, move |json: &str| {
            let text = CString::new(json.replace('\0', "")).unwrap_or_default();
            done(ctx as *mut c_void, text.as_ptr());
        }));
    }

    /// The launcher framework's binary in this bundle, if it has one.
    pub fn framework() -> Option<CString> {
        let framework = std::env::current_exe()
            .ok()?
            .parent()?
            .join("Frameworks/HorizonLauncherKit.framework/HorizonLauncherKit");
        framework
            .exists()
            .then(|| CString::new(framework.into_os_string().into_encoded_bytes()).ok())?
    }

    pub(super) fn begin_checkpoint() -> Option<super::BackgroundCheckpoint> {
        let begin = symbol(c"helium_checkpoint_begin")?;
        let end = symbol(c"helium_checkpoint_end")?;
        // SAFETY: These are the dedicated fixed-width Swift C exports. The
        // begin call validates main-thread use and end dispatches to main.
        let begin: extern "C" fn() -> u64 = unsafe { std::mem::transmute(begin) };
        let end: extern "C" fn(u64, u8) = unsafe { std::mem::transmute(end) };
        let generation = begin();
        (generation != 0).then_some(super::BackgroundCheckpoint { generation, end })
    }

    pub fn present() -> bool {
        call(SYMBOL)
    }

    /// Call `symbol` in the launcher framework with the bridge; false when there is no framework or symbol.
    pub fn call(symbol_name: &CStr) -> bool {
        let Some(symbol) = symbol(symbol_name) else {
            return false;
        };
        // SAFETY: the launcher's `@_cdecl` function takes the bridge's C function pointer and returns a
        // Bool; it is called on the main thread (the game's window event), where UIKit has to be touched.
        unsafe {
            let present: extern "C" fn(Submit) -> bool = std::mem::transmute(symbol);
            present(submit)
        }
    }

    /// Call `symbol` in the launcher framework with a NUL-terminated path; false when there is no
    /// framework, no symbol, or the path has a NUL in it.
    pub fn call_with_path(symbol_name: &CStr, path: &std::path::Path) -> bool {
        let Ok(path) = CString::new(path.as_os_str().as_encoded_bytes()) else {
            return false;
        };
        let Some(symbol) = symbol(symbol_name) else {
            return false;
        };
        // SAFETY: the launcher's `@_cdecl` function takes a NUL-terminated UTF-8 path and returns a Bool,
        // on the main thread, where UIKit has to be touched; the path outlives the call.
        unsafe {
            let open: extern "C" fn(*const c_char) -> bool = std::mem::transmute(symbol);
            open(path.as_ptr())
        }
    }

    /// Load the launcher framework and find `symbol_name` in it.
    pub(super) fn symbol(symbol_name: &CStr) -> Option<*mut c_void> {
        load_symbol(symbol_name, true)
    }

    pub(super) fn load_symbol(symbol_name: &CStr, boot_log: bool) -> Option<*mut c_void> {
        static HANDLE: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        let handle = *HANDLE.get_or_init(|| {
            let Some(path) = framework() else {
                log::info!("Horizon: no launcher framework in this bundle");
                if boot_log {
                    super::generated_boot_event("launcher-framework-missing");
                }
                return None;
            };
            if boot_log {
                super::generated_boot_event("launcher-framework-load-attempted");
            }
            // SAFETY: this is the absolute in-bundle framework path. A
            // successful handle remains retained for the process lifetime.
            let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) };
            if handle.is_null() {
                log::warn!("Horizon: the launcher framework did not load");
                if boot_log {
                    super::generated_boot_event("launcher-framework-load-failed");
                }
                None
            } else {
                if boot_log {
                    super::generated_boot_event("launcher-framework-loaded");
                }
                Some(handle as usize)
            }
        });
        let handle = handle? as *mut c_void;
        // SAFETY: the framework handle stays valid for the process lifetime.
        unsafe {
            let symbol = libc::dlsym(handle, symbol_name.as_ptr());
            if symbol.is_null() {
                log::warn!("Horizon: the launcher framework has no {symbol_name:?}");
                return None;
            }
            Some(symbol)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_desktop_never_waits_to_be_started() {
        assert!(!waiting());
        assert!(!wait_for_start());
    }

    #[test]
    fn requests_come_out_in_order_and_answer_once() {
        let answers = Arc::new(Mutex::new(Vec::new()));
        for line in ["worlds", "servers"] {
            let answers = answers.clone();
            push(Request::new(line, move |json: &str| {
                answers.lock().unwrap().push(json.to_owned())
            }));
        }
        let taken = take();
        assert_eq!(
            taken.iter().map(|r| r.line.as_str()).collect::<Vec<_>>(),
            ["worlds", "servers"]
        );
        assert!(take().is_empty());
        let mut taken = taken.into_iter();
        taken.next().unwrap().answer(r#"{"ok":true}"#);
        // Dropped unanswered: the launcher still hears back.
        drop(taken);
        let answers = answers.lock().unwrap();
        assert_eq!(answers[0], r#"{"ok":true}"#);
        assert!(answers[1].contains("error"));
        assert_eq!(answers.len(), 2);
    }
}
