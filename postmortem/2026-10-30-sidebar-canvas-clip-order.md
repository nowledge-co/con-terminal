# Sidebar motion could present a stale canvas mask

## What happened

Compact/pinned transitions could briefly expose the backdrop at a terminal edge.
The settled view looked correct, making static screenshots insufficient to
check the changing geometry.

## Root cause

Terminal children apply their native pane mask intersected with the shared
canvas clip during prepaint. CanvasSurround then updates the shared canvas clip
later in the same frame. The setter advanced its generation but left already
attached native masks unchanged until a subsequent terminal layout callback.
When the sidebar shrank, that previous narrower canvas could cut away pixels
inside the newly expanded terminal. Existing presentation synchronization does
not fix a mask that still contains the previous frame's geometry.

## Fix

After shared canvas geometry changes, immediately reapply it to registered native
consumers identified by existing mask/pane-clip state. This includes clean cached
hosts and native pane headers, while leaving GPUI and unrelated AppKit siblings
untouched. Retain the current frame-scoped presentation synchronization, pane
coverage raster, glass opacity and narrow surrounds. Do not add opaque covers.

## Verification

An isolated AppKit fixture reproduces host-prepaint-before-canvas ordering and
checks the actual mask path immediately after the late canvas update. Before
the fix it fails at the first shrinking-sidebar sample (canvas x=200→50);
after the fix the same repeated collapse/expand/reversal samples pass.
The linked Rust regression exercises that order and also asserts that an
unregistered sibling acquires no mask. It passed in the full macOS app run,
alongside 729 other passing tests. One caption endpoint test exposed a separate
real-clock exact-float assertion (0.99999976 versus 1.0); its endpoint fixture
was made deterministic and passed its focused rerun.

The app and companion CLI built successfully. An isolated 2x macOS preview at
background opacity 0.94 exercised compact/pinned switching, rapid reversal and
split panes in Flexoki Dark and Light; sampled edges were inspected and logs
contained no corner-coverage errors. These checks do not certify every moving
frame, 1x/Intel hardware, remote-desktop presentation or Windows/Linux behavior.

## Lessons

A global geometry generation invalidates a cache but does not update an already
presented mask. Finalize native consumers when the last geometry owner changes,
without waiting for another frame. Keep shared coverage, lifecycle, alpha and
presentation-order failures distinct. The project skill
`skills/native-surface-coverage/SKILL.md`, referenced by AGENTS.md, captures those
boundaries and the corresponding verification methods.
