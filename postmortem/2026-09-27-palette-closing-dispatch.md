# Command palette dispatch during closing animation

## What happened

The GPUI migration review found that two rapid Enter presses could emit the
selected palette action twice while the closing animation retained the view.

## Root cause

`select_action` marked the palette invisible but accepted subsequent activation
without checking that state. Rendering intentionally keeps the overlay alive
until its exit animation finishes.

## Fix applied

Refuse selection when `visible` is false. Keep the existing animation and input
contracts. A GPUI keystroke test wraps to the last option and presses Enter
twice, asserting exactly one event.

## What we learned

Animation lifetime and interaction lifetime are different. A closing view must
stop accepting activation before it disappears. Removing the visibility guard
in the regression experiment produced two `quit` events instead of one; the
guard is necessary, whereas separate one-use navigation wrappers were not.
