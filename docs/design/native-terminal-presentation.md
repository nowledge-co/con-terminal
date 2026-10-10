# Native Terminal Presentation

Status: reporter accepted the isolated macOS experiment on 2026-10-10.
The upstream proposal is published; Con's temporary dependency integration is
implemented and locally verified in PR #460, pending native CI and merge.
The maintainer reviewed the upstream candidate and
authorized a temporary public fork on 2026-10-10. Source publication, PR merge,
and release remain separate delivery states.

## Isolated Experiment

On 2026-10-10 the maintainer authorized an isolated GPUI experiment, not a
production dependency change. The experiment copies the resolved 0.3.7 core,
macOS, and Apple packages to `/private/tmp/` and overrides them only in a
separate worktree. Registry sources, `3pp/`, and this workspace's manifests
remain unchanged.

The candidate exposes a default-no-op platform request and consumes it in
macOS drawable presentation. A requested frame temporarily uses transaction
presentation, preserves AppKit's previous mode, and retains the request when
drawing fails. Con requests it on native geometry updates and tab visibility
handoff; ordinary terminal output retains asynchronous presentation.

`CON_NATIVE_PRESENT_SYNC=0` disables the Con requests in that experimental
binary for an otherwise identical comparison. This is not a supported Con
setting. Debug logs under `con::native_presentation` and
`gpui::native_presentation` record geometry requests and submission duration,
not terminal text.

The isolated app is **Con Sync Preview**, with private configuration, history,
session, and socket paths. A successful build or a settled screenshot does not
prove that it fixes the reported transition frames. The reporter subsequently
tested the preview and could no longer reproduce the leak. This acceptance
applies to that macOS setup, not every display, OS version, or platform.

The native macOS app/CLI build and formatting checks passed for the isolated
candidate. An initial smoke run recorded 53 requested presentations, all
successful, with mean draw submission time 0.51 ms and maximum 3.71 ms including
warm-up. This measures the Metal draw phase, not end-to-end latency or FPS.
Subsequent terminal typing, output, and scrolling did not add synchronization
requests. Nested panes remained live through panel toggles, tab switching, and
an agent-panel divider drag. These checks do not replace inspection of actual
transition frames or the reporter's acceptance.

All four geometry tests and both initial-fallback scene tests passed against
the experimental dependency graph. Earlier complete serial runs timed out in
unchanged native file-watcher tests. With the final reviewed snapshot and a
short, isolated temporary directory under the user's home, the complete serial
app suite passed all 558 tests without exclusions or test-code changes.
Using a long temporary path separately failed an existing Unix socket test's
`SUN_LEN` limit; it was corrected by shortening that fixture path. The initial
failures are not counted as passes, and this does not establish the cause of
every native watcher timeout. Windows/Linux native validation remains unavailable.

## Problem

On macOS, terminal panes are native Ghostty IOSurface layers below GPUI's Metal
layer. Con changes their frames during GPUI prepaint, then synchronously draws
Ghostty. GPUI's ordinary Metal presentation is asynchronous. Even when every
logical coordinate is correct, the compositor can combine terminal geometry
from one frame with pane title bars and dividers from another.

The 2026-10-10 reporter capture includes a light rectangle one title bar high,
not just a thin divider. Endpoint snapping is useful but cannot establish that
this presentation mismatch is resolved.

## Existing Upstream Support

In the resolved `gpui-pre-*` 0.3.7 packages:

- `gpui-pre-apple/src/metal_renderer.rs` has
  `set_presents_with_transaction`. Its synchronous draw branch commits the
  command buffer, waits until it is scheduled, then presents the drawable.
- `gpui-pre-macos/src/window.rs` uses that branch around `display_layer` and
  subsequent key-window activation callbacks.
- `gpui-pre/src/window.rs` has no public, frame-scoped request for applications
  embedding native layers. `on_next_frame` callbacks run before the next draw;
  they do not observe actual GPU presentation.

[Apple's presentation contract](https://developer.apple.com/documentation/quartzcore/cametallayer/presentswithtransaction)
explains why asynchronous Metal and native layer changes need not appear
together. Setting only the CAMetalLayer property from Con is not valid: the
renderer must also use the matching command-buffer submission sequence.

## Proposed Contract

Expose an upstream, application-requested synchronization of the **current
frame's native layer changes and drawable presentation**. The exact public API
name belongs to upstream; it must not mention Con or Ghostty.

Con requests synchronization only when a visible native terminal changes
geometry or visibility. All pane updates belonging to that frame participate
in the same presentation boundary. An idle terminal, ordinary text output,
scrolling without geometry changes, and ordinary GPUI-only views retain the
current fast asynchronous path.

The platform implementation must preserve existing transaction presentation
around AppKit callbacks, including nested requests. A one-frame request cannot
turn off a transaction mode that AppKit already owns. It must not persist across
later unrelated frames or be implemented as a timer. Presentation failure or an
unavailable drawable needs an explicit retry policy, not silent success.

The non-macOS default must preserve current behavior. Windows readback images
and Linux terminal cells already participate in GPUI's scene; they do not need
AppKit/Metal native-layer synchronization. Their resize coverage problems
remain independently testable.

## Integration Boundaries

- Prefer compatible published upstream packages. The 2026-10-10 exception
  permits a public, immutable-revision GPUI fork containing only the reviewed
  native-presentation change and necessary snapshot packaging. It does not
  permit local patches, registry edits, or `3pp/` dependencies.
- Preserve the exact compatible GPUI snapshot and paired component graph.
  Zed's Git packages cannot directly replace the renamed `gpui-pre-*` packages.
  Check dependency identity across the app, components, assets, and test support
  before integrating the fork.
- Keep background opacity and blur unchanged. No full-terminal matte, larger
  separator, snapshot stretch, or next-frame visibility heuristic.
- Trigger synchronization on real geometry/visibility changes, not every
  terminal repaint. Do not read terminal text or runtime metadata to decide it.
- Use the GPU scheduling fence required by the API, not `wait_until_completed`
  on every frame. Measure geometry transitions before enabling the integration.
- Preserve old-macOS backing compatibility and display/sleep lifecycle rules.

## Acceptance

- Verify frame-request consumption, nested AppKit transaction behavior, and
  failed/absent drawable handling in upstream tests.
- Capture repeated agent-panel, input-bar, and sidebar show/hide with nested
  panes, glass enabled, and a bright window behind Con. Inspect actual transition
  frames, not only the final screenshot.
- Test tab and surface switching, split/close/zoom, live resize, and display
  scale changes, including rapid reversals.
- Compare idle, scrolling, and transition frame timing against the existing
  renderer; avoid an always-on synchronization performance tax.
- Require native Windows/Linux CI and keep their renderer changes separate.

The reporter scenario passed in the isolated preview. The production issue
remains open until the same mechanism is integrated through compatible
dependencies and validated in Con. Correct scene geometry alone is not proof
of atomic native/GPU presentation.

## Upstream Handoff

The proposal is published as [Zed Discussion #65433](https://github.com/zed-industries/zed/discussions/65433),
with the reporter's original transition screenshot, before/after behavior,
reviewed source diff, and explicit verification limits. A new upstream PR waits
for confirmation of the public API direction, following Zed's contribution rules.

The proposed API is `Window::request_native_surface_presentation_sync`, backed
by a default-no-op `PlatformWindow` method. It does not schedule a frame. On
macOS, requests coalesce until a drawable is successfully submitted; an
unavailable drawable retains the request for the next draw without a busy retry
loop. The renderer restores the previous transaction mode on both success and
failure. Existing `draw` signatures and non-macOS implementations are unchanged.

The reviewed source candidate is based on official Zed revision
`f16f9652ec57bf806e65b2a0d51bb92a63644914`, independently of Con's manifests.
It is publicly available at
[`8bd2ec8cd022e3b4c2a0d62e9b39b4c0f1508aa1`](https://github.com/wey-gu/zed/commit/8bd2ec8cd022e3b4c2a0d62e9b39b4c0f1508aa1).
This is not an upstream merge or a compatible package release.
Its tests cover request coalescing, return to asynchronous rendering, request
retention after failed draws, restoration of both prior presentation modes, and
the default no-op platform's lack of frame demand.
The unavailable-layer test exercises the real Metal backend; it does not claim
to simulate drawable exhaustion or prove compositor timing.

Native verification ran directly in the official Zed checkout with its
original macros, test font assets, and pinned dependencies:
370 GPUI tests, 14 Apple tests, and 11 macOS tests passed. Two existing
timing-print tests were intentionally ignored. All four new regression tests
passed. The official manifest and lockfile were unchanged. This supersedes
the earlier standalone verification attempt.

The final candidate also passed the official workspace formatting check and
Zed's `script/clippy` for those three crates in release mode, with all targets,
all features, and warnings denied. The last renderer refinement avoids writing
the layer's transaction property when AppKit already enabled that mode; native
tests were rerun with the official lockfile after this refinement.

The maintainer confirmed human review of the candidate on 2026-10-10 and
manually removed the README review marker. The final diff against the official
base contains only the four GPUI source files; README is unchanged. The source
candidate is now available on the public fork at
[`8bd2ec8cd022e3b4c2a0d62e9b39b4c0f1508aa1`](https://github.com/wey-gu/zed/commit/8bd2ec8cd022e3b4c2a0d62e9b39b4c0f1508aa1).
This is an upstream source candidate, not a compatible `gpui-pre-*` package
release or a revision already consumed by Con. No upstream PR is submitted yet.
Prepare an honest before/after comparison and disclose the native test scope
and unavailable platform coverage. Do not submit an autonomous contribution or
substitute the isolated Con preview for the official-source Zed checks.

The complete Zed application subsequently built natively with the official
lockfile on 2026-10-10. A separate stateless app, private home/data directory,
and disposable text project exercised ordinary editor input and saving,
command-palette dismissal and reopening, right-dock hide/show, and window zoom
followed by further input. Rendered results and saved text were inspected,
then the test app quit normally. The project stayed in Restricted Mode; no
user project, account, or production settings were used. This is a bounded
ordinary-rendering smoke check, not a complete Zed regression suite or a test
of embedded native surfaces inside Zed. The debug link emitted an oversized
unwind-section warning, and the existing `block` dependency emitted a
future-compatibility notice; neither was suppressed.

## Compatible Dependency Snapshot

Con's three overrides resolve from the public `wey-gu/zed` repository at
[`655ce8bbc10e1f1ef3d1d5bdf5a3405008157792`](https://github.com/wey-gu/zed/commit/655ce8bbc10e1f1ef3d1d5bdf5a3405008157792).
The transport branch is `fix/con-native-presentation-snapshot`; consumers pin
the full revision, not that moving branch.

The snapshot copies the published `gpui-pre`, `gpui-pre-apple`, and
`gpui-pre-macos` 0.3.7 packages based on Zed revision
`1a28cff4b409169bac058bca40dfbfeb7621d19b`. Their normalized manifests,
versions, and other source files remain unchanged. Only the four reviewed
presentation source files differ, with modification notices; the Apple test
uses a new module because the older snapshot lacks the newer test module.
The root workspace excludes the transport packages so their original registry
dependency graph is preserved. No local registry or `3pp/` patch is used.

The components, assets, and `gpui-base` remain at 0.7.0, with a single core
GPUI source/type identity. This is not an unrelated Zed upgrade. Con contains
neither the experiment's environment flag nor its per-frame debug logging.

Native linked snapshot tests passed: 348 core, five Apple, and 12 macOS tests.
An initial unchanged spring-animation timing test failed; the full rerun passed.
These 365 results are separate from the 395 newer official-source results above.

Con's locked remote dependency graph passed all 558 app tests with the short
isolated temporary directory. The app and companion CLI also built natively
against byte-identical snapshot sources in the isolated validation worktree.
The final integration has no experimental flag or presentation debug logs.

A separate Con Presentation Preview, with private home/session/history/socket
paths, exercised rapid reversals of all three chrome regions, zoom/unzoom,
nested splitting and closing, and tab creation/switching/closing. Terminal
input after layout changes produced the expected output. Settled rendering was
inspected; this smoke check is not a high-speed recording, proof of 120 FPS, or
acceptance for unavailable Windows/Linux hardware. The earlier reporter's
Con Sync Preview and production sessions were not replaced or stopped.

## Temporary Fork Exit Criteria

Before production integration, record the public repository, full immutable
revision, upstream submission URL, and exact snapshot/package identities here.
The authorization is not evidence that those steps have already happened.
Retain the normal asynchronous path and non-macOS default behavior; do not
bundle an unrelated upstream upgrade into this fix.

Run affected Con tests and inspect the integrated app, not just the experimental
binary. The final snapshot's full Con suite passed with the isolated short
temporary directory above. Windows/Linux native CI remains required; macOS
tests cannot establish their runtime behavior.

Once official compatible packages contain the accepted fix, remove the fork
override, upgrade GPUI and paired components together where necessary (including
assets and `gpui-base`), and repeat interaction and performance checks. Keep
upstream acceptance, package availability, and Con delivery as separate states.
