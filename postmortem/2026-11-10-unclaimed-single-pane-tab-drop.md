# In-window tab drops could be discarded before splitting

## What happened

The user reported that dragging another tab into a single-pane destination was
unreliable, while adding a second destination pane made the interaction work.
The user later confirmed horizontal tabs and could no longer reproduce it.
Horizontal-tab single-pane transfers also succeeded locally; vertical compact
drags were timing-dependent. The exact original trigger remains unconfirmed. This is an event-delivery boundary rather
than a reason to require an existing split.

## Root cause

The pane container committed only a bubble-phase on_drop whose hitbox needed
to be hovered. Native terminal descendants or stale last-move hit state can
prevent that callback. The macOS release observer nevertheless treated every
in-window release as already handled and unconditionally discarded any pending
drag, losing a valid final position. Split/titlebar/gutter hit targets could
mask this dependency. Model layout alone did not validate event delivery.

## Fix

Observe left-button release at window capture, resolve a valid pane split from
the final event position, and consume the active drag before committing. Keep
normal tab reordering, invalid/self/cross-window drops and explicit new-window
affordances under their existing rules. If AppKit receives a release that GPUI
misses, resolve the same split from its recorded release position before
cleanup. Cancellation wins and a consumed drag cannot commit twice. Share the
commit path with the ordinary pane drop handler. No native frame, alpha, clip,
material or animation timing changes.

## Verification

A linked GPUI event test covers a single full-size occluding child which stops
bubble-phase mouse-up, an inside release without a preceding move to its final
position, outside release and non-left buttons. Workspace and pane transfer
regressions plus an isolated native preview cover destination topology and live
terminal identity. Exact check results and platform limits accompany delivery.

Local verification passed 220 workspace tests, 34 pane-tree tests and the app/CLI
build. The fixed native macOS preview passed compact vertical slow single-pane
merge, a two-event fast merge of a split source into one pane, and a merge into
an existing three-pane destination. Original foreground PIDs and TTYs survived
the first merge unchanged. Before the fix, the same compact slow gesture left
both tabs unchanged. Windows/Linux native drag runtime and remote-desktop
framerate scenarios were not exercised.

The final horizontal-tab single-pane native drag also passed. The explicit new
window drop still opened a separate window. Test previews were quit and their
process exit verified. This hardens a demonstrated release-event gap, without
claiming the user's original intermittent horizontal failure was reproduced.
