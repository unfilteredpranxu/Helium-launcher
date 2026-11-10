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
    @Published private(set) var selectedMode: ProbeMode = .g01
    @Published private(set) var runningMode: ProbeMode?
    @Published private(set) var results: [ProbeMode: ProbeResult] = [:]
    private let worker = DispatchQueue(label: "dev.helium.metalprobe.worker", qos: .userInitiated)

    var running: Bool { runningMode != nil }

    // Hide a previous image/status while that mode is being rerun. Other modes
    // retain their independent results for this app session only.
    var selectedResult: ProbeResult? {
        runningMode == selectedMode ? nil : results[selectedMode]
    }

    var report: String {
        if running {
            return selectedMode.title + "\n" + NSLocalizedString(
                "Waiting for the worker and GPU. A stalled driver has no timeout; close this test app if necessary.",
                comment: "Blocking probe instructions"
            )
        }
        guard let result = selectedResult else {
            return selectedMode.title + "\n" + NSLocalizedString(
                "Not run in this app session. Run this renderer to obtain its result and actual GPU image.",
                comment: "Initial instructions"
            )
        }
        return "Helium Metal Probe — G02-C\niOS \(UIDevice.current.systemVersion)\n" + result.report
    }

    func status(for mode: ProbeMode) -> String {
        if runningMode == mode { return NSLocalizedString("Running", comment: "Probe status") }
        guard let result = results[mode] else { return NSLocalizedString("Not run", comment: "Probe status") }
        return result.passed
            ? NSLocalizedString("PASS", comment: "Probe status")
            : NSLocalizedString("FAIL", comment: "Probe status")
    }

    func select(_ mode: ProbeMode) {
        guard !running else { return }
        selectedMode = mode
    }

    func run() {
        guard !running else { return }
        // Capture the selection before dispatch; completion never derives its
        // identity from mutable UI selection.
        let mode = selectedMode
        runningMode = mode
        worker.async {
            let result = ProbeRunner.run(mode: mode)
            DispatchQueue.main.async {
                self.results[result.mode] = result
                self.runningMode = nil
            }
        }
    }
}

@MainActor
struct ProbeView: View {
    @StateObject private var model = ProbeModel()
    @State private var sharePayload: SharePayload?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Helium Direct Metal Test").font(.title).bold()
                Text("Standalone 256 × 256 indexed terrain and depth probe")
                Group {
                    Picker("Renderer", selection: Binding(
                        get: { model.selectedMode }, set: { model.select($0) }
                    )) {
                        ForEach(ProbeMode.allCases) { mode in
                            Text(mode.title).tag(mode)
                        }
                    }
                    .pickerStyle(.menu)
                    .disabled(model.running)
                    Text(model.selectedMode.title).font(.headline)
                    Text(model.selectedMode.details)
                    ForEach(ProbeMode.allCases) { mode in
                        HStack {
                            Text(mode.title)
                            Spacer()
                            Text(model.status(for: mode)).bold()
                                .foregroundColor(statusColor(for: mode))
                        }
                    }
                }
                HStack {
                    Button("Run Metal Test") { model.run() }
                        .buttonStyle(.borderedProminent)
                        .disabled(model.running)
                    if model.running { ProgressView() }
                }
                if let result = model.selectedResult, let image = result.image {
                    Text(result.mode.title).font(.headline)
                    Image(uiImage: image)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 512)
                        .border(Color.secondary)
                        .accessibilityLabel(Text(result.mode.imageDescription))
                    Text(result.mode.inspection)
                }
                Text(model.report)
                    .font(.system(.body, design: .monospaced))
                    .textSelection(.enabled)
                Text("Results apply only to this run and device. GPU duration is not gameplay FPS. Readback completion does not prove physical display presentation.")
                    .font(.footnote)
                Button("Share Diagnostics and Image") {
                    if let result = model.selectedResult {
                        sharePayload = SharePayload(report: model.report, image: result.image)
                    }
                }
                .disabled(model.running || model.selectedResult == nil)
            }
            .padding()
        }
        .sheet(item: $sharePayload) { payload in
            ActivitySheet(report: payload.report, image: payload.image)
        }
    }

    private func statusColor(for mode: ProbeMode) -> Color {
        guard model.runningMode != mode, let result = model.results[mode] else { return .secondary }
        return result.passed ? .green : .red
    }
}

// Freeze the matching diagnostics and image while the share sheet is presented.
private struct SharePayload: Identifiable {
    let id = UUID()
    let report: String
    let image: UIImage?
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
