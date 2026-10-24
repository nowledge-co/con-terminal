# Input tray material and delayed terminal expansion

## What happened

The macOS command bar looked brighter and more opaque than its neighboring
terminal pane. Hiding it faded the bar first, then expanded the terminal at the
animation endpoint, making one action feel like two stages.

## Root cause

The tray used a separate white/foreground lift over title-bar chrome and a full
rectangular chrome backing. Its translucent fill therefore compounded with the
backing instead of sharing the terminal's base color and opacity.

The input reservation deliberately stayed at its final height while the tray
faded. This stabilized the old terminal viewport, but also deferred all space
reclamation until the fade ended. The agent panel inherited the same delayed
bottom clearance.

## Fix

Use the focused terminal's background and opacity for the input card. Paint
workspace chrome only outside that card. The tray uses the native terminal's
exact corner coverage data in cached, tinted physical-pixel tiles, disjoint
body fills and the same complementary cutouts and fine contour. This avoids
composing a separately antialiased rounded fill over another corner raster.

Sample one spring for the terminal reservation, agent-panel clearance and
input reveal. Slide fixed-size content from the reveal's top through its clip
instead of scaling it. Keep material alpha constant throughout the movement. Preserve
focus transfer, reversal velocity, reduced-motion endpoints and frame-scoped
native presentation synchronization. Do not add a separate animation clock.

## Verification and lessons

Rendered-geometry tests cover intermediate progress, closing and reversal,
fixed content dimensions, and the absence of a final viewport jump. Assertions
allow one physical pixel of layout rounding rather than requiring fractional
Taffy dimensions to survive raster alignment.

Native tracing on the old preview showed only the starting and ending terminal
heights for a hide. The candidate recorded 16 successive intermediate heights
(717.5pt through 764pt), with individual native frame updates taking 0.38–0.97ms.
This measures frame geometry and host update cost, not end-to-end frame rate.
Multiline text survived hide/show; native previews covered split panes and both
Flexoki themes. These changes target the macOS card path;
portable renderer coverage remains a separate CI/native check.

Transparent surfaces can look different over different desktop regions. Do not
infer a broken material from that difference alone. Preserve shared terminal
opacity, concentric radius and fine contour; do not add opacity to hide a real
backdrop difference or replace the existing contour without rendering evidence.

A shared radius and contour do not guarantee shared material: inspect the
entire stack of fills beneath a translucent surface. Keep layout and visual
progress together whenever the surrounding content must reclaim space during
a transition.

## Follow-up from native review

The first candidate still multiplied the entire tray by reveal opacity. During
hide, that faded its glass material toward zero rather than moving the surface
below the viewport. Its independently antialiased quad also differed from the
terminal's shared corner coverage. Remove the fade, make the full-size card
follow the clip's top and share the actual coverage pixels. A regression checks
body/corner partitioning, native coverage alpha, BGRA encoding and texture reuse.
