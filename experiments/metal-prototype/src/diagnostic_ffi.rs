//! Additive diagnostics ABI v2; the five display ABI v1 symbols are unchanged.
//! MIT; Copyright (c) 2026 Horizon contributors.
use crate::flight_recorder::{self as r, Cause, Emitter, Stage};
#[unsafe(no_mangle)]
pub extern "C" fn helium_diag_init_v2(test_only: u32) -> u32 {
    std::panic::catch_unwind(|| {
        #[cfg(target_os = "ios")]
        let path = std::env::current_dir().unwrap_or_default().join("logs");
        #[cfg(not(target_os = "ios"))]
        let path = std::env::var_os("HELIUM_DIAGNOSTIC_DIRECTORY")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("helium-diag-host"));
        let _ = r::initialize(path, test_only != 0);
        2
    })
    .unwrap_or(0)
}
#[unsafe(no_mangle)]
pub extern "C" fn helium_diag_event_v2(
    id: u64,
    stage: u32,
    code: u32,
    expected: u64,
    observed: u64,
) -> u32 {
    std::panic::catch_unwind(|| {
        let Some(stage) = Stage::from_code(stage) else {
            return 0;
        };
        if !r::global().known(id) {
            return 0;
        }
        if !matches!(
            stage,
            Stage::ModeSelected
                | Stage::LauncherShown
                | Stage::LauncherDismissed
                | Stage::Attached
                | Stage::WorkerStarted
                | Stage::UiRevealed
                | Stage::Removed
                | Stage::Cancelled
                | Stage::SelfTest
                | Stage::DeviceMetadata
        ) {
            return 0;
        }
        #[cfg(target_os = "ios")]
        if stage == Stage::UiRevealed && !crate::attached::diagnostic_reveal_allowed(id) {
            return 0;
        }
        if code > 41 {
            return 0;
        }
        r::global().record(
            id,
            stage,
            Emitter::Swift,
            if code == 0 {
                None
            } else {
                Some(Cause::from_code(code))
            },
            expected,
            observed,
        );
        2
    })
    .unwrap_or(0)
}
/// Caller supplies 32 writable u64 words for this synchronous copy only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn helium_diag_snapshot_v2(out: *mut u64, count: usize) -> u32 {
    if out.is_null() || count != 32 {
        return 0;
    }
    std::panic::catch_unwind(|| {
        let data = r::global().status();
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), out, 32) };
        2
    })
    .unwrap_or(0)
}
/// Host-only E2E admission; never accepted for real iOS gameplay attempts.
#[unsafe(no_mangle)]
pub extern "C" fn helium_diag_test_attempt_v2(id: u64) -> u32 {
    if cfg!(target_os = "ios") || r::global().status()[15] == 0 {
        return 0;
    }
    u32::from(r::global().begin_world(id, 99)) * 2
}
#[unsafe(no_mangle)]
pub extern "C" fn helium_diag_flush_test_v2() -> u32 {
    if cfg!(target_os = "ios") {
        return 0;
    }
    u32::from(r::global().wait_ack(std::time::Duration::from_secs(5))) * 2
}
