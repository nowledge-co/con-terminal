# Files and Search bypassed sidebar motion

## What happened

Clicking Files or Search changed the reserved sidebar width and its contents
immediately. The file-tree root also started above the rail's first control.

## Root cause

The drawer's boolean state directly selected the width and rendered view.
Existing sidebar springs only covered the navigation and whole-panel visibility.
The tool column and rail also computed their leading space independently.

## Fix

Align the root row to the rail's first control. Give tool reveal its own channel
on the existing sidebar spring, complementing the navigation channel on one
clock. Caption geometry and pane-resize offsets read their combined live extent.
For opaque terminals, keep the existing native transition underlay active while
that extent changes; glass terminals retain their configured alpha.

Reveal a stable tool viewport and briefly crossfade the retained tool entities.
Persist tree/navigation scroll handles, keep the search query, and move focus off
an outgoing query before it can accept hidden input. Initial/hidden presentation
and Reduce Motion settle directly; reversals retain their current state.
Dismiss a file popup when its tool hides, restoring focus only if it still owns
focus, so retained views cannot resurrect stale menus.

## Lessons

Animate presentation state rather than replacing views at a boolean boundary.
Test pinned swaps, compact dismissal, rapid reversal, saved content state and
reduced motion. Preview fixtures must contain files and nested directories so an
empty test folder cannot be mistaken for a broken expansion control.
