// TEST_EVENT_ONLY. Real Swift -> static Rust C ABI -> writer -> reopened report.
import Foundation
@main struct Rehearsal {
 static func main() throws {
  let root=ProcessInfo.processInfo.environment["HELIUM_DIAGNOSTIC_DIRECTORY"]!
  assert(helium_diag_init_v2(1)==2)
  if CommandLine.arguments.contains("--recover") {
   assert(helium_diag_flush_test_v2()==2)
   let report=try String(contentsOfFile:root+"/Helium-Metal-Report.txt",encoding:.utf8)
   assert(report.contains("PREVIOUS_BOOT_INCOMPLETE=true") && report.contains("ViewLayoutInvalid"))
   print("TEST_EVENT_ONLY previous process recovery PASS");return
  }
  assert(helium_diag_event_v2(0,2,0,12001,2)==2)
  assert(helium_diag_test_attempt_v2(101)==2)
  for (code,expected) in [(UInt32(10),UInt64(2360)),(12,1),(14,1),(13,1)] {assert(helium_diag_event_v2(101,30,code,expected,0)==2)}
  for code in [UInt32(11),18,19,24,34,28,30,31,6,2,4,15,21,22,23,33] { assert(helium_diag_event_v2(101,30,code,11,12)==2) }
  assert(helium_diag_event_v2(101,43,0,0,0)==0) // Swift cannot fabricate capture validation
  assert(helium_diag_event_v2(101,44,0,0,0)==0)
  assert(helium_diag_event_v2(101,21,0,0,0)==0) // Swift cannot fabricate DRAW_GPU_COMPLETED
  assert(helium_diag_event_v2(101,30,999,0,0)==0)
  assert(helium_diag_test_attempt_v2(102)==2);assert(helium_diag_event_v2(102,14,0,2360,1640)==2)
  assert(helium_diag_event_v2(101,30,19,0,0)==2) // late callback MUST stay attempt101
  assert(helium_diag_event_v2(102,30,7,11,12)==2);assert(helium_diag_event_v2(102,999,1,0,0)==0)
  assert(helium_diag_test_attempt_v2(103)==2);assert(helium_diag_event_v2(103,30,3,0,0)==2);assert(helium_diag_test_attempt_v2(104)==0)
  assert(helium_diag_flush_test_v2()==2)
  var state=[UInt64](repeating:0,count:32);assert(state.withUnsafeMutableBufferPointer{helium_diag_snapshot_v2($0.baseAddress,32)}==2)
  assert(state[2]==103 && state[4]==3 && state[6]>=state[5] && state[8]==0 && state[15]==1)
  // Synthetic numeric shutdown context, same real report writer and C ABI.
  try "1 8 0 1 5100 1 \(state[1])\n10 11 1 5 1 1 0\n20 11 1 7 1 1 1\n".write(toFile:root+"/shutdown-save-numeric.txt",atomically:true,encoding:.utf8)
  assert(helium_diag_event_v2(0,34,0,0,2)==2)
  assert(helium_diag_flush_test_v2()==2)
  let report=try String(contentsOfFile:root+"/Helium-Metal-Report.txt",encoding:.utf8)
  assert(report.contains("app_build=12001") && report.contains("recorder_status=OK") && report.contains("ViewLayoutInvalid") && report.contains("LightmapChanged") && report.contains("ColumnUnloaded") && report.contains("PHYSICAL_PIXELS_UNVERIFIED"))
  assert(report.contains("SHUTDOWN/SAVE P0 HOLD") && report.contains("FILE SAVE/WORKER FAILURE") && report.contains("five_second_probe_near=true"))
  let lines=try String(contentsOfFile:root+"/helium-metal-events.jsonl",encoding:.utf8).split(separator:"\n").map{try JSONSerialization.jsonObject(with:Data($0.utf8)) as! [String:Any]}
  let sequence=lines.map{($0["event_seq"] as! NSNumber).uint64Value}
  assert(zip(sequence,sequence.dropFirst()).allSatisfy{$0 < $1})
  let causes=lines.filter{($0["first"] as? Bool)==true}
  assert(causes.count==3 && (causes[0]["attempt_id"] as! NSNumber).uint64Value==101 && (causes[0]["cause_code"] as! NSNumber).intValue==10)
  assert(lines.filter{($0["attempt_id"] as! NSNumber).uint64Value==101 && ($0["cause_code"] as! NSNumber).intValue==19}.count==2)
  assert(lines.allSatisfy{($0["test_only"] as? Bool)==true})
  let hud=MetalDiagnosticModel.hud(state,mode:"TEST_EVENT_ONLY")
  assert(hud.contains("COLUMN_UNLOADED") && hud.contains("wgpu producer/fallback active"))
  var historical=state; historical[20]=47;historical[29]=48;historical[19]=32;historical[30]=33
  let captureHUD=MetalDiagnosticModel.hud(historical,mode:"AUTO-ARMED")
  assert(captureHUD.contains("HISTORICAL CAPTURE") && captureHUD.contains("48/33"))
  for (bundle,marker,legacy,file,expected) in [("dev.helium.g06ctest",2,false,Optional<Data>.none,Int32(2)),("dev.helium.g06ctest",2,false,Data("off\n".utf8),0),("dev.horizon.game",2,false,nil,0),("dev.horizon.game",nil,false,nil,0),("dev.helium.g06ctest",3,false,nil,-1),("dev.helium.g06ctest",2,false,Data("bad".utf8),-1),("dev.helium.g06ctest",2,false,Data("live-terrain\n".utf8),3),("dev.horizon.game",nil,true,Data("live-terrain".utf8),-1)] {
   assert(MetalDiagnosticModel.mode(bundle:bundle,marker:marker,legacy:legacy,file:file).0==expected)
  }
  assert(MetalDiagnosticModel.pixels(Double.nan,10)==0)
  for (age,key,active,modal,size,expected) in [(0.0,false,true,false,true,UInt32(12)),(0,true,true,false,false,10),(0,true,false,false,true,14),(0,true,true,true,true,13),(35,true,true,false,true,15)] {
   assert(MetalDiagnosticModel.invalidHost(age:age,parent:true,window:true,attached:true,hidden:false,key:key,active:active,modal:modal,size:size)==expected)
  }
  print("TEST_EVENT_ONLY Swift/C/Rust/writer/report/HUD-model roundtrip PASS. No UIKit, Minecraft or physical pixels claim.")
  print(hud)
 }
}
