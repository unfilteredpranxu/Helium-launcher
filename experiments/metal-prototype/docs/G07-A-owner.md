# G07-A Revision 2 — owner testing on hold (Prompt 12/25)

**NO NEW OWNER IPA. Do not install or run the previously linked build 12001 for
this combined test. Its recommendation is withdrawn following the new process-
exit watchdog evidence. No replacement build is qualified for shutdown/save safety.**

The new evidence is FRONTBOARD 0x8BADF00D, process-exit, five-second graceful-exit
failure under LiveContainer. It is separate from earlier light/material revision
cancellations and does not establish a Metal/OOM crash or corrupted save.
No action is taken on your world, backups, app container or existing installation.
Do not reproduce the crash or stress a real world to gather more logs.

The source investigation and remaining P0 blocker are in [G07-A.md](G07-A.md).
An owned nonblocking shutdown/recovery contract must be implemented and qualified
before a new owner build is distributed. Prompt 13 has **not** started.

Only AFTER that hold is explicitly cleared, the acceptance plan is one combined
short session, not a second crash investigation:

1. Back up original resources/saves/settings using your trusted method before any
   upgrade. Do not uninstall or migrate a container without that backup.
2. Install one specifically qualified unsigned build using your own signer. Verify
   exact source/build, AUTO-ARMED and HISTORICAL CAPTURE in the HUD. Use the isolated
   dev.helium.g06ctest guest and already legitimately prepared 26.3 resources.
3. Use a disposable world only. Look at nearby solid terrain for 30–60 seconds,
   keep orientation/foreground stable, and record the yellow/black native snapshot
   if it appears. wgpu remains active. GPU completion is not proof of scanout.
4. Use ordinary Save & Quit, wait for confirmed completion, and relaunch once to
   check the disposable progress. Do not force-kill, fill storage or provoke a crash.
   Native-frame acceptance and save/exit/relaunch acceptance are independent.
5. Share ONE report: Files → guest application container → Documents → logs →
   Helium-Metal-Report.txt, plus the matching video. LiveContainer's guest Files
   location need not match standalone SideInstaller. No config editing, personal
   diagnostic ZIP, log splicing or private Minecraft assets are requested.

G07 physical A16/iPadOS 27.3 scene and save/exit behavior remain untested. This plan
is inactive while the P0 hold above remains. Do not perform it with build 12001.
