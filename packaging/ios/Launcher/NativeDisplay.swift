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
    let path=FileManager.default.urls(for:.documentDirectory,in:.userDomainMask)[0].appendingPathComponent("helium-metal-test.txt")
    var data:Data?=nil
    if FileManager.default.fileExists(atPath:path.path) {
        guard let file=try? FileHandle(forReadingFrom:path) else {setMetalDebugMode("INVALID");return -1}
        data=try? file.read(upToCount:65);try? file.close()
        if data==nil {setMetalDebugMode("INVALID");return -1}
    }
    let result=MetalDiagnosticModel.mode(bundle:Bundle.main.bundleIdentifier ?? "",marker:Bundle.main.object(forInfoDictionaryKey:"HeliumMetalDiagnosticsVersion") as? Int,legacy:Bundle.main.object(forInfoDictionaryKey:"HeliumNativeMetalTest") as? Bool == true,file:data)
    setMetalDebugMode(result.1);diagnosticEvent(0,2,result.0 < 0 ? 39 : 0,0,UInt64(max(result.0,0)))
    return result.0
}

private final class NativeMetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }
    var invalid: ((UInt32,UInt64,UInt64) -> Void)?
    var initialPixels = CGSize.zero
    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = window?.screen.scale ?? 0
        let size = CGSize(width: (bounds.width*scale).rounded(), height: (bounds.height*scale).rounded())
        if size != initialPixels { invalid?(10,MetalDiagnosticModel.pixels(Double(initialPixels.width),Double(initialPixels.height)),MetalDiagnosticModel.pixels(Double(size.width),Double(size.height))) } // cancel, NEVER mutate a pending drawable layout
    }
    override func didMoveToWindow() { super.didMoveToWindow(); if window == nil { invalid?(11,0,0) } }
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
    func invalidCause() -> UInt32 {
        let size=parent.map {p in CGSize(width:(p.bounds.width*(window?.screen.scale ?? 0)).rounded(),height:(p.bounds.height*(window?.screen.scale ?? 0)).rounded())}
        return MetalDiagnosticModel.invalidHost(age:ProcessInfo.processInfo.systemUptime-started,parent:parent != nil,window:window != nil,attached:parent?.window === window && view.superview === parent,hidden:window?.isHidden != false || parent?.isHidden != false,key:window?.isKeyWindow == true,active:UIApplication.shared.applicationState == .active,modal:window?.rootViewController?.presentedViewController != nil,size:size==view.initialPixels)
    }
    deinit { timer?.invalidate() }
}
func cancelAllNativeTargets(_ cause:UInt32=13) { diagnosticEvent(0,cause==13 ? 5 : 30,cause);for id in Array(nativeTargets.keys) { cancelNative(id,cause) } }
func nativeDisplayIsIdle()->Bool {nativeTargets.isEmpty}
private func cancelNative(_ id: UInt64,_ cause:UInt32=16,_ expected:UInt64=0,_ observed:UInt64=0) {
    diagnosticEvent(id,cause==20 ? 29 : 30,cause,expected,observed)
    guard let target=nativeTargets.removeValue(forKey:id) else { return }
    target.timer?.invalidate(); target.cancel(id); target.view.invalid=nil; target.view.removeFromSuperview()
    diagnosticEvent(id,29,0,UInt64(cause),0)
    LauncherLog.write("native session closed \(id); wgpu fallback visible")
}
@_cdecl("horizon_native_display_request")
public func nativeDisplayRequest(_ id: UInt64, _ pointer: UnsafeMutableRawPointer?, _ work: NativeWork?, _ cancel: NativeCancel?, _ ready: NativeReady?) -> Bool {
    guard Thread.isMainThread, nativeTargets.isEmpty, let pointer=pointer, let work=work, let cancel=cancel, let ready=ready else {diagnosticEvent(id,30,38);return false}
    guard let parent=Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue() as? UIView else {diagnosticEvent(id,30,21);return false}
    guard let window=parent.window else {diagnosticEvent(id,30,22);return false}
    let hostCause=MetalDiagnosticModel.invalidHost(age:0,parent:true,window:true,attached:true,hidden:window.isHidden || parent.isHidden,key:window.isKeyWindow,active:UIApplication.shared.applicationState == .active,modal:window.rootViewController?.presentedViewController != nil,size:true)
    guard hostCause==0 else {diagnosticEvent(id,30,hostCause);return false}
    let pixels=CGSize(width:(parent.bounds.width*window.screen.scale).rounded(),height:(parent.bounds.height*window.screen.scale).rounded())
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
    nativeTargets[id]=target; view.invalid={ cause,old,new in cancelNative(id,cause,old,new) }; parent.addSubview(view)
    diagnosticEvent(id,14,0,UInt64(pixels.width),UInt64(pixels.height));liftMetalHUD()
    target.timer=Timer.scheduledTimer(withTimeInterval:0.1,repeats:true){[weak target] _ in
        guard let target=target else {return}; let cause=target.invalidCause();if cause != 0 {cancelNative(id,cause)}
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
        target.view.alpha=1
        diagnosticEvent(id,28)
        LauncherLog.write("native captured frame revealed generation=\(id); scanout unverified")
        DispatchQueue.main.asyncAfter(deadline:.now()+5){cancelNative(id,20)}
    }
}
@_cdecl("horizon_native_display_cancel")
public func nativeDisplayCancel(_ id:UInt64) { DispatchQueue.main.async {cancelNative(id)} }
