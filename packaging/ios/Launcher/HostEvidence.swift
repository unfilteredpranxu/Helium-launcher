// MIT; Copyright (c) 2026 Horizon contributors.
import Foundation

/// Stages carry no input from a world, account, file path or native error string.
public enum HostStage: String {
    case executableEntered, uiKitInitialized, sceneConnected, swiftLauncherAttached
    case launcherRequested, launcherUnavailable, gameHandoffRequested
    case sceneActive, sceneInactive, sceneBackground, sceneDisconnected
    case metalDeviceReady, metalDeviceUnavailable, metalLayerAttached
    case metalWorkerEntered, metalRendererReady, metalFrameCompleted, drawableAttempted
    case presentSubmitted, presentCompleted, drawableUnavailable, metalFailed, metalCancelled
    case checkpointRequested, checkpointDurable, checkpointFailed, checkpointAbandoned
    case backgroundTimeDenied, backgroundTimeExpired
    case probeAdmitted, probeMemoryPressure, probeLayerRevealed, probeRetired, probeShutdown
    case probeAttachmentUnavailable, probeUploadCompleted
}

/// A fixed-size boot snapshot. Writes only short, enumerated stages in the
/// user's Documents/logs folder. All callers in either host run on main.
public final class HostEvidence {
    public static let shared = HostEvidence()
    private static let limit = 48
    private let folder: URL?
    private let filename: String
    private let header: String?
    private let start = ProcessInfo.processInfo.systemUptime
    private var entries: [String] = []
    private var last: String?

    public init(folder: URL? = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)
        .first?.appendingPathComponent("logs", isDirectory: true), filename: String = "Helium-Host.txt",
        header: String? = nil) {
        self.folder = folder
        self.filename = filename
        self.header = header
    }

    public func record(_ stage: HostStage, generation: UInt64 = 0) {
        precondition(Thread.isMainThread)
        let entry = generation == 0 ? stage.rawValue : "\(stage.rawValue) generation=\(generation)"
        guard entry != last else { return }
        last = entry
        if entries.count == Self.limit { entries.remove(at: 16) }
        let ms = Int((ProcessInfo.processInfo.systemUptime - start) * 1000)
        entries.append("\(ms) \(entry)")
        guard let folder else { return }
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            let file = folder.appendingPathComponent(filename)
            let lines = (header.map { [$0] } ?? []) + entries
            try Data((lines.joined(separator: "\n") + "\n").utf8).write(to: file, options: .atomic)
        } catch {
            // Do not print path fragments or error payloads from Foundation.
            NSLog("Helium: host evidence write failed")
        }
    }
}
