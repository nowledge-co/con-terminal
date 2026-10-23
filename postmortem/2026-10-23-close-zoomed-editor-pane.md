# Closing a zoomed editor left surviving terminals hidden

## What Happened

Closing a file pane while it occupied the window left unused space where
surviving terminal panes should have appeared. Explicitly unzooming first did
not show the same behavior.

## Root Cause

`PaneTree::close_pane` cleared the removed pane's zoom state correctly, but the
workspace's editor-specific close branch returned before running the native
layout invalidation and visibility synchronization used for terminal closes.
macOS terminal NSViews live outside the GPUI element tree: rebuilding the pane
tree does not itself reveal terminals that were hidden by editor zoom.

That branch also skipped saving the changed pane layout and focused the first
surviving terminal rather than respecting the tree's surviving focused pane.

## Fix

Editor-pane removal now marks native terminal layout pending and uses the
existing zoom-aware post-layout visibility path. It respects surviving editor
focus and saves the resulting layout. No matte, opaque cover, renderer change,
or new presentation timing mechanism is added.

## Verification

The regression test closes a real editor leaf while zoomed and checks restored
zoom state, focus and full survivor bounds. An isolated macOS preview also
verified zooming and closing a file pane with one and two surviving terminals:
the native surfaces reappeared and filled their restored layout. A tree-only
test cannot prove that NSViews repaint.

The app suite passed 683 tests with two unrelated, previously flaky native
file-watcher tests excluded. Strict app Clippy and Rust formatting passed.
The layout/focus path is shared, while native layout invalidation remains macOS
only. Windows/Linux require their native CI paths; a macOS build is not evidence
of native rendering behavior on those platforms.

## Lesson

Editor and terminal panes share layout lifecycle responsibilities even though
only terminals need PTY shutdown. Early returns for content-specific cleanup
must not bypass layout, visibility, focus or persistence work.
