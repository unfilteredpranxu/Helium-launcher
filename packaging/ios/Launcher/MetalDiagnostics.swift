// MIT; Copyright (c) 2026 Horizon contributors.
import UIKit
private var eventFunction: MetalDiagnosticEvent?
private var snapshotFunction: MetalDiagnosticSnapshot?
private var debugHUD: UILabel?
private weak var debugParent: UIView?
private var debugTimer: Timer?
private var debugMode = "DIAGNOSTIC"
private var lastHUD = ""
func diagnosticEvent(_ id:UInt64,_ stage:UInt32,_ cause:UInt32=0,_ expected:UInt64=0,_ observed:UInt64=0) {
    _ = eventFunction?(id,stage,cause,expected,observed)
}
func diagnosticValues() -> [UInt64] {
    var values=[UInt64](repeating:0,count:32)
    let result=values.withUnsafeMutableBufferPointer { snapshotFunction?($0.baseAddress,32) }
    return result == 2 ? values : []
}
@_cdecl("horizon_diag_install_v2")
public func installMetalDiagnostics(_ version:UInt32,_ event:MetalDiagnosticEvent?,_ snapshot:MetalDiagnosticSnapshot?) -> UInt32 {
    guard version==2, let event=event, let snapshot=snapshot, Bundle.main.bundleIdentifier=="dev.helium.g06ctest",
          Bundle.main.object(forInfoDictionaryKey:"HeliumMetalDiagnosticsVersion") as? Int == 2 else {return 0}
    if eventFunction == nil {eventFunction=event; snapshotFunction=snapshot}
    return 2
}
@_cdecl("horizon_diag_hud_v2")
public func attachMetalHUD(_ pointer:UnsafeMutableRawPointer?) {
    guard Thread.isMainThread, eventFunction != nil, let pointer=pointer,
          let parent=Unmanaged<AnyObject>.fromOpaque(pointer).takeUnretainedValue() as? UIView else {diagnosticEvent(0,30,37);return}
    if debugParent === parent, debugHUD != nil {return}
    debugTimer?.invalidate();debugHUD?.removeFromSuperview();debugParent=parent
    let label=UILabel();label.font = .monospacedSystemFont(ofSize:11,weight:.medium)
    label.numberOfLines=0;label.textColor = .white;label.backgroundColor=UIColor.black.withAlphaComponent(0.85)
    label.isUserInteractionEnabled=false;label.translatesAutoresizingMaskIntoConstraints=false
    parent.addSubview(label);debugHUD=label
    NSLayoutConstraint.activate([label.leadingAnchor.constraint(equalTo:parent.safeAreaLayoutGuide.leadingAnchor,constant:12),label.bottomAnchor.constraint(equalTo:parent.safeAreaLayoutGuide.centerYAnchor,constant:100),label.widthAnchor.constraint(lessThanOrEqualToConstant:420),label.trailingAnchor.constraint(lessThanOrEqualTo:parent.safeAreaLayoutGuide.trailingAnchor,constant:-12)])
    debugTimer=Timer.scheduledTimer(withTimeInterval:0.2,repeats:true){_ in
        guard let label=debugHUD, let parent=debugParent else {return}
        // Modal launcher always takes precedence; no native layer owns the HUD.
        label.isHidden=parent.window?.rootViewController?.presentedViewController != nil
        let values=diagnosticValues();let source=Bundle.main.object(forInfoDictionaryKey:"HeliumDiagnosticSource") as? String ?? "development-unidentified";let text=MetalDiagnosticModel.hud(values,mode:debugMode)+"\nSource: \(source) | build \(Bundle.main.object(forInfoDictionaryKey:"CFBundleVersion") as? String ?? "unknown")"
        if text != lastHUD {lastHUD=text;label.text=text;label.textColor=values.count==32 && (values[4] != 0 || values[8] != 0) ? .systemRed : .yellow}
    }
    let os=ProcessInfo.processInfo.operatingSystemVersion
    diagnosticEvent(0,39,0,(UInt64(os.majorVersion)<<32)|(UInt64(os.minorVersion)<<16)|UInt64(os.patchVersion),UIDevice.current.userInterfaceIdiom == .pad ? 2 : 1)
    diagnosticEvent(0,34,0,0,1)
    // Asynchronous disk acknowledgement/readback: no main-thread file serialization.
    DispatchQueue.global(qos:.utility).asyncAfter(deadline:.now()+1){
        let path=FileManager.default.urls(for:.documentDirectory,in:.userDomainMask)[0].appendingPathComponent("logs/Helium-Metal-Report.txt")
        let values=diagnosticValues()
        let valid:Bool
        if let file=try? FileHandle(forReadingFrom:path) {
            let data=try? file.read(upToCount:256*1024+1);try? file.close()
            let text=data.flatMap {String(data:$0,encoding:.utf8)}
            valid=values.count==32 && values[6]>0 && values[8]==0 && (data?.count ?? Int.max)<=256*1024 && text?.contains("boot=\(values[1])")==true && text?.contains("SelfTest")==true
        }else{valid=false}
        diagnosticEvent(0,34,valid ? 0 : 41,0,valid ? 2 : 0)
    }
}
@_cdecl("horizon_diag_host_idle_v2")
public func metalHostIdle() -> Bool {Thread.isMainThread && nativeDisplayIsIdle() && UIApplication.shared.applicationState == .active && debugParent?.window?.isKeyWindow == true && debugParent?.window?.rootViewController?.presentedViewController == nil}
func liftMetalHUD(){if let hud=debugHUD, let parent=debugParent {parent.bringSubviewToFront(hud)}}
func setMetalDebugMode(_ mode:String) {debugMode=mode}
