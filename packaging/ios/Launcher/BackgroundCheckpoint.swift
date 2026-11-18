// MIT; Copyright (c) 2026 Horizon contributors.
import UIKit

// All task identifiers are created, expired and ended on UIKit's thread.
// The generation also guards a queued worker receipt after expiration.
private enum BackgroundCheckpoint {
    static var next: UInt64 = 0
    static var active: (UInt64, UIBackgroundTaskIdentifier)?

    static func end(_ generation: UInt64, stage: HostStage) {
        guard let (current, task) = active, current == generation else { return }
        active = nil
        UIApplication.shared.endBackgroundTask(task)
        HostEvidence.shared.record(stage)
    }
}

@_cdecl("helium_checkpoint_begin")
public func heliumCheckpointBegin() -> UInt64 {
    guard Thread.isMainThread, BackgroundCheckpoint.active == nil else { return 0 }
    BackgroundCheckpoint.next += 1
    let generation = BackgroundCheckpoint.next
    HostEvidence.shared.record(.checkpointRequested)
    let task = UIApplication.shared.beginBackgroundTask(withName: "Helium world checkpoint") {
        BackgroundCheckpoint.end(generation, stage: .backgroundTimeExpired)
    }
    guard task != .invalid else {
        HostEvidence.shared.record(.backgroundTimeDenied)
        return 0
    }
    BackgroundCheckpoint.active = (generation, task)
    return generation
}

@_cdecl("helium_checkpoint_end")
public func heliumCheckpointEnd(_ generation: UInt64, _ outcome: UInt8) {
    DispatchQueue.main.async {
        let stage: HostStage = outcome == 1 ? .checkpointDurable :
            (outcome == 2 ? .checkpointFailed : .checkpointAbandoned)
        BackgroundCheckpoint.end(generation, stage: stage)
    }
}
