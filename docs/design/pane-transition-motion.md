# Pane Transition Motion

Status: implementation study, not a shipped animation feature.

## Reference Direction

The maintainer's Rex reference recording (2026-10-13) shows the important
relationship: a newly split region takes space from its neighbor, and closing
it returns that space. Existing content remains legible throughout. This is a
spatial transition, not an independent fade on each pane. Match that continuity
without copying its visual design or assuming its rendering implementation.

For tabs, animate slot redistribution together with arrival/departure, rather
than fading a label in a slot that already jumped to its final size. For panes,
resolve the presentation-versus-grid contract below before choosing an easing
curve. A pleasing endpoint does not validate intermediate terminal frames.

## Current Boundaries

`PaneTree::render_node` lays out splits with flex bases and a one-point divider.
Splitting adds a leaf immediately; closing removes it immediately. macOS embeds
native terminal layers under GPUI. `GhosttyView::update_frame` synchronizes
backing dimensions and requests frame-scoped native presentation for geometry
changes. Windows `RenderSession::resize` resizes the renderer, VT, mouse geometry
and ConPTY. Linux `resize` also updates the VT and PTY.

Consequently, interpolating the real split ratio is not a cheap decorative
animation: it can repeatedly change the terminal grid and trigger TUI redraw.
The existing native presentation synchronization fixes compositor consistency,
not that application-level resize cost.

## Recommended Sequence

1. Finish per-tab chrome motion separately. Logical closure stays immediate;
   decorative exit state must never own a terminal process.
2. Establish a pane transition presentation layer with final logical bounds
   committed once. Keep cell geometry, selection and mouse reporting truthful.
3. Animate only presentation if the backend can provide a retained, correctly
   clipped frame without stretching text, readback per frame, or changing PTY
   size per frame. Disable interaction with an outgoing decorative frame.
4. Validate each native backend before enabling terminal-body motion there.
   No capable backend means an immediate transition, not an opaque cover,
   live-terminal fade or stretched screenshot fallback.

macOS may need an upstream presentation-only layer primitive; GPUI transforms
do not automatically transform its embedded AppKit terminal views. Windows and
Linux participate in GPUI's scene, but their retained terminal frames, resize
readiness and mouse-coordinate contracts still need explicit validation. The
current code does not establish that this primitive exists across all ports.

## Acceptance

Count PTY/grid resizes during a transition, inspect intermediate frames rather
than just endpoints, and measure a content-heavy TUI. Include nested splits,
rapid open/close, zoom, restored panes, IME, selection, transparency, multiple
displays and Reduce Motion. Idle windows must not request animation frames.

See [native terminal presentation](native-terminal-presentation.md) for the
existing macOS synchronization contract and its platform scope.
