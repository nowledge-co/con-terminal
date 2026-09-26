# Activity animation lost clipping information during parent rendering

## What happened

Review of the new shared tab activity layer found that rebuilding tab rows
cleared all marker bounds before animation eligibility was evaluated. A clipped
Busy row could therefore continue requesting frames. This was found before the
animation change was committed.

## Root cause

GPUI render decides whether to attach the animation before the row's prepaint
supplies its new bounds. Treating an unmeasured marker as eligible is necessary
on its first frame, but clearing retained bounds made every parent render look
like a first frame.

## Fix applied

Keep marker geometry across row rebuilds, track the currently registered prefix,
and replace bounds/masks at prepaint. A change in visible intersection requests
reevaluation. Removed rows no longer participate. Determinate and attention
markers never request a repeating pulse.

## What we learned

Layout-derived animation eligibility needs last-frame geometry as well as
current-frame membership. In the removal experiment, replacing retained bounds
and masks with `None` made the regression test fail; restoring retention made it
pass. This proves the retention contract, not an end-to-end scroll benchmark.

An eight-second macOS sustained-Busy sample recorded 169 animated layer renders,
zero chrome preparations, zero identity screen scans, seven process batches and
three summary polls. This establishes separation of these cadences in that
scenario, not a general CPU/GPU performance claim.
