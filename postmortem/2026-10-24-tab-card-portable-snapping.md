# Stacked-card contours referenced a platform-gated helper

## What happened

The tab-card outline passed local macOS checks, but Linux and Windows UI
compilation failed in CI at the new `assets.rs` pixel-snapping call.

## Root cause

The bitmap/vector identity renderer is shared across platforms. Its new contour
called `terminal_geometry::snap_terminal_bounds`, whose module was absent on
Linux and whose function was absent on Windows. macOS checks cannot detect
these missing symbols.

## Fix and lessons

Compile this pure GPUI geometry module and its endpoint-snapping function on all
platforms. Keep the Windows retained-frame helpers under their existing cfgs.
There is no new native linkage or FFI. Existing snapping tests cover the same
calculation and rounding behavior. Portable CI must validate the new head;
local macOS success alone does not establish that coverage. Inspect both the
module gate and the item's gate before reusing helpers in shared UI code.
