# Mobile upstream integration, 31 October 2026

The merge joins mobile main `7247a94173f5be182553e4ff2098a574c1ed0816` with upstream
`ed5565713e80ab9ec8550eacd46de99484b7367f`, retaining upstream ancestry. The previous
integration ended at `7ebf7d26c`; this adds 2,318 upstream commits.

## Conflict decisions

- Keep the SwiftUI launcher and deferred engine initialization, owned engine resources,
  Quit-to-launcher cleanup, AirPlay idle screen, touch/pointer switching, clipboard bridge,
  read-only signed-in identity, reduced launcher Settings and three-tab navigation.
- Adopt upstream's unified asynchronous resource-pack coordinator, progress and rollback.
  Retain loader-safe application, superseded-selection rejection and worker draining on Quit.
  Server reload outcomes are delivered even when another local selection is pending.
- Combine upstream OIT, HDR, post effects, mesh pooling, equipment overlays and item activation
  with mobile GPU capability gates, deep-stack construction, terrain metadata and actual item
  IDs for shader draws. Clear and account for metadata arrays when pooling meshes.
- Preserve NPC texture lifetime and cache recovery while merging upstream per-entity texture
  snapshots. Invisible entities can still render worn equipment and its shader item spans.
- Use upstream's structure-placement generation and live-placement APIs, which now cover the
  same behavior as the local implementation and also return spawned entities. Remove duplicate
  local APIs. Keep independent world regression tests.
- Retain cancellation independent of the network writer lock alongside upstream handshake,
  server identity, timeout, legacy ping and remote-world changes.
- Merge upstream IME with the mobile keyboard controller: a focused chat field must not reopen
  the iPad keyboard after the player hides it. Desktop-only unmodified-key learning is unavailable
  on winit's mobile backends, so mobile retains its existing binding names.
- Retain native mod lifecycle/input/render hooks and upstream native-mod updates. The native
  launcher does not gain an on-device compiler in this merge.
- Make resource-backed structure-mob comparison fixtures use their reference calendar, rather
  than the host date. Halloween gameplay remains date-dependent; tests use a per-instance override
  without mutating process environment.
- Update Android's direct dependencies and lockfile for the shared game entry point.
- Preserve the fork's removed desktop CI. Scope inherited desktop release/nightly and upstream
  governance workflows to the original repository; the mobile fork lacks their release tag,
  signing, parity and runner infrastructure. Keep iOS and Android builds active. iOS PR builds
  use hosted runners and never post to Discord; Android PRs never use signing secrets.
- Validate distributable IPAs before uploading artifacts. Discord delivery is optional and
  time-bounded; an invalid webhook produces a warning and leaves the built artifact available.
  Replacing the deleted webhook still requires a repository owner to update its Actions secret.

## Validation boundaries

The integration is reviewed by subsystem and by Astra before publication. Test results and the
exact GitHub Actions runs belong in the integration pull request. A skipped desktop job is not
an executed desktop or parity check. Local compilation is not physical-device, Hypixel,
shader-pack, AirPlay or multiplayer authentication validation.

Network cancellation closes attached sockets immediately; DNS/connect/authentication work may
finish briefly after disconnect before rejecting its cancelled result. It does not retain a
running game renderer or world while the native launcher is idle.

Microsoft client-ID source configuration and local signing credentials are not changed by this
integration. No prepared game resources or player data are included.
