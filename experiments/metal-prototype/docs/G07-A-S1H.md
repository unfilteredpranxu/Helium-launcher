# S1H — standalone iOS host and isolated Metal probe

Starting branch `feature/helium-g07-immutable-capture` at
`4908e1571610172a95171d7b9fe53af07e25b5b7`. S1's qualified implementation
`a17712f66a8a6002957ec79d308122a2b3eda7c6` remains the baseline for comparison.
[Baseline Actions](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44231706646).

## Known facts

Baseline CI linked the Rust executable, copied it over the Xcode shell and
checked the embedded launcher. It did not verify an archived IPA or execute a
device install. The earlier standalone probe also built its archive without
a final contents gate. Game boot evidence could miss its first event if
Documents did not yet exist and included container paths and raw errors.
Swift's existing "launcher presented" line described a request queued before
the attempt to attach a view. Those are source-proven verification gaps.
The previous SideInstaller failure stage and cause remain unknown without
the sign/install result, device boot trace or a launch crash report.

The probe has its own identifier, executable, explicit UIKit scene delegate and
SwiftUI launcher framework. The framework links only the standalone Metal
experiment. Neither the host app nor its framework links the Rust Minecraft
workspace, wgpu, the Horizon launcher, game import or account paths. Its
persistent writes target only its own Documents/logs evidence. Rust renders
the original synthetic chart offscreen, including the depth fixture, and
presents through CAMetalLayer. Empty host layouts and unrelated scenes cannot
submit a drawable request. UIKit creates, lays out and detaches the layer on
main. Rust owns its retained layer and submitted command resources on the
worker until their actual retirement. Status and sticky stage bits are scoped
to the exact attempt. Completion does not establish physical scanout.

The archived probe verification checks flat Payload layout, permitted members,
required app icon, executable modes, plists, proper framework embedding, thin
arm64 device image kinds, iOS 15 target, LC_MAIN and PIE, dylib identities, run
paths, dependencies, Rust display exports and exact app/framework hashes. Apple
tools inspect the extracted binaries. The source must match the build SHA in
Info.plist. Code sign inspection accepts unsigned or linker-only ad hoc output
without provisioning or a distribution identity. The same archive checks run
on the held gameplay app after the existing Rust executable verifier. The
workflow discards that gameplay archive and retains the publication guard.

## Gameplay host contract

- The Rust executable owns winit's `UIApplicationMain` entry and primary
  scene. The embedded Horizon launcher is loaded on demand and presents its
  SwiftUI view within that scene. Existing missing-resource import remains
  the gameplay recovery flow. The probe needs no resource preparation.
- Save & Quit uses S1's nonblocking barrier. The authoritative server stops,
  finishes storage and releases the session lock only after valid retirement.
  Failure remains failed and continues to retain the owner.
- On resign-active, the client cancels its native target and posts one
  authoritative checkpoint per integrated server. The Swift bridge can ask
  UIKit for a background task before the worker starts. Its generation guard
  ends the task on completion, actual failure, abandoned work or expiration.
  Expiration ends the OS assertion and does not acknowledge the save or drop
  storage owners. Checkpointing continues as the platform permits.
- Controlled fatal return still uses S1. iOS can suspend or kill the process
  without granting additional time or waiting for Rust. Only durably admitted
  records are replayable after forced termination. LiveContainer guest unload
  remains a separate path, and the prior FRONTBOARD watchdog has no established
  join target.

## Owner acceptance stages

| Stage | What to report |
|---|---|
| A Package | Downloaded IPA and SHA-256 before signing |
| B Signed | Signer succeeded, or exact error text |
| C Installed | SideInstaller succeeded, or exact error text |
| D Cold launch | Whether Home Screen launch opens the app. If it crashes, report crash reason and any trace |
| E SwiftUI | Whether the no-world banner appeared and `swiftLauncherAttached` was recorded |
| F Metal | The chart is visibly drawn on the iPad. Save one screenshot or video and the stage trace |
| G Gameplay | **HELD.** The probe has no gameplay, asset import or world access |

Sign only with your own legitimate signing method. Keep the probe's identifier
separate from the game. No world or backup is needed for this probe and it does
not request any. Cold-launch the standalone app from the Home Screen, identify
the labeled native chart above the fixture selector, then background and
foreground the app once. Retrieve the report from **Files → On My iPad →
Helium Metal Probe → logs → Helium-Host.txt**. Keep this file after the first
failed launch if available because each new launch starts a fresh snapshot.

The first line is `executableEntered`, before the call to UIApplicationMain but
after dyld has loaded the app's required frameworks. A failure before that
point may only have an iPad crash or signing/install error. The next stages
distinguish `uiKitInitialized`, `sceneConnected`, `swiftLauncherAttached`,
`metalDeviceReady`, `metalLayerAttached`, `drawableAttempted`,
`presentSubmitted` and `presentCompleted`. Device unavailable, drawable
unavailable, failure and cancellation are separate states. Logs use finite
enumerated stages and retain early boot plus recent events.

The held game has separate `Documents/horizon-boot.log` for Rust entry,
framework load classifications and winit resume, `Documents/logs/Helium-Host.txt`
for Swift launcher attachment, gameplay handoff and background task receipts,
and the existing Helium Metal report for genuine game capture events.
Do not infer gameplay entry from a launcher request, or pixels from a GPU
command. No private path, world, credential or raw error is in the stage files.

## Qualified no-world probe

**NO-WORLD DEVICE PROBE — HARDWARE NOT YET VERIFIED.** Gate A approved for
the Stage B–F owner sequence above. Stage A passed on Apple CI. Stages B–F
have no owner/device evidence yet, and Stage G remains held.

- [Unsigned GitHub Actions artifact 14398029533](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/artifacts/14398029533)
- Filename `Helium-S1H-Metal-Probe-unsigned.ipa`
- IPA SHA-256 `450dbc17126e895e851d91e81f1f8a745d2cb120176f87b4359f575ecec9668b`
- App executable SHA-256 `8110f3f7e9415e11c4e54b0dd142ae9ac7154bc0470cc1028b8766b1e2aec097`
- Framework executable SHA-256 `be98424a7681ccd88ad025e88a984cf1a87581ac951f368d444f5290a28ef887`
- Bundle `dev.helium.metalprobe`, version 0.1.0, build 13001, `HeliumProbeKit.framework`
- Source `34f775a66852a29c801a1e8dc9d18cb620d48358`
- CI verified the exact archive before upload. GitHub lists the artifact as
  available. Re-download from the cloud executor was denied by the artifact
  storage endpoint. Use the runner's SHA-256 to check the file after download
  and before legitimate owner signing.

The first S1H run [44239442082](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239442082)
failed in GenerateTAPI. The iPhoneOS Xcode process inherited both
`MACOSX_DEPLOYMENT_TARGET` and `IPHONEOS_DEPLOYMENT_TARGET`. The framework build
now clears the macOS setting locally and passed in
[44239790131](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131).
The failed run published no IPA.

## Qualification ledger

CI [44239790131](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131)
uses Rust 1.95.0 and committed lockfiles. These tests used synthetic
temporary storage and the actual CI Metal device where indicated.

| Scope | Exact command or job | Result |
|---|---|---|
| Mach-O and archive fixtures | `python3 -m unittest discover -s tests -p test_helium_ios_package.py` and `python3 -m unittest discover -s tests -p test_attached_ios_binary.py` | 10 and 4 passed locally. The archive suite also ran in [Linux diagnostics 134584728077](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/job/134584728077) |
| Owned jobs | `cargo +1.95.0 test --locked -p horizon-threads --lib jobs::` | 8 passed per OS |
| Save and recovery | `cargo +1.95.0 test --locked -p horizon-client-lib --no-default-features --lib save:: -- --test-threads=4` | 93 passed per OS. Six harness ignores, four subprocess fixtures launched by passing parents and two resource cases skipped |
| Player receipts | Same package and flags, `--lib lan::playerdata::tests::` | 26 passed per OS |
| Gameplay shutdown | `cargo +1.95.0 test --locked -p horizon-client --no-default-features --lib game::integrated::shutdown_tests -- --test-threads=1` and same invocation with `game::saving::shutdown_tests` | 4 and 2 passed per OS. The new checkpoint coalescing regression passed |
| Fatal return | Same game test command with `game::runtime_exit::tests` | 2 passed per OS. The ignored child was invoked by its parent |
| Launcher | `cargo +1.95.0 test --locked -p horizon-ios-launcher --lib` | 2 Linux, 3 macOS including native Core Foundation callback |
| Streaming | `cargo +1.95.0 test --locked -p horizon-client-render --lib streaming::` | 25 passed and two benchmark ignores per OS |
| Metal and verified probe IPA | [Apple SDK 134584728394](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/job/134584728394) | Prototype standard fixture suite 87 passed with 9 deferred, 2 doc tests passed. Explicit `cargo +1.95.0 test --locked --manifest-path experiments/metal-prototype/Cargo.toml -- --ignored --nocapture --test-threads=1` ran all 9 real-Metal tests. MSL deployment, app and framework link, Cargo graph isolation and actual archive verification passed |
| Production bridge | [134584728408](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/job/134584728408) | 20 passed with 2 deferred. Real native worker fixture explicitly passed. Prepared Minecraft resource fixture remained skipped. Opt-in game and iOS compile passed |
| Swift / C / Rust and UIKit | [134584728474](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/job/134584728474) | Recorder 18 passed, Swift/C/Rust report and recovery passed, Swift startup evidence persisted injected failure and bounded writes, actual UIKit framework compiled |

Shutdown job links are [Linux 134584728507](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/job/134584728507)
and [macOS 134584728300](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/job/134584728300).
The fixture suites above reported zero failures on the qualified source. The
new iOS background assertion and normal close still require device execution
before any gameplay release authorization.

All seven execution jobs passed at
`34f775a66852a29c801a1e8dc9d18cb620d48358`. The
[game link job 134588424998](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/job/134588424998)
built the real Rust release executable using
`cargo +1.95.0 build --release --locked --target aarch64-apple-ios -p horizon-client --no-default-features --features audio,helium-direct-metal`.
The Xcode app link, preexisting ABI and executable identity guard, and
archive verifier passed. The game executable had 84,036,688 bytes and
SHA-256 `0b286b5cd242b66e29cda61100cec11b959a17e5aa1d379475977bb241ba399b`.
The temporary unsigned `held.ipa` had SHA-256
`b9cbf8e4e1e62b58c705f2ecbccba3dfd47678a4e8ff6776d82964b343103239`.
It contained `HorizonLauncherKit` with SHA-256
`eaff9688c958c3e727ed6eda39be37c3d5f850be5c3bdf0e2276f5354cb67668`,
bundle `dev.helium.g06ctest`, version 0.2.8, build 12001, and iOS 15.0
device arm64 images. CI discarded that archive and skipped the gameplay
publication step. [Build and verifier logs only](https://github.com/unfilteredpranxu/Helium-launcher/actions/runs/44239790131/artifacts/14399992966).

**Next dependency:** collect the standalone probe's SideInstaller stages B–F
on the iPad with the host report and one screenshot or video, or capture the
exact first stage failure. S2 has not started.
