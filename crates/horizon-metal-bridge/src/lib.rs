//! G06 v1 production handoff. Default gameplay does not depend on this crate.
//! MIT; Copyright (c) 2026 Horizon contributors.
#![forbid(unsafe_code)]
pub mod adapter;
mod runtime;
pub use runtime::{Controller, Mode, enabled};

pub fn runtime_snapshot_budget() -> usize {
    runtime::SNAPSHOT_BUDGET
}
