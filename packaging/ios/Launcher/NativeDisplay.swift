// MIT; Copyright (c) 2026 Horizon contributors.
import Foundation
import UIKit
import Metal
import QuartzCore

public typealias NativeWork = @convention(c) (UInt64, UnsafeMutableRawPointer?) -> Void
public typealias NativeCancel = @convention(c) (UInt64) -> Void
public typealias NativeReady = @convention(c) (UInt64) -> Bool
private var nativeObserving = false
private var nativeTargets: [UInt64: NativeSession] = [:]

@_cdecl("horizon_native_display_abi")
public func nativeDisplayABI() -> UInt32 { 1 }

// Test-only bundle marker AND a strict <=64-byte sandbox file. Default is OFF.
@_cdecl("horizon_native_display_mode")
public func nativeDisplayMode() -> Int32 {
    guard Bundle.main.object(forInfoDictionaryKey: "HeliumNativeMetalTest") as? Bool == true else { return 0 }
    let path = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0].appendingPathComponent("helium-metal-test.txt")
    guard FileManager.default.fileExists(atPath: path.path) else { return 0 }
    guard let attrs = try? FileManager.default.attributesOfItem(atPath: path.path),
          let size = attrs[.size] as? NSNumber, size.intValue <= 64,
          let file = try? FileHandle(forReadingFrom: path) else { return -1 }
    defer { try? file.close() }
    guard let data = try? file.read(upToCount: 65), data.count <= 64,
          let value = String(data: data, encoding: .utf8) else { return -1 }
    switch value { case "off", "off\n": return 0
    case "attached-one-section", "attached-one-section\n": return 2
    default: NSLog("Helium native configuration invalid; disabled"); return -1 }
}

private final class NativeMetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var invalid: (() -> Void)?
    var initialPixels = CGSize.zero
    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = window?.screen.scale ?? 0
        let size = CGSize(width: (bounds.width*scale).rounded(), height: (bounds.height*scale).rounded())
        if size != initialPixels { invalid?() } // cancel, NEVER mutate a pending drawable layout
    }
    override func didMoveToWindow() { super.didMoveToWindow(); if window == nil { invalid?() } }
}
private final class NativeSession {
    let id: UInt64
    let started = ProcessInfo.processInfo.systemUptime
    let view: NativeMetalView
    weak var parent: UIView?
    weak var window: UIWindow?
    let cancel: NativeCancel
    let ready: NativeReady
    var timer: Timer?
    init(_ id: UInt64, _ parent: UIView, _ cancel: @escaping NativeCancel, _ ready: @escaping NativeReady) {
        self.id=id; self.parent=parent; self.window=parent.window; self.cancel=cancel; self.ready=ready
        view=NativeMetalView(frame: parent.bounds)
    }
    func valid() -> Bool {
        guard ProcessInfo.processInfo.systemUptime - started < 35, let parent = parent, let window = window, parent.window === window,
              view.superview === parent, !window.isHidden, window.isKeyWindow, !parent.isHidden, UIApplication.shared.applicationState == .active,
              window.rootViewController?.presentedViewController == nil else { return false }
        return CGSize(width:(parent.bounds.width*window.screen.scale).rounded(),height:(parent.bounds.height*window.screen.scale).rounded()) == view.initialPixels
    }
    deinit { timer?.invalidate() }
}
func cancelAllNativeTargets() { for id in Array(nativeTargets.keys) { cancelNative(id) } }
private func cancelNative(_ id: UInt64) {
    guard let target=nativeTargets.removeValue(forKey:id) else { return }
    target.timer?.invalidate(); target.cancel(id); target.view.invalid=nil; target.view.removeFromSuperview()
    LauncherLog.write("native session closed \(id); wgpu fallback visible")
}
@_cdecl("horizon_native_display_request")
public func nativeDisplayRequest(_ id: UInt64, _ pointer: UnsafeMutableRawPointer?, _ work: NativeWork?, _ cancel: NativeCancel?, _ ready: NativeReady?) -> Bool {
    guard Thread.isMainThread, nativeTargets.isEmpty, let pointer=pointer, let work=work, let cancel=cancel, let ready=ready,
          let parent=Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue() as? UIView,
          let window=parent.window, !window.isHidden, window.isKeyWindow, !parent.isHidden, window.rootViewController?.presentedViewController == nil,
          UIApplication.shared.applicationState == .active else {
        LauncherLog.write("native attach unavailable: idle host and live, visible, key game view without modal required")
        return false
    }
    let pixels=CGSize(width:(parent.bounds.width*window.screen.scale).rounded(),height:(parent.bounds.height*window.screen.scale).rounded())
    guard pixels.width>0, pixels.height>0, pixels.width<=4096, pixels.height<=4096, pixels.width*pixels.height*12 <= 64*1024*1024 else {
        LauncherLog.write("native attach rejected: physical dimensions \(pixels), policy 1..4096 and <=64MiB three-drawable reservation")
        return false
    }
    guard MTLCreateSystemDefaultDevice() != nil else {
        LauncherLog.write("native attach rejected: no system Metal device")
        return false
    }
    // UIKit lifecycle notifications execute on main; no session is retained by observer.
    if !nativeObserving { nativeObserving=true
        NotificationCenter.default.addObserver(forName: UIApplication.willResignActiveNotification, object:nil, queue:.main) { _ in cancelAllNativeTargets() }
    }
    let target=NativeSession(id,parent,cancel,ready)
    let view=target.view; let metal=view.layer as! CAMetalLayer
    view.isUserInteractionEnabled=false; view.autoresizingMask=[.flexibleWidth,.flexibleHeight]; view.alpha=0
    view.initialPixels=pixels; metal.device=MTLCreateSystemDefaultDevice(); metal.pixelFormat = .bgra8Unorm
    metal.colorspace=CGColorSpace(name: CGColorSpace.linearSRGB); metal.framebufferOnly=true
    metal.allowsNextDrawableTimeout=true; metal.presentsWithTransaction=false; metal.maximumDrawableCount=3
    metal.contentsScale=window.screen.scale; metal.drawableSize=pixels
    let label=UILabel(frame:CGRect(x:8,y:8,width:parent.bounds.width-16,height:44))
    label.text=NSLocalizedString("Native diagnostic snapshot · 5 seconds · wgpu producer active",comment:"Native diagnostic")
    label.textColor = .yellow; label.backgroundColor = .black; label.numberOfLines=2; label.font = .systemFont(ofSize:12)
    view.addSubview(label)
    nativeTargets[id]=target; view.invalid={ cancelNative(id) }; parent.addSubview(view)
    target.timer=Timer.scheduledTimer(withTimeInterval:0.1,repeats:true){[weak target] _ in
        guard let target=target else {return}; if !target.valid() {cancelNative(id)}
    }
    LauncherLog.write("native layer attached generation=\(id) size=\(pixels); transparent until validated")
    Thread.detachNewThread { // strongly retains the separate layer; UIKit view stays main-owned
        autoreleasepool { withExtendedLifetime(metal) { work(id,Unmanaged.passUnretained(metal).toOpaque()) } }
    }
    return true
}
@_cdecl("horizon_native_display_publish")
public func nativeDisplayPublish(_ id:UInt64) {
    DispatchQueue.main.async {
        guard let target=nativeTargets[id], target.valid(), target.ready(id) else {cancelNative(id);return}
        target.view.alpha=1
        LauncherLog.write("native captured frame revealed generation=\(id); scanout unverified")
        DispatchQueue.main.asyncAfter(deadline:.now()+5){cancelNative(id)}
    }
}
@_cdecl("horizon_native_display_cancel")
public func nativeDisplayCancel(_ id:UInt64) { DispatchQueue.main.async {cancelNative(id)} }
