// MIT; Copyright (c) 2026 Horizon contributors.
import UIKit
import HeliumProbeKit

@main
enum ProbeEntry {
    static func main() {
        HostEvidence.shared.record(.executableEntered)
        UIApplicationMain(CommandLine.argc, CommandLine.unsafeArgv, nil,
                          NSStringFromClass(ProbeApplication.self))
    }
}

final class ProbeApplication: UIResponder, UIApplicationDelegate {
    func application(_ application: UIApplication,
                     didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]?) -> Bool {
        HostEvidence.shared.record(.uiKitInitialized)
        return true
    }
}
