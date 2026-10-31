---
name: native-surface-coverage
description: Prevent and investigate backdrop leaks, seams, stale clipping and rounded-corner artifacts when changing con pane/sidebar geometry, animation, transparency or native terminal composition.
---

# Native surface coverage

Preserve con's glass material while keeping each moving pixel covered by its
correct owner. A darker result, passing layout test or clean log is insufficient.

## Identify the failure before choosing a fix

Record direction and trigger, tab orientation, compact/pinned/hover state,
pane/surface layout, input-bar visibility, theme, terminal/UI opacity, blur,
macOS version, display scale and release versus preview binary.

| Observation | Investigate |
| --- | --- |
| Different backdrop colors visible through adjacent surfaces at rest | Intended transparency; compare the same position and backdrop |
| A stable hairline or jagged curve | Physical-pixel endpoints, shared corner coverage, overlapping alpha |
| A broad moving strip, especially one that also clips text | Native frame/mask/GPUI presentation timing; measure successive video frames |
| Correct model bounds but wrong native output after reuse | Attached masks, native backing-layer replacement and actual framebuffer dimensions |

Remote desktop or OS differences are verification limits, not presumed causes.
For a transient report, inspect a recording before treating a helper-level bug
as the root cause. A strip advancing with per-frame travel is evidence of a
presentation mismatch, not a reason to thicken its edge.

## Trace the real rendering path

Read the relevant sections, not every historical investigation:

- [Workspace composition](../../crates/con-app/src/workspace/render.rs) and
  [frame geometry](../../crates/con-app/src/workspace/frame.rs).
- [Pane ownership](../../crates/con-app/src/pane_tree.rs),
  [native host](../../crates/con-app/src/ghostty_view.rs) and
  [AppKit masks](../../crates/con-app/src/objc/ghostty_surface_trampoline.m).
- [Native presentation contract](../../docs/design/native-terminal-presentation.md).
- For cache/opacity changes, [GPUI cache guidance](../gpui-cache-aware/SKILL.md).

Write down each pixel's owner and when its geometry becomes final. Check actual
callback order: CanvasSurround currently precedes terminal content. GPUI
`.occlude()` controls hit testing, not visual coverage; `.overflow_hidden()`
does not itself clip an AppKit terminal sibling.

Keep these independent contracts intact:

- Shared canvas changes refresh registered native masks in the same frame,
  including already-prepainted or idle consumers. Local pane clips must not
  escape into siblings, shared underlays or GPUI itself.
- Host reuse after split closure, zoom or tab/surface changes still executes
  native clip prepaint. A Metal-backed descendant does not imply parent backing;
  AppKit may replace a layer without changing model bounds.
- Native and GPUI corner coverage share snapped physical endpoints and a raster.
  Omit individual zero-area corner tiles near animation endpoints; do not discard
  other visible corners or suppress real raster-budget errors.
- A geometry-correct native mask and correctly sized IOSurface still need to
  **present with the matching GPUI scene**.

## Restore hidden native conversations without exposing bootstrap geometry

Ordinary control initialization still requires authoritative canvas bounds.
Native conversation resume has one narrow exception: an inactive restored
surface can initialize from its previously confirmed physical viewport size so
its shell and native agent can start without selecting the tab. Keep that host
hidden, keep `awaiting_first_layout_visibility` set, and discard the bootstrap
bounds as current layout. Before activation exposes the host, the real canvas
must commit the current frame, mask and clipping through the ordinary frame
transaction. Never generalize this exception to estimated split/zoom bounds.
Verify inactive tabs and stacked surfaces after restart, then activate them;
verify closing a split and switching tabs still use current geometry.

## Keep native edits and platform presentation in one transaction

The accepted Con boundary is `NativeGeometryTransaction::hold_for_frame`, opened
by CanvasSurround before terminal children. It spans native frame/mask changes,
synchronous Ghostty drawing and GPUI's platform presentation. A retained pending
flag coalesces opens, and RAII closes the transaction through App deferred effects.

This relies on the **resolved GPUI implementation**: its frame callback runs
`window.draw(cx)` and `window.present()` within one App update; deferred effects
flush after that callback returns. Recheck that ordering on GPUI upgrades.
`Window::draw` alone builds a scene; it is not proof of platform presentation.
Do not assume every defer callback runs after presentation in arbitrary callers.

Preserve the existing frame-scoped presentation request as well. Repeating that
request at paint does not reunite native transactions already committed during
prepaint; this approach passed tests but failed maintainer visual acceptance.
Do not replace the transaction boundary with `on_next_frame`, longer timers or
extra redraws. Close within the current update, restore the caller's animation
context, suppress a second native implicit animation, and keep ordinary output
on the asynchronous renderer path. Do not modify pinned dependencies or registry
sources to manufacture a passing preview.

## Preserve material and ownership

Keep configured alpha, blur, rounding and animation timing. The opaque transition
underlay is for opaque terminals only. Do not cover a translucent terminal with
it, enlarge masks, add full-pane fills, overlap translucent strips or add borders
and shadows to disguise the gap. Keep surround paint narrow; viewport-sized paths
with holes still incur large Metal blits.

## Verify the failing boundary and the moving result

Choose linked tests for the changed contract:

- `native_frame_transaction_stays_open_through_paint_and_closes_after_update`:
  transaction lifetime and coalescing; no wait for another frame/user action.
- `native_geometry_transaction_does_not_inherit_or_escape_implicit_animation`:
  inherited animation suppression and restoration of the caller's context.
- `late_canvas_prepaint_refreshes_registered_masks_in_the_same_frame`:
  late canvas changes refresh actual attached paths; unrelated siblings stay free.
- Corner/reuse tests in `workspace/frame.rs`, `ghostty_view.rs` and `pane_tree.rs`:
  mixed visible/empty corners, fractional radii, alpha/scale endpoints and the
  actual render path after topology changes.

Run affected linked macOS tests and Rust formatting with the pinned toolchain.
Type checking cannot prove FFI linkage, attached-mask correctness or presentation.
Match the reporter's binary/configuration/backdrop in an isolated home/session;
near-opaque Flexoki with input visible does not validate translucent Kanagawa
with input hidden. Exercise **Cmd+B and Cmd+L themselves**: compact↔pinned,
hover pinning and tools are different paths. Include the affected opposite
transition, rapid reversal, pane/surface variants and Reduce Motion as appropriate.
Use a bright contrasting backdrop when the report has one. Never alter the
user's live configuration/session or OS appearance for a fixture.

Confirm moving frames, not only endpoints. Green tests, correct texture sizes,
empty coverage-error logs and sparse automatic screenshots are supporting
evidence, not visual acceptance. If automation misses an intermittent defect,
use the supplied recording and maintainer reproduction to close the loop.
Record the exact accepted build/configuration and delivery state. Scope claims
to actual OS/hardware: no inferred FPS, 1x, Intel, remote-desktop or Windows/Linux
certification. Once the relevant checks and matched acceptance pass, stop testing
unless new changes, failures or feedback justify another pass.
