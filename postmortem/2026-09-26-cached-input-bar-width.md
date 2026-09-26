# Cached input bar lost its available width

## What happened

The bottom input bar showed its buttons packed at the left while placeholder
and input text disappeared. This followed the addition of view caching on the
terminal-status branch, not an established upstream GPUI regression.

## Root cause

The cache shell declared full width, but `InputBar::render` returned an auto-width
flex root. GPUI 0.3.6 lays cached contents out as an independent root using the
shell's available space; it does not transfer the shell's style to the contents.
Normal parent stretching had previously hidden the missing width contract.

## Fix applied

Declare full width on InputBar's rendered root as well. Keep the existing cache,
invalidation and transition policy. A real GPUI layout regression test exercises
Smart, Shell and Agent modes, cached and uncached, through narrow/wide resizes.
Before the fix the cached Smart input measured only 20pt at a 1100pt viewport.

## What we learned

A cache shell's bounds and its rendered subtree's sizing are separate contracts.
Check input width and send-button position, not just container height. Previous
status-icon screenshots included this defect, but verification focused on the
icons and missed the broken input surface. Pure height-formula tests did not
exercise the cached layout boundary.
