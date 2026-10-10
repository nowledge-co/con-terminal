# macOS Terminal Handoff Flash

## What Happened

With terminal transparency enabled, creating tabs, creating split panes, and
switching tabs could briefly make the terminal look opaque.

## Root Cause

Two host-side paths could composite terminal backgrounds twice:

1. `GhosttyView::render` selected a full-pane GPUI fallback before native surface
   initialization. Canvas prepaint then created, positioned, drew, and revealed
   the Ghostty surface. The already-built parent still painted its fallback over
   the native terminal until the next GPUI render. With 85% background opacity,
   two such layers accumulate to 97.75% opacity.
2. Tab activation revealed incoming native views immediately but hid outgoing
   views in an `on_next_frame` callback. Both tabs therefore contributed their
   translucent backgrounds during the handoff. Rapid A → B → A activation could
   also leave a queued callback that hid the newly active tab.

These are observable ordering defects in Con's rendering and visibility code,
not evidence that Ghostty needs a different background-opacity setting.

## Fix

- Paint the initial fallback inside the terminal canvas, using the pending
  state returned after native layout. Keep fallback coverage when initialization
  is still pending or failed, with the configured alpha unchanged.
- Exchange macOS native visibility at the pane tree's prepaint-completion
  boundary, after incoming layout and before GPUI paint. Preserve the outgoing
  frame until then, hide every inactive tab, and reveal only the latest active
  tab. Remove the macOS next-frame hide callback. Retain the existing
  Windows/Linux scheduling.
- Do not add a matte, timer, animation, draw loop, or opacity override. Preserve
  native geometry synchronization, blur, focus handling, and the Monterey
  compatibility backing.

## Verification

GPUI scene tests cover successful initialization within the same frame (no
fallback quad), pending initialization (one fallback at the requested alpha),
and a later successful retry (fallback removed). A separate test verifies that
the pane tree's prepaint-completion callback runs between incoming canvas layout
and paint.

Native macOS build and 554 app tests passed with serial test execution. The
initial parallel run hit timeouts in unrelated native file-watcher tests;
those tests passed separately and in the serial full-suite run. In an isolated
dark, 85%-opacity preview, six A/B switch cycles, a new tab, and right/down
splits retained a working terminal; the control socket reported all three new
surfaces ready and alive. Windows/Linux physical validation is unavailable.
A scene test and settled screenshots do not prove every WindowServer timing
case, so single-frame flash acceptance still needs fast real-window observation.

## Lessons

Treat initial coverage and live terminal content as mutually exclusive paint
owners. Decide coverage after layout, not from a pre-layout snapshot. Deferred
visibility work must not outlive the selection state that scheduled it.
