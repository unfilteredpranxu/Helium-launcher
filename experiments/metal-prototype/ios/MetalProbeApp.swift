// MIT; see the repository LICENSE and preserved Horizon attribution.
import SwiftUI
import UIKit

@main
@MainActor
struct MetalProbeApp: App {
    var body: some Scene {
        WindowGroup { ProbeView() }
    }
}

@MainActor
final class ProbeModel: ObservableObject {
    @Published private(set) var running = false
    @Published private(set) var passed: Bool?
    @Published private(set) var image: UIImage?
    @Published private(set) var status = NSLocalizedString("Ready", comment: "Probe status")
    @Published private(set) var report = NSLocalizedString(
        "Run the standalone offscreen Metal smoke test.", comment: "Initial instructions"
    )
    private let worker = DispatchQueue(label: "dev.helium.metalprobe.worker", qos: .userInitiated)

    func run() {
        guard !running else { return }
        running = true
        passed = nil
        image = nil
        status = NSLocalizedString("Running", comment: "Probe status")
        report = NSLocalizedString(
            "Waiting for the worker and GPU. A stalled driver has no timeout; close this test app if necessary.",
            comment: "Blocking probe instructions"
        )
        worker.async {
            let result = ProbeRunner.run()
            DispatchQueue.main.async {
                self.running = false
                self.passed = result.passed
                self.image = result.image
                self.status = result.passed
                    ? NSLocalizedString("PASS", comment: "Probe status")
                    : NSLocalizedString("FAIL", comment: "Probe status")
                self.report = "Helium Metal Probe — G01-C\niOS \(UIDevice.current.systemVersion)\n"
                    + result.report
            }
        }
    }
}

@MainActor
struct ProbeView: View {
    @StateObject private var model = ProbeModel()
    @State private var sharing = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Helium Direct Metal Test").font(.title).bold()
                Text("Standalone 256 × 256 indexed terrain and depth probe")
                HStack {
                    Button("Run Metal Test") { model.run() }
                        .buttonStyle(.borderedProminent)
                        .disabled(model.running)
                    if model.running { ProgressView() }
                    Text(model.status).bold()
                        .foregroundColor(model.passed == true ? .green : model.passed == false ? .red : .secondary)
                }
                if let image = model.image {
                    Image(uiImage: image)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 512)
                        .border(Color.secondary)
                        .accessibilityLabel(Text("Actual Metal GPU readback of the stepped block hill"))
                    Text("Inspect the hill for missing faces, wrong colors and depth errors. PASS is a smoke-check result, not a complete image oracle.")
                }
                Text(model.report)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
                Text("GPU timing covers one offscreen render + blit command buffer. It is not gameplay FPS. Readback completion does not prove physical display presentation.")
                    .font(.footnote)
                Button("Share Diagnostics and Image") { sharing = true }
                    .disabled(model.running || model.passed == nil)
            }
            .padding()
        }
        .sheet(isPresented: $sharing) {
            ActivitySheet(report: model.report, image: model.image)
        }
    }
}

private struct ActivitySheet: UIViewControllerRepresentable {
    let report: String
    let image: UIImage?

    func makeUIViewController(context: Context) -> UIActivityViewController {
        var items: [Any] = [report]
        if let image = image { items.append(image) }
        return UIActivityViewController(activityItems: items, applicationActivities: nil)
    }

    func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
