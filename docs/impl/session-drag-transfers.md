# Moving terminal sessions by dragging

Drag a tab onto a different tab's terminal area to join its layout. The preview
shows which half of the target pane will receive it. A tab containing several
panes keeps its internal layout, ratios and surface stacks. Hover another tab
briefly during a drag to reveal that tab's layout first.

Drag a pane's title (or a surface label within its header) onto the tab strip or vertical tab list to give it its own
tab. The entire pane moves, including inactive surfaces or unsaved editor state.
The existing active surface and surface order stay intact.

Drag a tab or pane outside its source window and release to move it into an
independent window. On macOS, a quiet title-and-action preview follows the pointer
outside the window; it cannot take focus or intercept the drop. Moving back into
the source returns to ordinary split/reorder targets. A 12-point outer margin
prevents edge jitter from accidentally detaching a session. Escape cancels,
including while the pointer is outside; deactivating the app cancels as well.
The in-window **New window** drop destination remains available.
Escape cancels without changing the layout. Dropping on the source pane is a
no-op; ordinary tab reordering continues to use the tab strip/list.

## Continuity and boundaries

These are moves, not close/relaunch operations. Terminal entities, PTYs,
scrollback, native agent identity and saved conversation references remain live.
New-window creation succeeds before the source relinquishes anything. The
original window closes when its last tab moves out. Existing windows do not
accept each other's window-local drag payloads.

A running built-in con request has window-bound approval/tool callbacks, so
finish it before moving its tab to another window. Native agents such as Codex
and Claude Code can move while running. A tab with built-in con conversation
history may move intact to another window; joining it into another tab's panes
is declined because a pane has no independent built-in conversation panel.
Quick Terminal retains its existing ownership rather than detaching sessions.

Native shell integration keeps its original socket address and terminal context
when a terminal moves. Reports for that exact context route to its new owner.
If the socket-owning source window closes, a destination inherits the socket
and its request receiver. The socket handle retains its existing runtime and
listener cancellation authority; source closure does not stop that executor.
Positional CLI tab/pane addresses still refer to the
workspace owning that socket; this change does not introduce cross-window CLI
addressing.

## Implementation contracts

- Extract/insert whole pane nodes; do not reconstruct a stack from active-first
  surface lists. Remap incoming pane, surface and split IDs when joining trees,
  and move pane-scoped history and runtime observations with their new IDs.
- The drag constructor shares its source-window identity with GPUI's original
  payload; changing only the preview entity does not change the drag payload.
- Store subscriptions with each terminal/editor and replace them when its window
  ownership changes. Stale bootstrap callbacks must not focus or initialize an
  entity no longer owned by their workspace.
- Resolve the final drop from the release position as well as from live preview.
  The first move crossing the drag threshold may also be the final move.
- On macOS, reparent the existing host without destroying the Ghostty surface.
  Hide it until destination layout commits its actual frame and clips, renew
  its backing/occlusion observer against the new NSWindow, and adopt the new
  parent's transition underlay. Preserve the frame transaction documented in
  `skills/native-surface-coverage/SKILL.md`; do not mask transfer artifacts with
  opaque fills, borders or shadows.

## Window-outside release ownership

An element mouse-up-out handler depends on hitbox hover state and is not the
authority for finishing a drag outside its window. The workspace listens at the
window capture phase. On macOS a gesture-scoped AppKit observer records the exact
release point and Escape before dispatch; a main-thread task samples it only
while the gesture is alive, with `pressedMouseButtons` as the missed-release
fallback. No global event tap, Accessibility permission or profile changes are
involved. The observer never calls Rust/GPUI reentrantly.

The capture and preview are RAII-owned by the source workspace and released on
every completion/cancellation path. A per-gesture token prevents an older task
from completing a newer drag. Claim completion before opening the destination,
so a native release and GPUI drop cannot each move the same tab. Outside release
positions the new window near the pointer, constrained to the display’s visible
bounds. The original transfer readiness, failure rollback and native-surface
transaction contracts still apply. Linux/Windows use window-level pointer event
capture; macOS native preview/observer code is not compiled on those platforms.

Run `bash scripts/macos/test-session-drag-capture.sh` for the linked AppKit
release/cancel/reentry/cleanup tests. They replace input getters and the local
monitor only inside their own test process: no events are posted and no windows
are shown. The macOS E2E workflow runs them before building con. Rust event
tests separately cover a release without an outside move and a fast release
before the New window target paints.
