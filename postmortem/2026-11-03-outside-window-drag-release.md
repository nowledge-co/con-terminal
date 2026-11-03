# Outside-window drag release was not a reliable transfer boundary

## What happened

PR #546 added live tab/pane transfers, with a New window drop target and an
outside-window mouse-up fallback. The fallback was described as platform-limited;
user testing correctly identified that it did not meet the expected direct
tear-off interaction. The explicit drop target did not substitute for this path.

## Root cause

The fallback used a root element's `on_mouse_up_out`. That API is gated by the
hitbox's hover state, rather than owning the native gesture. Moving outside the
window can leave that state or the final event unavailable. The horizontal tab
identity also remained visually locked to its row after leaving the strip.

## Fix

Use window capture for the generic release path. On macOS own a short-lived
AppKit observation of release/Escape with a main-thread button-state fallback.
Keep it independent of GPUI's hitbox and frame cadence; do not call GPUI from
inside AppKit event dispatch. An inactive, non-interactive, shadowless preview
follows the pointer only outside the source window. Native capture ends on
cancel, in-window drop, successful transfer, failure, or source destruction.
A gesture token and early completion claim prevent double execution.

Keep the existing terminal/editor/PTY transfer transaction. The source is never
removed before a destination opens successfully. Reentering keeps ordinary drop
behavior, and an outer margin avoids edge jitter. New windows use the release
location and display-visible bounds.

## Lessons

Validate the natural gesture users expect, including a release outside a real
window and returning inside, rather than treating an in-window drop destination
as evidence of outside capture. Endpoint screenshots cannot prove cancellation,
release delivery or process continuity. Cross-platform runtime coverage must be
reported separately from compiling the shared implementation.
