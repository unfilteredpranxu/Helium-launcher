import SwiftUI
import UIKit
import UniformTypeIdentifiers

/// An installed mod, as `mods-list` reports it (crates/horizon-client/src/game/mods_control.rs).
struct InstalledMod: Identifiable, Equatable {
    let file: String
    let modID: String
    let name: String
    let version: String
    let description: String
    let api: String
    let enabled: Bool
    /// Whether the package carries `mod.wasm`, which does not run on iOS.
    let code: Bool
    let problem: String?
    var id: String { file }

    init?(_ json: [String: Any]) {
        guard let file = json["file"] as? String else { return nil }
        self.file = file
        modID = json["id"] as? String ?? ""
        name = json["name"] as? String ?? ""
        version = json["version"] as? String ?? ""
        description = json["description"] as? String ?? ""
        api = json["api"] as? String ?? ""
        enabled = json["enabled"] as? Bool ?? false
        code = json["code"] as? Bool ?? false
        problem = json["problem"] as? String
    }
}

/// The mods the game has, and the lines that change them. Installs are staged in
/// `Documents/.horizon-import` first, so the game only ever moves a file it has validated.
@MainActor
final class ModStore: ObservableObject {
    @Published private(set) var rows: [InstalledMod] = []
    @Published private(set) var folder = GameFolder.mods.path
    @Published private(set) var loading = false
    @Published private(set) var installing = false
    @Published var failed: String?
    @Published var notice: String?

    func reload() async {
        loading = true
        defer { loading = false }
        let answer = await Bridge.send("mods-list")
        if let error = answer.error {
            failed = error
            return
        }
        if let folder = answer["folder"] as? String {
            self.folder = folder
        }
        rows = (answer["mods"] as? [[String: Any]] ?? []).compactMap(InstalledMod.init)
    }

    func install(urls: [URL]) async {
        guard !installing else { return }
        installing = true
        failed = nil
        notice = nil
        defer { installing = false }
        var installed = 0
        var problems: [String] = []
        for url in urls {
            let name = url.lastPathComponent
            guard name.lowercased().hasSuffix(".hmod") else {
                problems.append("\(name) is not a .hmod content mod. Native .hnmod mods are compiled into the game and cannot be installed on iOS.")
                continue
            }
            do {
                let staged = try await stage(url)
                defer { try? FileManager.default.removeItem(at: staged) }
                let answer = await Bridge.send(command("mods-install", ["path": staged.path]))
                if let error = answer.error {
                    problems.append("\(name): \(error)")
                } else {
                    installed += 1
                }
            } catch {
                problems.append("\(name): \(error.localizedDescription)")
            }
        }
        await reload()
        if installed > 0 {
            notice = installed == 1
                ? "Installed. Quit the game and press Play to load it."
                : "Installed \(installed) mods. Quit the game and press Play to load them."
            Haptics.done()
        }
        if !problems.isEmpty {
            failed = problems.joined(separator: "\n")
            Haptics.failed()
        }
    }

    /// A copy of the picked file inside the sandbox. The copy runs off the main actor: a mod may be
    /// tens of megabytes, and the picker's security scope must stay open until it finishes.
    private func stage(_ url: URL) async throws -> URL {
        let accessing = url.startAccessingSecurityScopedResource()
        defer { if accessing { url.stopAccessingSecurityScopedResource() } }
        let folder = GameFolder.imports
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let staged = folder.appendingPathComponent(UUID().uuidString).appendingPathExtension("hmod")
        try await Task.detached(priority: .userInitiated) {
            try? FileManager.default.removeItem(at: staged)
            try FileManager.default.copyItem(at: url, to: staged)
        }.value
        return staged
    }

    func set(_ row: InstalledMod, on: Bool) async {
        guard !row.modID.isEmpty else { return }
        let answer = await Bridge.send(command("mods-set", ["id": row.modID, "on": on]))
        if let error = answer.error {
            failed = error
            Haptics.failed()
        } else {
            Haptics.tap()
        }
        await reload()
    }

    func remove(_ row: InstalledMod) async {
        let answer = await Bridge.send(command("mods-delete", ["file": row.file]))
        if let error = answer.error {
            failed = error
            Haptics.failed()
        } else {
            Haptics.tap()
        }
        await reload()
    }

    private func command(_ name: String, _ arguments: [String: Any]) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: arguments),
              let json = String(data: data, encoding: .utf8) else { return name }
        return "\(name) \(json)"
    }
}

// MARK: - The page

struct ModsPage: View {
    @StateObject private var store = ModStore()
    @State private var picking = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            SectionHeader(title: "Installed")
            Text("⚠️ .hmod files that carry code (a mod.wasm) do not run on iOS — only content mods work.")
                .font(Theme.sans(11)).foregroundStyle(Theme.textFaint)
            if store.loading && store.rows.isEmpty {
                ProgressView().tint(Theme.accentText).frame(maxWidth: .infinity).padding(24)
            } else if store.rows.isEmpty {
                EmptyNote(icon: "puzzlepiece.extension",
                          text: "No mods installed. Install a .hmod content mod here, or drop one into Documents/horizon-mods from the Files app.")
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(store.rows.enumerated()), id: \.element.id) { index, row in
                        ModRow(row: row, store: store)
                        if index < store.rows.count - 1 {
                            Divider().overlay(Theme.border)
                        }
                    }
                }
                .card()
            }
            if let failed = store.failed {
                Label(failed, systemImage: "exclamationmark.triangle")
                    .font(Theme.sans(12)).foregroundStyle(Theme.warn)
            }
            if let notice = store.notice {
                Label(notice, systemImage: "checkmark.circle")
                    .font(Theme.sans(12)).foregroundStyle(Theme.text)
            }
            PrimaryButton(title: store.installing ? "Installing…" : "Install .hmod",
                          icon: "square.and.arrow.down") { picking = true }
                .disabled(store.installing)
                .accessibilityIdentifier("mods-install-button")
            SecondaryButton(title: "Open Mod Folder", icon: "folder") {
                FilesOpener.open(URL(fileURLWithPath: store.folder, isDirectory: true))
            }
            .accessibilityIdentifier("mods-open-folder")
            Text("Changes apply after Quit Game → Play, or after reopening the app. Native .hnmod mods are compiled into the game and cannot be installed here.")
                .font(Theme.sans(11)).foregroundStyle(Theme.textFaint)
        }
        .fileImporter(isPresented: $picking, allowedContentTypes: [.data, .item],
                      allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls): Task { await store.install(urls: urls) }
            case .failure(let error): store.failed = error.localizedDescription
            }
        }
        .task { await store.reload() }
    }
}

private struct ModRow: View {
    let row: InstalledMod
    @ObservedObject var store: ModStore

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(row.name.isEmpty ? row.file : row.name)
                    .font(Theme.sans(15, .semiBold)).foregroundStyle(Theme.text)
                Text(subtitle).font(Theme.mono(11)).foregroundStyle(Theme.textMuted)
                if !row.description.isEmpty {
                    Text(row.description).font(Theme.sans(12)).foregroundStyle(Theme.textMuted).lineLimit(3)
                }
                if let problem = row.problem {
                    Label(problem, systemImage: "exclamationmark.triangle")
                        .font(Theme.sans(11)).foregroundStyle(Theme.warn)
                }
            }
            Spacer(minLength: 8)
            Toggle("", isOn: Binding(get: { row.enabled },
                                     set: { on in Task { await store.set(row, on: on) } }))
                .labelsHidden()
                .disabled(row.problem != nil || row.modID.isEmpty)
                .accessibilityLabel(row.enabled ? "Disable \(row.name)" : "Enable \(row.name)")
                .accessibilityIdentifier("mods-toggle-\(row.file)")
        }
        .padding(14)
        .accessibilityElement(children: .contain)
        .contextMenu {
            Button(role: .destructive) { Task { await store.remove(row) } } label: {
                Label("Delete", systemImage: "trash")
            }
        }
    }

    private var subtitle: String {
        guard !row.modID.isEmpty else { return row.file }
        return row.version.isEmpty ? row.modID : "\(row.modID) · \(row.version)"
    }
}

// MARK: - Opening the folder

/// Shows the mods folder in the Files app, or in the system folder browser when the Files app's
/// private `shareddocuments://` scheme is refused.
enum FilesOpener {
    private static let pickerDelegate = FolderPickerDelegate()

    static func open(_ folder: URL) {
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        guard let encoded = folder.standardizedFileURL.path.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed),
              let url = URL(string: "shareddocuments://" + encoded)
        else {
            pick(folder)
            return
        }
        UIApplication.shared.open(url, options: [:]) { opened in
            if !opened { pick(folder) }
        }
    }

    /// The documented way to point the system browser at a folder: a picker opened there, `asCopy: false`.
    private static func pick(_ folder: URL) {
        DispatchQueue.main.async {
            guard let host = gameViewController() else { return }
            let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
            picker.directoryURL = folder
            picker.delegate = pickerDelegate
            host.present(picker, animated: true)
        }
    }
}

private final class FolderPickerDelegate: NSObject, UIDocumentPickerDelegate {
    /// The browser is only a way to see the folder; a picked file (or Cancel) closes it.
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        controller.dismiss(animated: true)
    }

    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        controller.dismiss(animated: true)
    }
}

/// The symbol `horizon-ios-launcher::open_folder` looks up (crates/horizon-ios-launcher/src/lib.rs).
/// The game calls it from its own event loop on the main thread; the browser is opened on the next turn.
@_cdecl("horizon_open_folder")
public func horizonOpenFolder(_ path: UnsafePointer<CChar>?) -> Bool {
    guard let path else { return false }
    let folder = URL(fileURLWithPath: String(cString: path), isDirectory: true)
    DispatchQueue.main.async { FilesOpener.open(folder) }
    return true
}
