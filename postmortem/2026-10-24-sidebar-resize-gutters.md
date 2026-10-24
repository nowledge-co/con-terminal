# Sidebar resize targets missed the card gutters

## What happened

The expanded Sessions sidebar could not be resized. The agent panel's resize
strip sat inside the panel rather than in the gap beside the terminal and
painted a full-height hover fill inconsistent with the card surfaces.

## Root cause

Only the Files/Search visibility branch mounted the left resize target, even
though Sessions already had a stored width and shared drag handling. The right
hit target was a child of the clipped agent container and was offset into its
content. Sidebar maximum width was available but not updated during rendering.

## Fix

Mount the left target for expanded Sessions as well as Files/Search. Preserve
the fixed compact rail and the user's stored width. Clamp the rendered width to
the current terminal/agent budget; start each drag from that visible width.

Place both targets above the layout, centred in their surface gutters, with the
same short hover/drag grip as pane splits. Keep the larger hit target transparent
and preserve root mouse capture, focus, release handling and session persistence.
Reserve the left sidebar when constraining an agent-panel drag. Remove the
legacy full-height opaque seam covers during pressed resize; changing the
hover grip alone leaves that older active-state paint path behind.

## Verification and lessons

The GPUI regression test checks actual 7pt hitbox bounds and mouse delivery
at both edges, including the overlap with clipped/occluded neighbors. The full
app suite passed 697 tests. Native isolated previews exercised Sessions and
Files dragging, Search, the fixed compact rail, a 950pt window, split panes and
both Flexoki themes. Session JSON recorded the resized sidebar and agent widths.
The rendered sidebar shrinks to the available budget without overwriting its
saved preference. A resize callback that exists in source is insufficient
when its view is conditionally omitted or clipped by another surface.

Portable CI also caught `cfg!` expressions referring to the macOS-only canvas
depth. `cfg!` does not remove branches from type checking. Select the geometry
with `#[cfg]` bindings and shared frame constants instead; native/macOS checks
alone cannot validate those portable compilation paths.
