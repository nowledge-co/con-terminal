# Terminal selection remained active after releasing over the sidebar

## What happened

Dragging a selection from a terminal into the sidebar and releasing the mouse
left Ghostty's selection gesture active. Moving the pointer back over the
terminal continued the old drag, and the next click cleared the selection.

## Root cause

GPUI's `on_mouse_up` handler only runs while the pointer is inside the
element's hitbox. The macOS terminal sent Ghostty a button press but did not
track ownership of that gesture, so a release over another element was never
forwarded. Right-button handling also used Ghostty's consumed result as the
pairing condition even though a non-consumed press still updates Ghostty's
mouse state.

## Fix applied

Each terminal view now records the mouse sequences it starts and handles both
inside and outside releases. Release and cancellation paths clear ownership
before forwarding one matching release, so unrelated panes cannot receive a
stray event. Left and right buttons keep independent ownership, so restarting
one does not end the other's active gesture. Synthetic releases retain the
press modifiers. Windows and Linux keep their existing move-based recovery
while also closing supported gestures at the real outside-release position.
Windows also owns Shift-bypassed local selection while terminal mouse tracking
is active.

## What we learned

Mouse gesture ownership belongs to the view that sent the press, not to the
element under the pointer at release time. A terminal's consumed result is a
host UI decision, such as whether to show a context menu; it is not proof that
the terminal did or did not begin internal button state.
