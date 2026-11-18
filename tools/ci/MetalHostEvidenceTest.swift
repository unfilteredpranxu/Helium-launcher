import Foundation

@main
enum EvidenceTest {
    static func main() throws {
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: folder) }
        let trace = HostEvidence(folder: folder)
        let path = folder.appendingPathComponent("Helium-Host.txt")
        trace.record(.executableEntered)
        trace.record(.metalDeviceUnavailable)
        let early = try String(contentsOf: path, encoding: .utf8)
        precondition(early.contains("executableEntered") && early.contains("metalDeviceUnavailable"))
        trace.record(.metalDeviceUnavailable)
        let repeated = try String(contentsOf: path, encoding: .utf8)
        precondition(repeated == early)
        for _ in 0..<200 {
            trace.record(.sceneInactive)
            trace.record(.sceneActive)
        }
        trace.record(.presentCompleted)
        let final = try String(contentsOf: path, encoding: .utf8)
        precondition(final.utf8.count < 4096 && final.split(separator: "\n").count == 48)
        precondition(final.contains("executableEntered") && final.contains("metalDeviceUnavailable"))
        precondition(final.hasSuffix("presentCompleted\n"))
        print("PASS: persistent early failure, bounded history, repeat suppression, latest completion")
    }
}
