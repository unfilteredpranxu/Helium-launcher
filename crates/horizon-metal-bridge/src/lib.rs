//! G06 v1 production handoff. Default gameplay does not depend on this crate.
//! MIT; Copyright (c) 2026 Horizon contributors.
#![forbid(unsafe_code)]
pub mod adapter;
mod runtime;
pub use runtime::{Controller, Mode, enabled};

pub fn runtime_snapshot_budget() -> usize {
    runtime::SNAPSHOT_BUDGET
}

/// Actual winit/UIKit game window; does not create a renderer/surface.
pub fn diagnostic_window(window: &winit::window::Window) {
    #[cfg(target_os = "ios")]
    {
        if helium_metal_prototype::attached::hud_window(window).is_err()
            && helium_metal_prototype::attached::diagnostics_enabled()
        {
            helium_metal_prototype::flight_recorder::emit(
                helium_metal_prototype::flight_recorder::Stage::Cancelled,
                helium_metal_prototype::flight_recorder::Emitter::Game,
                Some(helium_metal_prototype::flight_recorder::Cause::HudUnavailable),
                0,
                0,
            );
        }
    }
    #[cfg(not(target_os = "ios"))]
    let _ = window;
}
pub fn checkpoint(stage: u32) {
    #[cfg(target_os = "ios")]
    if !helium_metal_prototype::attached::diagnostics_enabled() {
        return;
    }
    if let Some(stage) = helium_metal_prototype::flight_recorder::Stage::from_code(stage) {
        helium_metal_prototype::flight_recorder::emit(
            stage,
            helium_metal_prototype::flight_recorder::Emitter::Game,
            None,
            0,
            0,
        );
    }
}

/// Called after iOS has selected Documents, before loading resources/window creation.
/// The qualified bundle marker is checked by the native host; ordinary apps remain off.
pub fn bootstrap_diagnostics() {
    #[cfg(target_os = "ios")]
    {
        let _ = helium_metal_prototype::attached::device_test_mode();
    }
}
