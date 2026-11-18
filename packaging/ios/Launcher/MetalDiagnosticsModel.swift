// MIT; Copyright (c) 2026 Horizon contributors. Shared by UIKit and the macOS ABI E2E.
import Foundation
public typealias MetalDiagnosticEvent = @convention(c) (UInt64, UInt32, UInt32, UInt64, UInt64) -> UInt32
public typealias MetalDiagnosticSnapshot = @convention(c) (UnsafeMutablePointer<UInt64>?, Int) -> UInt32
struct MetalDiagnosticModel {
    static let stages = ["", "BOOT", "TEST_MODE_SELECTED", "RESOURCES_FOUND", "RESOURCES_LOADING", "LAUNCHER_SHOWN", "LAUNCHER_DISMISSED", "WORLD_REQUESTED", "WORLD_READY", "PRODUCER_READY", "CAMERA_READY", "REAL_MESH_ACCEPTED", "CANDIDATE_REJECTED", "HOST_REQUESTED", "UI_LAYER_ATTACHED_HIDDEN", "WORKER_STARTED", "NATIVE_DEVICE_CREATED", "BANK_UPLOAD_STARTED", "BANK_COMPLETED", "ALL_BANKS_RESIDENT", "DRAW_SUBMITTED", "DRAW_GPU_COMPLETED", "DRAWABLE_REQUESTED", "DRAWABLE_ACQUIRED", "PRESENT_SUBMITTED", "PRESENT_GPU_COMPLETED", "PRODUCER_REVALIDATED", "PUBLISH_REQUESTED", "UI_REVEALED", "UI_REMOVED", "CANCELLED", "RETIRED", "RETRY_WAITING", "RETRY_EXHAUSTED", "SELF_TEST", "MESH_IDENTITY", "MESH_ORIGIN", "MATERIAL_REVISION", "BANK_LAYOUT", "DEVICE_METADATA", "TEXTURE_LAYOUT", "UPLOAD_SUBMITTED", "MEMORY_COUNTERS", "CAPTURE_VALIDATED_HISTORICAL", "HISTORICAL_CAPTURE_READY", "CURRENT_MATERIAL", "CURRENT_LIGHT", "WORLD_SOURCE", "CAPTURE_METRICS", "UPLOAD_PREPARATION"]
    static let causes = ["NONE", "UNKNOWN", "MESH_REPLACED", "COLUMN_UNLOADED", "WORLD_EPOCH_CHANGED", "RENDERER_REBUILT", "MATERIAL_REVISION_CHANGED", "LIGHTMAP_REVISION_CHANGED", "BANK_SOURCE_STALE", "FRAME_SIZE_CHANGED", "VIEW_LAYOUT_INVALID", "VIEW_DETACHED", "WINDOW_NOT_KEY", "MODAL_OPEN", "APP_INACTIVE", "HOST_TTL_EXPIRED", "TICKET_CANCELLED", "SHUTDOWN", "WORKER_FAILED", "RUST_PANIC", "SNAPSHOT_EXPIRED", "PARENT_GONE", "WINDOW_GONE", "WINDOW_HIDDEN", "RESOURCE_RELOAD", "LAUNCHER_COVERED", "PRODUCER_UNAVAILABLE", "CAMERA_OUTSIDE_FRUSTUM", "DEVICE_FAILURE", "UPLOAD_FAILED", "FRAME_FAILED", "PRESENT_FAILED", "DRAWABLE_UNAVAILABLE", "GPU_TIMEOUT", "MEMORY_PRESSURE", "NO_FRESH_MESH_AVAILABLE", "RETRY_EXHAUSTED", "HUD_UNAVAILABLE", "INVALID_ABI", "INVALID_CONFIG", "PREVIOUS_BOOT_INCOMPLETE", "RECORDER_DEGRADED"]
    static func pixels(_ width:Double,_ height:Double)->UInt64 {
        guard width.isFinite,height.isFinite,width>=0,height>=0,width<=4096,height<=4096 else{return 0}
        return (UInt64(width)<<32)|UInt64(height)
    }
    static func invalidHost(age:Double,parent:Bool,window:Bool,attached:Bool,hidden:Bool,key:Bool,active:Bool,modal:Bool,size:Bool)->UInt32 {
        if age>=35{return 15};if !parent{return 21};if !window{return 22};if !attached{return 11};if hidden{return 23};if !key{return 12};if !active{return 14};if modal{return 13};if !size{return 10};return 0
    }
    static func mode(bundle: String, marker: Int?, legacy: Bool, file: Data?) -> (Int32, String) {
        let isolated = bundle == "dev.helium.g06ctest" && marker == 2
        if bundle=="dev.helium.g06ctest", let marker=marker, marker != 2 {return (-1,"INVALID")}
        if marker != nil && !isolated { return (0,"OFF") }
        guard isolated || legacy else { return (0,"OFF") }
        guard let file = file else { return isolated ? (2,"AUTO-ARMED") : (0,"OFF") }
        guard file.count <= 64, let value = String(data:file,encoding:.utf8) else {return (-1,"INVALID")}
        switch value {case "off", "off\n":return (0,"OFF")
        case "attached-one-section", "attached-one-section\n":return (2,"CONFIG-ARMED")
        case "live-terrain", "live-terrain\n":return isolated ? (3,"LIVE-TERRAIN") : (-1,"INVALID")
        default:return (-1,"INVALID")}
    }
    static func hud(_ a: [UInt64], mode: String) -> String {
        if mode == "LIVE-TERRAIN" {
            return "HELIUM S2B · LIVE TERRAIN MODE\nNative completion and frame timing in game log\nwgpu reference active · PHYSICAL SCANOUT UNVERIFIED"
        }
        guard a.count == 32, a[0] == 2 else {return "HELIUM G07-A · HISTORICAL CAPTURE\nRECORDER DEGRADED: ABI mismatch\nwgpu producer/fallback active"}
        let stage = a[3] < UInt64(stages.count) ? stages[Int(a[3])] : "INVALID_STAGE"
        let cause = a[4] < UInt64(causes.count) ? causes[Int(a[4])] : "UNKNOWN"
        return "HELIUM G07-A · HISTORICAL CAPTURE — DIAGNOSTIC\nMode: \(mode) | Attempt \(a[2]) / 3 | boot \(a[1])\nStage: \(stage) | \(a[10]) ms\nCapture gen/material/light: \(a[18])/\(a[20])/\(a[19]) | current material/light: \(a[29])/\(a[30])\nBanks: \(a[16])/\(a[17]) | gen: \(a[18]) | material: \(a[20]) | light: \(a[19])\nFIRST CAUSE: \(cause)\nGPU possibly pending: \(a[28]) | NATIVE REVEAL: \(a[24] == 1 ? (a[15] == 1 ? "TEST_ONLY" : "YES") : "NO") | PIXELS UNVERIFIED\nRecorder: \(a[8] == 0 ? "OK" : "DEGRADED") | ack \(a[6])/\(a[5]) | lost \(a[7])\nSelf-test: \(a[26] == 2 ? "PASS (recorder only)" : "RUNNING / NOT VERIFIED")\nwgpu producer/fallback active\nFiles → Helium G06C Test → logs → Helium-Metal-Report.txt"
    }
}
