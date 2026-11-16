// MIT; Copyright (c) 2026 Horizon contributors.
import SwiftUI
import UIKit
import Metal
import QuartzCore

// One-shot native display request. The dedicated Thread owns all Rust/Metal
// session calls. Main-thread lifecycle cancellation touches only atomic tokens.
final class DisplayView: UIView {
    private var target: CAMetalLayer?
    private var handle: UInt64 = 0
    private var requested: CGSize = .zero
    private var timer: Timer?
    private let label = UILabel()
    override init(frame: CGRect) {
        super.init(frame: frame)
        label.textColor = .white; label.backgroundColor = .black
        label.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        addSubview(label)
        NotificationCenter.default.addObserver(self, selector: #selector(stop), name: UIApplication.didEnterBackgroundNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(resume), name: UIApplication.didBecomeActiveNotification, object: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) unsupported") }
    @objc func stop() {
        if handle != 0 { helium_display_cancel(handle); helium_display_release(handle) }
        handle = 0; timer?.invalidate(); timer = nil
        target?.removeFromSuperlayer(); target = nil; requested = .zero
    }
    @objc private func resume() { setNeedsLayout() }
    override func didMoveToWindow() { super.didMoveToWindow(); if window == nil { stop() } else { setNeedsLayout() } }
    override func layoutSubviews() {
        super.layoutSubviews()
        label.frame = CGRect(x: 0, y: bounds.height-24, width: bounds.width, height: 24)
        guard window != nil, UIApplication.shared.applicationState != .background else { return }
        let scale = window?.screen.scale ?? 1
        let size = CGSize(width: (bounds.width*scale).rounded(), height: (bounds.height*scale).rounded())
        guard size.width > 0, size.height > 0 else { stop(); return }
        guard size != requested else { return }
        stop()
        let token = helium_display_create()
        guard token != 0 else {
            label.text = NSLocalizedString("Previous display request retiring; retrying", comment: "Display status")
            DispatchQueue.main.asyncAfter(deadline: .now()+0.2) { [weak self] in self?.setNeedsLayout() }
            return
        }
        let metal = CAMetalLayer()
        metal.device = MTLCreateSystemDefaultDevice()
        metal.pixelFormat = .bgra8Unorm
        metal.colorspace = CGColorSpace(name: CGColorSpace.linearSRGB)
        metal.framebufferOnly = true; metal.isOpaque = true
        metal.allowsNextDrawableTimeout = true; metal.presentsWithTransaction = false
        metal.maximumDrawableCount = 3; metal.contentsScale = scale
        metal.frame = bounds; metal.drawableSize = size
        layer.insertSublayer(metal, at: 0)
        target = metal; handle = token; requested = size
        label.text = NSLocalizedString("Initializing native Metal display", comment: "Display status")
        timer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { [weak self] _ in
            guard let self = self, self.handle == token else { return }
            let state = helium_display_status(token)
            self.label.text = NSLocalizedString([1:"Initializing",2:"Submitted",3:"GPU completed; scanout unverified",4:"Drawable unavailable",5:"Failed or cancelled"] [Int(state)] ?? "Released", comment: "Display status")
        }
        Thread.detachNewThread {
            autoreleasepool {
                _ = helium_display_run(token, Unmanaged.passUnretained(metal).toOpaque())
                // Captured metal is strongly owned throughout the synchronous call.
                // A cancellation allows Rust to retire and release the token.
            }
        }
    }
    deinit { timer?.invalidate(); if handle != 0 { helium_display_cancel(handle); helium_display_release(handle) }; NotificationCenter.default.removeObserver(self) }
}
struct NativeDisplayProbe: UIViewRepresentable {
    func makeUIView(context: Context) -> DisplayView { DisplayView() }
    func updateUIView(_ view: DisplayView, context: Context) {}
    static func dismantleUIView(_ view: DisplayView, coordinator: ()) { view.stop() }
}
