# Window chrome steals keyboard focus

## What happened

The reporter could not type in the terminal after resizing, and dragging the
titlebar did not restore input. Clicking the terminal made typing work again.

## Root cause

The workspace root tracks a focus handle for shortcut dispatch and editor-only
fallback. In GPUI, tracking focus also installs default mouse-down behavior:
clicking a non-focusable descendant transfers focus to that root. Blank titlebar
space and sidebar/agent-panel resize handles could therefore replace the active
input's focus with a container that has no terminal text handler.

A real GPUI mouse-event test reproduced the titlebar path on the unchanged
focus-container implementation: clicking chrome changed terminal focus to the
workspace handle. This establishes the container defect.
Pane split dividers already prevent default focus and stop propagation.

## Fix

Keep the root handle, but prevent its default left-mouse focus in the bubbling
phase. Child input targets retain their normal focus behavior. Do not stop event
propagation or change window dragging, resize geometry, terminal rendering,
transparency, or native first-responder ownership. Do not force terminal focus
after every layout change, which would steal focus from other inputs.

## Verification

The regression test exercises production container construction with real mouse
events. It covers terminal and input focus across chrome/divider clicks, child
focus switching, and explicit workspace focus for editor-only fallback. It fails
before the default-focus guard is added and passes afterward. All 528 app tests,
Rust formatting, diff checks, and the native app/CLI build pass.

The first automated titlebar test was insufficient: input succeeded, but a
subsequent instrumented run showed that the synthetic titlebar gesture did not
actually move the native window. It must not count as native drag acceptance.
The reporter initially still observed a failure, so the container test alone
was not treated as proof that every reported path was fixed.

Temporary diagnostics recorded the GPUI focus, native first responder, window
frame, and activation events in an isolated macOS preview. The reporter then
manually moved and resized this exact diagnostic build and confirmed that
typing continued without clicking to restore focus. Logs confirmed actual
frame changes and that the native first responder remained GPUIView. These
diagnostics added no further focus repair and were removed after verification.
The discrepancy with the earlier failed retest remains unexplained; do not
attribute it to an old executable without evidence.

Automated outer-window resizing also changed the native frame and terminal grid
while retaining focus. Input-bar preservation is covered by the GPUI event test;
native Windows/Linux interaction remains unverified.

## Lessons

A shortcut ancestor is not necessarily a pointer-focus target. Check implicit
GPUI focus behavior before adding platform-specific focus restoration. Preserve
the input the user was actually using rather than choosing a new input after
window movement.

Native drag tests must verify movement, not just successful typing afterward.
Observe focus before sending another key, since window activation may restore
focus and hide a failure.
