// MIT; see the repository LICENSE and preserved Horizon attribution.
import Foundation
import UIKit

enum ProbeMode: String, CaseIterable, Identifiable, Sendable {
    case g01, g02

    var id: String { rawValue }

    var title: String {
        switch self {
        case .g01:
            return NSLocalizedString("G01 — Original Direct Metal", comment: "Renderer selection")
        case .g02:
            return NSLocalizedString("G02 — Production Terrain", comment: "Renderer selection")
        }
    }

    var details: String {
        switch self {
        case .g01:
            return NSLocalizedString(
                "16-byte stepped-block hill. Original smoke checks and triangle-order comparison.",
                comment: "G01 test description"
            )
        case .g02:
            return NSLocalizedString(
                "24-byte production-format terrain. Full-image oracle, reversed triangle order and equivalent forward-depth rendering.",
                comment: "G02 test description"
            )
        }
    }

    var imageDescription: String {
        switch self {
        case .g01:
            return NSLocalizedString("G01 actual GPU readback of the stepped block hill", comment: "Image description")
        case .g02:
            return NSLocalizedString("G02 actual GPU readback of the textured terrain validation fixture", comment: "Image description")
        }
    }

    var inspection: String {
        switch self {
        case .g01:
            return NSLocalizedString(
                "Inspect the hill for missing faces, wrong colors and depth errors. G01 PASS is a smoke-check result, not a complete image oracle.",
                comment: "G01 image instructions"
            )
        case .g02:
            return NSLocalizedString(
                "G02 PASS requires all three GPU images to match the independent full-image oracle. Inspect the texture patterns, tint, lighting, cutouts and overlapping geometry. Only the first GPU image is displayed.",
                comment: "G02 image instructions"
            )
        }
    }

    var timingScope: String {
        switch self {
        case .g01:
            return NSLocalizedString(
                "GPU duration: first offscreen render + readback command buffer only; excludes compilation and the second render.",
                comment: "G01 timing scope"
            )
        case .g02:
            return NSLocalizedString(
                "GPU duration: first offscreen upload + render + readback command buffer only; excludes pipeline compilation and the other oracle renders.",
                comment: "G02 timing scope"
            )
        }
    }
}

struct ProbeResult {
    let mode: ProbeMode
    let passed: Bool
    let image: UIImage?
    let report: String
}

enum ProbeRunner {
    // Called only by the serial worker. This owns the C outputs until Rust returns;
    // no UIKit view or observable UI state is accessed from this worker.
    static func run(mode: ProbeMode) -> ProbeResult {
        guard !Thread.isMainThread else {
            return ProbeResult(
                mode: mode, passed: false, image: nil,
                report: mode.title + "\n" + NSLocalizedString("Refusing to block the main thread.", comment: "Worker error")
            )
        }
        return autoreleasepool {
            let width = Int(HELIUM_METAL_WIDTH)
            let height = Int(HELIUM_METAL_HEIGHT)
            var pixels = [UInt8](repeating: 0, count: Int(HELIUM_METAL_RGBA_BYTES))
            var message = [CChar](repeating: 0, count: 4096)
            // These distinct arrays cannot move or be accessed concurrently while
            // their exclusive unsafe-buffer scopes are active. Rust retains neither.
            let code = pixels.withUnsafeMutableBufferPointer { output in
                message.withUnsafeMutableBufferPointer { diagnostic in
                    switch mode {
                    case .g01:
                        return helium_metal_smoke_test_rgba(
                            output.baseAddress, output.count,
                            diagnostic.baseAddress, diagnostic.count
                        )
                    case .g02:
                        return helium_metal_terrain_test_rgba(
                            output.baseAddress, output.count,
                            diagnostic.baseAddress, diagnostic.count
                        )
                    }
                }
            }
            let diagnostic = message.withUnsafeBufferPointer { String(cString: $0.baseAddress!) }
            let report = mode.title + "\n"
                + String(
                    format: NSLocalizedString("Status code: %d\nReadback: %ld × %ld RGBA8Unorm\n", comment: "Probe report"),
                    code, width, height
                )
                + diagnostic + "\n" + mode.timingScope
            guard code == HELIUM_METAL_OK else {
                return ProbeResult(mode: mode, passed: false, image: nil, report: report)
            }
            // Data gives CGDataProvider its own immutable copy before the Swift
            // output array is released. CGImage retains the provider. All pixels
            // passed the selected test's alpha=255 checks, so premultiplication is exact.
            guard let provider = CGDataProvider(data: Data(pixels) as CFData),
                  let space = CGColorSpace(name: CGColorSpace.linearSRGB),
                  let cgImage = CGImage(
                    width: width, height: height, bitsPerComponent: 8, bitsPerPixel: 32,
                    bytesPerRow: width * 4, space: space,
                    bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue)
                        .union(.byteOrder32Big),
                    provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent
                  ) else {
                return ProbeResult(
                    mode: mode, passed: false, image: nil,
                    report: report + "\n" + NSLocalizedString(
                        "FAIL: GPU checks passed, but CGImage construction failed.", comment: "Image error"
                    )
                )
            }
            // Metal texture row 0 maps to image row 0; no synthetic pixels or flips.
            return ProbeResult(mode: mode, passed: true, image: UIImage(cgImage: cgImage), report: report)
        }
    }
}
