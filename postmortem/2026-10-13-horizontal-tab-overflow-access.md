# Horizontal Tab Overflow Access

## What Happened

After creating several tabs, selecting the newest tab revealed the right end
of the strip and clipped tabs on the left. There was no visible list of tabs,
so recovering an offscreen tab depended on discovering a scroll gesture or
knowing a keyboard shortcut.

## Cause

Equal-width tabs stopped shrinking at 140 points. Overflow scrolling and
selected-tab reveal existed, but those mechanics alone did not provide a
discoverable pointer path to every tab.

The real tab targets also occluded the title-bar drag hitbox. That blocked the
ancestor scroll listener. The original layout fixture omitted occlusion, so
its passing scroll test did not represent the native tab strip.

## Change

Keep equal allocation and the 200-point maximum, but allow shrinking to 120
points before overflow. Add a compact, scrollable All tabs menu with full
names and an active-item check. Resolve choices by session ID at click time,
so closing or reordering tabs cannot redirect a menu choice. Preserve the
caption drag gap and explicit-selection reveal. Forward wheel events from
occluded tab targets to the shared scroll handle, preserving pixel deltas and
clamping both ends. Occlude the menu's outer trigger rather than its inner
button, so the component can open without starting a native window drag.

Paging controls occupy stable gutters outside the scroll viewport. A previous
visual prototype placed a gradient over clipped labels; it was rejected because
the leading text appeared erased. During overflow, paint the resting tray fill
on the fixed viewport and rounded gutters instead of clipping a moving fill.
Native inspection also found the left caret asset was absent even though its
button had a working hitbox. Bundle both matching Phosphor carets and test their
asset-source resolution; interaction-only tests cannot catch an invisible icon.

## Verification

Tab layout tests cover width allocation, the caption drag gap, selected-tab
reveal and manual-scroll retention. Exercise both line and pixel wheel events.
The macOS preview verified selecting an offscreen first tab from the menu and
wheel scrolling over a real tab. Edge-control tests cover paging to both ends
and disappearing when tabs fit. Windows/Linux runtime checks remain unavailable.

## Lesson

Overflow needs a visible escape route. Scrolling should supplement direct tab
selection, not be the only way to recover a hidden item.
