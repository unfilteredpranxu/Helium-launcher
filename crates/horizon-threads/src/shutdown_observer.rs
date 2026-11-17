//! Optional numeric cleanup observation. No borrowed game data or save ownership.
use std::sync::OnceLock;
#[derive(Clone, Copy)]
pub enum Target {
    StreamingDrain,
    JobRegistrationDrain,
    LodJoin,
}
static OBSERVER: OnceLock<fn(Target, bool)> = OnceLock::new();
pub fn install(observer: fn(Target, bool)) {
    let _ = OBSERVER.set(observer);
}
pub fn event(target: Target, beginning: bool) {
    if let Some(observer) = OBSERVER.get() {
        observer(target, beginning);
    }
}
