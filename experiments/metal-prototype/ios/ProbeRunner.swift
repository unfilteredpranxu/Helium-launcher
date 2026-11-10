// MIT; see the repository LICENSE and preserved Horizon attribution.
import Foundation
import UIKit

struct ProbeResult {
    let passed: Bool
    let image: UIImage?
    let report: String
}

enum ProbeRunner {
    // Called only by the serial worker. This owns the C outputs until Rust returns;
    // no UIKit view or observable UI state is accessed from this worker.
    static func run() -> ProbeResult {
        guard !Thread.isMainThread else {
            return ProbeResult(passed: false, image: nil, report: "Refusing to block the main thread.")
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
                    helium_metal_smoke_test_rgba(
                        output.baseAddress, output.count,
                        diagnostic.baseAddress, diagnostic.count
                    )
                }
            }
            let diagnostic = message.withUnsafeBufferPointer { String(cString: $0.baseAddress!) }
            let report = "Status code: \(code)\nReadback: \(width) × \(height) RGBA8Unorm\n\(diagnostic)"
            guard code == HELIUM_METAL_OK else {
                return ProbeResult(passed: false, image: nil, report: report)
            }
            // Data gives CGDataProvider its own immutable copy before the Swift
            // output array is released. CGImage retains the provider. All pixels
            // passed the existing alpha=255 smoke check, so premultiplication is exact.
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
                    passed: false, image: nil,
                    report: report + "\nFAIL: GPU smoke checks passed, but CGImage construction failed."
                )
            }
            // Metal texture row 0 maps to image row 0; no synthetic pixels or flips.
            return ProbeResult(passed: true, image: UIImage(cgImage: cgImage), report: report)
        }
    }
}
