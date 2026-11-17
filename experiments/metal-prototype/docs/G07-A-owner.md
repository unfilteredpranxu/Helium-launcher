# G07-A Revision 3 — owner testing remains on hold (Prompt 12/25)

**NO NEW OWNER IPA. No owner test is authorized yet.** Build 12001's earlier
recommendation remains withdrawn. Do not repeat the crash or stress a real world.
No owner saves, backups or existing installation were touched during this work.

The implemented shutdown contract is:

- Save & Quit requests cooperative stop, then polls owned cleanup while the UI
  remains active. A timeout or save error is never a successful save.
- Region, player and metadata records admitted for saving have a synced redo
  handoff before asynchronous writing. Successful completion follows actual
  destination sync. Valid interrupted handoffs replay automatically under the
  session lock on the next open.
- Invalid redo refuses opening and is preserved. Do not delete redo files or
  reopen an interrupted session using an older build that cannot understand them.
- Background checkpoint requests do not imply extra iOS time or guarantee that
  unsaved live mutations already reached a durable handoff.

One P0 blocker remains: a fatal event-loop return or guest/runtime unload can end
execution before all live world state has been handed off. The existing process
exit path can kill pending retirement. A controlled temporary-world subprocess
reproduces that exposure; it is not a claim about the owner's exact crash stack.
See [Revision 3 engineering evidence](G07-A.md).

After this hold is explicitly cleared, the acceptance plan remains ONE combined
short session with ONE specifically qualified unsigned build:

1. Back up original saves/resources/settings using your trusted method before an
   upgrade. Do not uninstall or migrate a container without that backup.
2. Use the isolated `dev.helium.g06ctest` guest, your own trusted signer and already
   legitimately prepared Minecraft 26.3 resources. Verify full source/build,
   `AUTO-ARMED` and `HISTORICAL CAPTURE` in the HUD.
3. In a disposable single-player world, look toward nearby solid terrain for
   30–60 seconds with stable orientation/foreground. Record the labeled yellow/
   black native snapshot if it appears. wgpu remains active; no continuous native
   gameplay or physical scanout pass is inferred from compilation/GPU commands.
4. Use ordinary Save & Quit, wait for real completion, then relaunch once to check
   disposable progress. Do not force-kill, fill storage or provoke another crash.
5. Share ONE report: Files → guest application container → Documents → logs →
   Helium-Metal-Report.txt, plus the matching video. LiveContainer's guest Files
   location may differ from standalone SideInstaller. No configuration editing,
   ordinary-log splicing, personal diagnostic ZIP, world/redo files or Minecraft
   assets are requested.

This plan is inactive while the hold remains. G07 physical A16/iPadOS 27.3 scene
and shutdown/save acceptance are untested. Prompt 13 has not started.
