# Terminal Pane Resize Coverage

## Report

Showing or hiding the sidebars and input bar could flash the window background
beside internal pane dividers, on Windows and macOS. The dividers themselves
were already opaque; changing their color or width was not a sufficient fix.

## Findings

- Windows builds its retained terminal image and resize-gap children during
  `Render`, using `self.pane_bounds` from the preceding prepaint. The current
  layout can grow without creating those gap children. Prepaint then updates
  the cached bounds, too late to change the current element tree. The old
  texture correctly keeps its original size, but newly exposed pixels can
  remain clear until the next render. A 0.5-point tolerance also deliberately
  omitted some gaps.
- The pinned GPUI renderer snaps both endpoints of quads and content masks to
  physical pixels, with half ties toward zero. The macOS host used raw floating
  pane bounds for native frame placement and size. Fractional split coordinates
  therefore did not necessarily end on the same pixels as the adjacent GPUI
  separator. This is a demonstrated geometry mismatch, not proof that it
  explains every native compositor timing artifact in the report.

## Changes

- Calculate Windows retained-frame gap fills inside canvas paint, from that
  frame's viewport. The right/bottom regions partition uncovered space without
  overlapping each other or the old texture. Keep the original alpha and do
  not stretch terminal text.
- Snap macOS host bounds using the pinned GPUI endpoint rule before native
  initialization/layout, including inactive surfaces synchronized by their
  pane host. Keep terminal sizes, hit testing, and native frames on those same
  bounds.
- Leave divider thickness, animation, blur, opacity, Linux rendering, terminal
  execution, and renderer readback scheduling unchanged.

## Verification And Remaining Acceptance

Tests cover growth and shrinkage, fractional gaps, coverage area, transparent
fill suppression, same-frame GPUI resize paint, adjacent pane/divider endpoints,
and equivalence with GPUI's actual quad snapping at 1x, 1.25x, 1.5x, 2x, and 3x.
Native Windows/Linux validation is unavailable; no cross-compilation is used.

All four geometry tests and both initial-fallback scene tests passed.
Early complete app runs timed out in unchanged native file-watcher tests.
With the final snapshot and a short, isolated temporary directory under the
user's home, all 558 app tests passed serially without exclusions or test-code
changes. A longer fixture path separately hit the existing Unix socket
`SUN_LEN` limit; shortening the isolated fixture resolved it. Preserve these
initial failures in the record rather than presenting only a successful rerun.

An isolated macOS preview uses Flexoki Dark, 85% opacity, and three nested
panes. Six repeated show/hide cycles per agent panel, sidebar, and input bar
kept the settled layout correct. This is interaction smoke coverage, not
frame-by-frame proof that every compositor flash is gone.

### Reporter Follow-Up: Still Reproduces

The reporter reproduced the flash in that exact preview on 2026-10-10.
The captured frame shows a light rectangle approximately one pane-title-bar
height at an internal boundary, plus a light vertical seam. This is wider than
the pixel-rounding discrepancy; the macOS report is **not fixed** by snapping.
The attachment remains a local reproduction artifact, not a tracked repository
image: `.context/attachments/kXNrVH/Screenshot 2026-10-10 at 11.14.17.png`.

Further inspection of the resolved dependencies establishes a presentation
gap in the host integration:

- Con updates native view frames and calls `ghostty_surface_draw` during GPUI
  prepaint. Ghostty's `Surface.draw` calls `renderer.drawFrame(true)`; Metal's
  synchronous presentation assigns IOSurface layer contents directly.
- GPUI's ordinary frame calls `window.draw` then `window.present`. Its Metal
  renderer defaults to `presents_with_transaction = false` and schedules
  `command_buffer.present_drawable` asynchronously.
- GPUI already supports transaction-synchronous drawable presentation using
  commit, `wait_until_scheduled`, and drawable `present`. The macOS backend
  enables it in `display_layer` and subsequent window activation callbacks,
  but Con cannot request it through the pinned public `Window` API.
- [Apple documents](https://developer.apple.com/documentation/quartzcore/cametallayer/presentswithtransaction)
  that asynchronous Metal presentation is not guaranteed to arrive in the
  same frame as other Core Animation content. That mechanism is consistent
  with the captured mixed boundary, but runtime acceptance must still verify
  the proposed synchronization fix.

Do not simply toggle `CAMetalLayer.presentsWithTransaction` through Objective-C:
GPUI also chooses the command-buffer presentation branch using its own Rust
state. Changing only the layer property leaves that branch inconsistent with
Apple's required presentation sequence. `on_next_frame` is not a presentation
fence either: the pinned implementation executes callbacks before the next
draw, not after drawable display.

See [the upstream interface proposal](../docs/design/native-terminal-presentation.md).
The production no-local-patches constraint still applies. The maintainer initially
authorized an isolated `/private/tmp/` GPUI experiment on 2026-10-10, without
changing production dependencies, registry packages, or `3pp/`.

That candidate built and launched as Con Sync Preview. Debug logs confirmed
that geometry updates and tab handoff use transaction presentation while
ordinary terminal input and scrolling do not request it. Initial interaction
checks preserved nested pane layout and transparent rendering. This is
evidence that the intended mechanism runs. The reporter subsequently accepted
the same preview on 2026-10-10: repeated attempts no longer reproduced the leak.
This closes acceptance for that isolated macOS setup, not Windows/Linux
validation. Con now pins the reviewed mechanism through a compatible public
snapshot at `655ce8bbc10e1f1ef3d1d5bdf5a3405008157792`; PR #460 tracks delivery.
The official-source proposal and original screenshot are published in
[Zed Discussion #65433](https://github.com/zed-industries/zed/discussions/65433).

- Run Windows portable CI before merging; shared helper tests on macOS do not
  type-check `windows_view.rs` or exercise D3D presentation.
- Test repeated left/right/bottom chrome toggles with nested splits and glass
  on macOS and Windows. Capture frames if a flash remains, distinguishing a
  clear gap from normal translucent terminal background.
- Do not close remaining compositor races based only on settled screenshots
  or introduce full-terminal opacity overrides to hide them.
- Request and validate an upstream frame-scoped presentation synchronization
  API. Re-test this exact macOS recording scenario after consuming a compatible
  official dependency version when available; retain the temporary public pin
  only until a compatible official package includes it.
- Treat Windows separately: its terminal readback images and chrome use one
  GPUI scene. The macOS native-layer diagnosis cannot establish the cause of
  a remaining Windows flash.

## Lesson

Coverage belongs to the current paint geometry, not a cached layout snapshot.
When GPUI shares a boundary with a native renderer, both sides must agree on
device-pixel endpoints as well as logical pane dimensions.

Correct coordinates are necessary, but not sufficient: native layer updates
and asynchronous drawable presentation can still reach the compositor in
different frames. The accepted preview addresses ordering through GPUI's
existing transaction presentation path, not through terminal opacity, thicker
borders, or timing guesses. Scope synchronization to geometry and visibility
changes so ordinary output and scrolling keep the asynchronous path.

Keep experimental acceptance separate from production delivery. The official
upstream candidate has four regression tests and passed native linked
verification (370 GPUI, 14 Apple, and 11 macOS tests; two existing timing-print
tests ignored). The maintainer subsequently confirmed human review and
authorized a temporary public, immutable-revision GPUI fork on 2026-10-10.
That exception does not authorize local patches or establish Windows/Linux
runtime acceptance. The compatible snapshot's 365 native linked tests passed;
the complete Con suite passed 558 tests. Keep upstream acceptance, package
availability, PR merge, and release as separate states; see the proposal's
fork exit criteria.
