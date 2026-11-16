//! Standalone regression driver for the shared production bridge; no duplicate adapter.
#[path = "../../../crates/horizon-metal-bridge/src/adapter.rs"]
mod shared;
pub use shared::*;
