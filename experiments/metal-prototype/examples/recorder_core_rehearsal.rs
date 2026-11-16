//! TEST_EVENT_ONLY: recorder core rehearsal, no Swift, GPU or Minecraft claim.
use helium_metal_prototype::flight_recorder::{Cause, Emitter, Recorder, Stage, Store};
fn main() {
    let out = std::env::args()
        .nth(1)
        .expect("explicit fresh output directory required");
    let r = Recorder::memory();
    r.begin(1);
    r.record(1, Stage::HostRequested, Emitter::Game, None, 0, 0);
    r.record(1, Stage::UploadStarted, Emitter::Worker, None, 0, 0);
    std::thread::scope(|scope| {
        for (emitter, cause) in [
            (Emitter::Game, Cause::MaterialChanged),
            (Emitter::Swift, Cause::ViewLayoutInvalid),
        ] {
            let r = &r;
            scope.spawn(move || r.record(1, Stage::Cancelled, emitter, Some(cause), 11, 12));
        }
    });
    let mut store = Store::new(out.into());
    for e in r.snapshot() {
        store.append(&e).unwrap();
    }
}
