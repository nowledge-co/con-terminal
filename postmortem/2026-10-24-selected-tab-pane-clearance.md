# Selected horizontal tab to pane clearance

## What happened

The top gap looked wider than the lateral inset and split gutters, although the
frame calculation and its test both reported 5pt.

## Root cause

The calculation measured the outer tab tray. The selected capsule is inset by
another 2pt, so its visible bottom was 7pt above the pane. The old test used the
same outer boundary and could not detect this visual mismatch.

## Fix

Share the selection inset between tab painting and canvas geometry. Measure
5pt from the selected capsule, moving the pane up by 2pt. Expand the viewport
clip into the unused bottom of the caption only as far as the pane needs;
matching padding preserves the original content origin. Let the adjacent agent
column stretch from the same top rather than shifting its bottom baseline.
Keep the caption height, traffic lights, tab targets and pane split geometry.

## Verification and lessons

The frame test measures from the selected surface. A GPUI layout fixture checks
pane and agent top/bottom alignment and ensures the pane top stays inside the
expanded clip throughout tab reveal. Native light/dark horizontal-tab and split
previews verify the result. Compare painted boundaries, and inspect overflow
clips whenever a negative margin reclaims space above a layout boundary.
