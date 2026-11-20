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
    let build=UInt64(Bundle.main.object(forInfoDictionaryKey:"CFBundleVersion") as? String ?? "") ?? 0
    let path=FileManager.default.urls(for:.documentDirectory,in:.userDomainMask)[0].appendingPathComponent("helium-metal-test.txt")
    var data:Data?=nil
    if FileManager.default.fileExists(atPath:path.path) {
        guard let file=try? FileHandle(forReadingFrom:path) else {setMetalDebugMode("INVALID");diagnosticEvent(0,2,39,build,0);return -1}
        data=try? file.read(upToCount:65);try? file.close()
        if data==nil {setMetalDebugMode("INVALID");diagnosticEvent(0,2,39,build,0);return -1}
    }
    let result=MetalDiagnosticModel.mode(bundle:Bundle.main.bundleIdentifier ?? "",marker:Bundle.main.object(forInfoDictionaryKey:"HeliumMetalDiagnosticsVersion") as? Int,legacy:Bundle.main.object(forInfoDictionaryKey:"HeliumNativeMetalTest") as? Bool == true,file:data)
    setMetalDebugMode(result.1);diagnosticEvent(0,2,result.0 < 0 ? 39 : 0,build,UInt64(max(result.0,0)))
    return result.0
}

private final class NativeMetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var invalid: ((UInt32,UInt64,UInt64) -> Void)?
    var initialPixels = CGSize.zero
    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
        let hit = super.hitTest(point, with: event)
        return hit === self ? nil : hit
    }
    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = window?.screen.scale ?? 0
        let size = CGSize(width: (bounds.width*scale).rounded(), height: (bounds.height*scale).rounded())
        if size != initialPixels { invalid?(10,MetalDiagnosticModel.pixels(Double(initialPixels.width),Double(initialPixels.height)),MetalDiagnosticModel.pixels(Double(size.width),Double(size.height))) } // cancel, NEVER mutate a pending drawable layout
    }
    override func didMoveToWindow() { super.didMoveToWindow(); if window == nil { invalid?(11,0,0) } }
}
private enum TerrainAttachment {
    case capture, inset, gameplay

    var live: Bool { self != .capture }
    var fullscreen: Bool { self != .inset }
}

private final class NativeSession {
    let id: UInt64
    let attachment: TerrainAttachment
    var live: Bool { attachment.live }
    let started = ProcessInfo.processInfo.systemUptime
    let view: NativeMetalView
    weak var parent: UIView?
    weak var window: UIWindow?
    let cancel: NativeCancel
    let ready: NativeReady
    var timer: Timer?
    init(_ id: UInt64, _ parent: UIView, _ cancel: @escaping NativeCancel, _ ready: @escaping NativeReady, attachment: TerrainAttachment) {
        self.id=id; self.parent=parent; self.window=parent.window; self.cancel=cancel; self.ready=ready
        self.attachment = attachment
        view = NativeMetalView(frame: nativeTerrainRect(parent.bounds, attachment: attachment))
    }
    func invalidCause() -> UInt32 {
        let size = parent.map { p in
            let rect = nativeTerrainRect(p.bounds, attachment: attachment)
            let scale = window?.screen.scale ?? 0
            return CGSize(width: (rect.width * scale).rounded(), height: (rect.height * scale).rounded())
        }
        return MetalDiagnosticModel.invalidHost(age:live ? 0 : ProcessInfo.processInfo.systemUptime-started,parent:parent != nil,window:window != nil,attached:parent?.window === window && view.superview === parent,hidden:window?.isHidden != false || parent?.isHidden != false,key:window?.isKeyWindow == true,active:UIApplication.shared.applicationState == .active,modal:window?.rootViewController?.presentedViewController != nil,size:size==view.initialPixels)
    }
    deinit { timer?.invalidate() }
}
func cancelAllNativeTargets(_ cause:UInt32=13) { diagnosticEvent(0,cause==13 ? 5 : 30,cause);for id in Array(nativeTargets.keys) { cancelNative(id,cause) } }
func nativeDisplayIsIdle()->Bool {nativeTargets.isEmpty}
private func cancelNative(_ id: UInt64,_ cause:UInt32=16,_ expected:UInt64=0,_ observed:UInt64=0) {
    var old=expected;var new=observed
    if old==0 && new==0, let target=nativeTargets[id] {
        if cause==15 {old=35000;new=UInt64(max(0,(ProcessInfo.processInfo.systemUptime-target.started)*1000))}
        else if cause==10 {old=MetalDiagnosticModel.pixels(Double(target.view.initialPixels.width),Double(target.view.initialPixels.height));if let parent=target.parent {new=MetalDiagnosticModel.pixels(Double((parent.bounds.width*(target.window?.screen.scale ?? 0)).rounded()),Double((parent.bounds.height*(target.window?.screen.scale ?? 0)).rounded()))}}
        else if [UInt32(11),12,13,14,21,22,23].contains(cause) {old=1}
    }
    diagnosticEvent(id,cause==20 ? 29 : 30,cause,old,new)
    guard let target=nativeTargets.removeValue(forKey:id) else { return }
    target.timer?.invalidate(); target.cancel(id); target.view.invalid=nil; target.view.removeFromSuperview()
    diagnosticEvent(id,29,0,UInt64(cause),0)
    LauncherLog.write("native session closed \(id) mode=\(target.attachment)")
}
private func nativeTerrainRect(_ bounds: CGRect, attachment: TerrainAttachment) -> CGRect {
    guard !attachment.fullscreen else { return bounds }
    let size = CGSize(width: bounds.width / 2, height: bounds.height / 2)
    return CGRect(x: bounds.maxX - size.width, y: bounds.minY, width: size.width, height: size.height)
}

@_cdecl("horizon_native_display_request_live")
public func nativeDisplayRequestLive(_ id: UInt64, _ pointer: UnsafeMutableRawPointer?, _ work: NativeWork?, _ cancel: NativeCancel?, _ ready: NativeReady?) -> Bool {
    guard Bundle.main.bundleIdentifier == "dev.helium.g06ctest",
          Bundle.main.object(forInfoDictionaryKey: "HeliumMetalDiagnosticsVersion") as? Int == 2 else { return false }
    return nativeDisplayRequestImpl(id, pointer, work, cancel, ready, attachment: .inset)
}
@_cdecl("horizon_native_display_gameplay_supported")
public func nativeDisplayGameplaySupported() -> Bool {
    Bundle.main.bundleIdentifier == "dev.helium.g06ctest" &&
        Bundle.main.object(forInfoDictionaryKey: "HeliumMetalDiagnosticsVersion") as? Int == 2
}

@_cdecl("horizon_native_display_request_gameplay")
public func nativeDisplayRequestGameplay(_ id: UInt64, _ pointer: UnsafeMutableRawPointer?, _ work: NativeWork?, _ cancel: NativeCancel?, _ ready: NativeReady?) -> Bool {
    guard nativeDisplayGameplaySupported() else { return false }
    return nativeDisplayRequestImpl(id, pointer, work, cancel, ready, attachment: .gameplay)
}
@_cdecl("horizon_native_display_request")
public func nativeDisplayRequest(_ id: UInt64, _ pointer: UnsafeMutableRawPointer?, _ work: NativeWork?, _ cancel: NativeCancel?, _ ready: NativeReady?) -> Bool {
    nativeDisplayRequestImpl(id, pointer, work, cancel, ready, attachment: .capture)
}

private func nativeDisplayRequestImpl(_ id: UInt64, _ pointer: UnsafeMutableRawPointer?, _ work: NativeWork?, _ cancel: NativeCancel?, _ ready: NativeReady?, attachment: TerrainAttachment) -> Bool {
    guard Thread.isMainThread, nativeTargets.isEmpty, let pointer=pointer, let work=work, let cancel=cancel, let ready=ready else {diagnosticEvent(id,30,38);return false}
    guard let parent=Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue() as? UIView else {diagnosticEvent(id,30,21);return false}
    guard let window=parent.window else {diagnosticEvent(id,30,22);return false}
    let hostCause=MetalDiagnosticModel.invalidHost(age:0,parent:true,window:true,attached:true,hidden:window.isHidden || parent.isHidden,key:window.isKeyWindow,active:UIApplication.shared.applicationState == .active,modal:window.rootViewController?.presentedViewController != nil,size:true)
    guard hostCause==0 else {diagnosticEvent(id,30,hostCause);return false}
    let rect = nativeTerrainRect(parent.bounds, attachment: attachment)
    let pixels = CGSize(width: (rect.width * window.screen.scale).rounded(), height: (rect.height * window.screen.scale).rounded())
    guard pixels.width>0, pixels.height>0, pixels.width<=4096, pixels.height<=4096, pixels.width*pixels.height*12 <= 64*1024*1024 else {
        diagnosticEvent(id,30,10,0,MetalDiagnosticModel.pixels(Double(pixels.width),Double(pixels.height)))
        LauncherLog.write("native attach rejected: physical dimensions \(pixels), policy 1..4096 and <=64MiB three-drawable reservation")
        return false
    }
    guard MTLCreateSystemDefaultDevice() != nil else {
        diagnosticEvent(id,30,28)
        LauncherLog.write("native attach rejected: no system Metal device")
        return false
    }
    // UIKit lifecycle notifications execute on main; no session is retained by observer.
    if !nativeObserving { nativeObserving=true
        NotificationCenter.default.addObserver(forName: UIApplication.willResignActiveNotification, object:nil, queue:.main) { _ in cancelAllNativeTargets(14) }
        NotificationCenter.default.addObserver(forName:UIApplication.didReceiveMemoryWarningNotification,object:nil,queue:.main){_ in cancelAllNativeTargets(34)}
    }
    if attachment == .inset { setMetalDebugMode("LIVE-TERRAIN") }
    if attachment == .gameplay { setMetalDebugMode("NATIVE-GAMEPLAY-TERRAIN") }
    let target=NativeSession(id,parent,cancel,ready,attachment:attachment)
    let view=target.view; let metal=view.layer as! CAMetalLayer
    view.isUserInteractionEnabled = attachment == .gameplay
    view.autoresizingMask = !attachment.fullscreen ? [.flexibleLeftMargin, .flexibleWidth, .flexibleHeight] : [.flexibleWidth, .flexibleHeight]; view.alpha=0
    view.initialPixels=pixels; metal.device=MTLCreateSystemDefaultDevice(); metal.pixelFormat = .bgra8Unorm
    metal.colorspace=CGColorSpace(name: CGColorSpace.linearSRGB); metal.framebufferOnly=true
    metal.allowsNextDrawableTimeout=true; metal.presentsWithTransaction=false; metal.maximumDrawableCount=3
    metal.contentsScale=window.screen.scale; metal.drawableSize=pixels
    let label=UILabel(frame:CGRect(x:8,y:8,width:rect.width-16,height:44))
    switch attachment {
    case .capture:
        label.text = NSLocalizedString("HELIUM S2A · HISTORICAL NATIVE TERRAIN · OPAQUE/CUTOUT\nwgpu producer/fallback active",comment:"Native diagnostic")
    case .inset:
        label.text = "HELIUM S2B · LIVE NATIVE OPAQUE/CUTOUT\nwgpu reference and input remain active"
    case .gameplay:
        label.text = "HELIUM S2D · DIRECT METAL TERRAIN\nOpaque and cutout only"
    }
    label.textColor = .yellow; label.backgroundColor = .black; label.numberOfLines=2; label.font = .systemFont(ofSize:12)
    view.addSubview(label)
    if attachment == .gameplay {
        let close = UIButton(type: .system)
        close.setTitle("Save & Quit", for: .normal)
        close.tintColor = .white
        close.backgroundColor = UIColor.black.withAlphaComponent(0.85)
        close.translatesAutoresizingMaskIntoConstraints = false
        close.addAction(UIAction { [weak close] _ in
            close?.isEnabled = false
            Task { @MainActor in
                let answer = await Bridge.send("native-terrain-save-quit")
                if answer["phase"] as? String != "saving" {
                    close?.isEnabled = true
                }
            }
        }, for: .touchUpInside)
        view.addSubview(close)
        NSLayoutConstraint.activate([
            close.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -12),
            close.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 12),
            close.heightAnchor.constraint(greaterThanOrEqualToConstant: 44),
            close.widthAnchor.constraint(greaterThanOrEqualToConstant: 100)
        ])
        let crosshair = UILabel()
        crosshair.text = "+"
        crosshair.textColor = .white
        crosshair.font = .monospacedSystemFont(ofSize: 20, weight: .semibold)
        crosshair.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(crosshair)
        NSLayoutConstraint.activate([
            crosshair.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            crosshair.centerYAnchor.constraint(equalTo: view.centerYAnchor)
        ])
    }
    nativeTargets[id]=target; view.invalid={ cause,old,new in cancelNative(id,cause,old,new) }; parent.addSubview(view)
    diagnosticEvent(id,14,0,UInt64(pixels.width),UInt64(pixels.height));liftMetalHUD()
    target.timer=Timer.scheduledTimer(withTimeInterval:0.1,repeats:true){[weak target] _ in
        guard let target=target else {return}
        let cause=target.invalidCause()
        if cause != 0 { cancelNative(id,cause) }
        else if target.live && target.view.alpha != 0 && !target.ready(id) { cancelNative(id,16) }
    }
    LauncherLog.write("native layer attached generation=\(id) size=\(pixels); transparent until validated")
    Thread.detachNewThread {
        diagnosticEvent(id,15) // strongly retains the separate layer; UIKit view stays main-owned
        autoreleasepool { withExtendedLifetime(metal) { work(id,Unmanaged.passUnretained(metal).toOpaque()) } }
    }
    return true
}
@_cdecl("horizon_native_display_publish")
public func nativeDisplayPublish(_ id:UInt64) {
    DispatchQueue.main.async {
        guard let target=nativeTargets[id] else {diagnosticEvent(id,30,16);return}
        let cause=target.invalidCause();guard cause==0, target.ready(id) else {cancelNative(id,cause==0 ? 16 : cause);return}
        if target.live {
            target.view.alpha = 1
            LauncherLog.write("live native terrain revealed mode=\(target.attachment); physical scanout unverified")
            return
        }
        let values=diagnosticValues()
        if values.count==32, let label=target.view.subviews.first as? UILabel {
            label.text="HELIUM S2A · HISTORICAL NATIVE TERRAIN · OPAQUE/CUTOUT\nCapture rev \(values[20]), current rev \(values[29]) · 5 s · wgpu producer/fallback active"
        }
        target.view.alpha=1
        diagnosticEvent(id,28)
        LauncherLog.write("native captured frame revealed generation=\(id); scanout unverified")
        DispatchQueue.main.asyncAfter(deadline:.now()+5){cancelNative(id,20)}
    }
}
@_cdecl("horizon_native_display_cancel")
public func nativeDisplayCancel(_ id:UInt64) { DispatchQueue.main.async {cancelNative(id)} }
