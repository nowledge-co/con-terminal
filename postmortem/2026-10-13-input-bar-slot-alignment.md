# Input Bar Slot Alignment

## What Happened

During mode-icon polish, the bottom icon appeared to have unequal leading and
bottom insets. The workspace reserved a 44-point single-line input slot, but
the rendered row was 48 points tall in the GPUI layout test.

## Root Cause

The textarea retained its component-derived intrinsic height. A minimum height
on the row did not constrain it to the workspace's reserved slot. Adjusting
only the icon padding would have hidden the mismatch rather than fixing it.

## Fix

Give the textarea an explicit height from the same rendered-height calculation
used by the workspace, excluding row padding. Mode and submit controls retain
their shared 28-point size. The leading inset is 8 points, matching the vertical
inset in a single-line row. Anchor the target picker to the control and actual
input height rather than a separate fixed popup offset.

## Verification

The cached and uncached input-bar test checks equal insets and aligned controls
across all modes, one/two panes and three viewport widths. Picker geometry tests
cover narrow terminals, scaling and multiline heights. Native macOS preview
checks covered Flexoki Light/Dark and a three-line draft without submission.
Windows/Linux runtime checks were unavailable; no terminal backend code changed.

## Lesson

A later visual check found a second layout mismatch in the target picker: an
empty trailing flex item shrank the preset strip relative to the map. Remove
the spacer and use a shared full-width frame and split gap. A rendered-bounds
test now checks both outer edges and the equal-split midline at three widths.

Use one height calculation for reserved slots and their component content.
Test rendered bounds, not just size constants, when polishing compact controls.
