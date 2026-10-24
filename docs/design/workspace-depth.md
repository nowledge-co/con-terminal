# Workspace Canvas

## Visual hierarchy

On macOS, workspaces with sidebars or multiple tabs use an inset terminal
canvas with 3pt of breathing room. Its native viewport stays rectangular;
rounded corners belong to the terminal cards, not a second enclosing mask. Terminal cards,
including a single pane, use 2pt insets and the same 8pt corners within that
canvas. In clean mode, a single terminal returns to zero inset and zero radius.
A card corner that meets a rounded window corner (pane, input tray or agent
composer) shares that corner's centre: its radius is the window radius, read
from AppKit, less the card's clearance from both window edges, and never below
8pt. Other corners keep 8pt. Fullscreen windows have no corner to follow. The
radius is derived from painted bounds, so it settles back to 8pt as an adjacent
sidebar opens rather than switching on a layout flag.

GPUI editor cards use `card_material`: one quad owns the rounded fill and its
one-physical-pixel contour, over a flat chrome backing. The bottom input tray
instead shares the native terminal's exact corner coverage field. Paint its
body as disjoint rectangles, cache tiny tinted corner images, then paint the
same complementary SVG cutouts and `card_edge` as the terminal. Do not add a
second independently antialiased rounded fill beneath that coverage. The tray
uses terminal background and opacity; chrome paints only the exterior. Keep
material alpha constant while it moves through the reveal mask. Exterior chrome
subtracts the intersection of the actual card and reveal bounds collected during
prepaint. Never infer this hole from a full-height card extending below the
viewport: that leaves the reveal's bottom inset unpainted during motion.

Horizontal tabs already include bottom clearance inside their fixed-height row,
including the selected capsule's two-point inner inset. Subtract that clearance from the canvas's top margin as the strip appears, so
the visible selected-tab-to-pane gap matches the 5pt lateral outer inset. The macOS
caption row is 32pt, matching AppKit's titlebar; the traffic lights and the
22pt controls are both centred in it, leaving 5pt around the controls. Without
a tab strip, the card starts at the row's bottom edge, so the space below the
controls equals the space above them. A docked caption gives its row back and
the card keeps its full 5pt top clearance. The strip's appearance does not
change caption height or tab/control centering. The native clip, gutter paint and
agent panel top use the same top geometry. When that places the pane above the
caption layout boundary, expand the canvas viewport clip only by that amount
and retain its content origin with matching padding. The agent column stretches
from the same top with a negative margin, preserving its bottom baseline. Do not
move the card above an unchanged overflow clip. The caption backing must
exclude the reclaimed strip over the canvas and agent columns; otherwise their
translucent chrome composites twice and draws a dark horizontal seam. Sample
that strip from actual viewport prepaint bounds, including layout snapping. Keep the
caption backing over the left sidebar, which does not reclaim that strip.
The original divider hit
targets remain available between them. Following maintainer feedback on 2026-10-17,
this prototype uses the same restrained, one-physical-pixel outline on panes,
the input tray and agent composer, with 8pt corners and no contact shade.
Hover details alone add a short contact shade directed toward the lower right.
Three exterior contour bands fade over three physical
pixels; they share the surface's inner rounded boundary. This avoids hard
rectangular cutoffs through the corner penumbra. The outer canvas has no additional
outline.
Transparent quad paint is clipped to narrow perimeter regions before
rasterization; contact shading uses small corner masks and narrow side masks.
It must not tint the terminal interior. Edge geometry snaps to the native
terminal's physical pixel grid. This is a scoped exception to the borderless
design, not permission to frame every control.
Pane exterior corners use one normalized circular alpha mask through GPUI's
supersampled SVG atlas, reflected into four small tiles. Equal device sizes
share the same entry across panes, positions and theme colors; tint is applied
when painting. Keep the curve aligned with the native clip and fine contour,
not inset as a second outline. Skip tiles that snap to zero size during reveal.
The mask is geometric coverage, not a full-pane image or a new icon asset.
Pane titles align to the leading edge and surface tabs use the UI font, without
a nested track or redundant surface count. Their close and rename actions stay
in the same pane-local scope.
Resize feedback uses a centered 2pt by 28pt rounded grip within the 5pt gutter,
not a full-length active line. It appears on hover and stays visible while
dragging without changing geometry. Its centered 7pt hit target is independent
of the visible gutter width and remains unchanged; small
panes shorten the grip rather than overflowing their shared edge.

The agent panel is base chrome, like the left sidebar: no enclosing frame,
rounded card fill or shadow. Its composer bottom aligns with the terminal-card
bottom, including when the bottom tray is reserved. The composer adds no second
bottom padding inside this baseline; only its fill and fine edge lift it. Do not nest the conversation inside a
second floating card.
Measure the composer's leading clearance from the adjacent terminal card's
visible edge, not the unframed sidebar's logical boundary. That terminal already
contributes the 5pt gap. The sidebar body therefore spans its full column; the
composer starts at the column boundary and keeps 5pt on the trailing side. Its
bottom remains on the terminal baseline. Header, transcript and history retain
their own 2pt horizontal inset so this correction does not move their content.
Sidebar resize uses the same short 2pt × 28pt hover/drag grip as pane splits.
Its transparent 7pt target belongs to the shared layout overlay, centred between
visible surface edges; never clip it inside the agent panel or paint a full-height
hover stripe. Expanded Sessions and Files/Search share the saved width. The
compact icon rail remains fixed. Pressed resize must not resurrect the old
full-height opaque seam covers; the short grip is the complete feedback.
Clamp rendered widths to the current window
budget and begin drags at the visible width, preserving the saved preference.
Compact-tab details use a stable tile anchor and one opaque, rounded material.
The title and identity icon share one row, with the pane count at its trailing
edge; the path follows below. Keep 12pt horizontal and 10pt vertical padding.
Use an opaque popover material with a quiet edge, and separate other pane titles
with a faint rule rather than adding another button-like card inside the popover.
SSH and unread state use a separate footer only when present. Long paths wrap
and the card scrolls rather than silently dropping pane details.

The input tray spans the terminal column and aligns with the pane cards, with
5pt total horizontal inset and 8pt corners. Pane-to-pane, pane-to-tray and
tray-to-window-bottom gaps are all 5pt at the decorated endpoint, matching
the visible outer clearance and agent-composer margins. The existing
1pt split divider plus two 2pt pane insets define that rhythm. The tray sits
2pt above the inner canvas bottom, plus the 3pt canvas inset. A 44pt tray
reserves 49pt at rest and scales that reservation with the reveal progress.
Its content height stays unchanged. The tray uses the focused terminal's base
color and opacity, without a white lift or a rectangular chrome backing beneath
its translucent interior. The exterior gap belongs to the workspace. Its fill
and fine contour use the terminal's shared coverage and perimeter treatment.
Sidebar boundaries use the canvas gutter, not a decorative vertical line.
Compact navigation tools and session identities share the rail's centerline;
their different hit-target sizes must not introduce a horizontal offset.

Show/hide shares one spring sample across the terminal reservation, agent-panel
bottom clearance, popover anchor and input reveal. Clip the full-size input to
the revealed height, anchoring the full-size card at the reveal's top so it
slides below the viewport. Never scale its text or multiply the material alpha
by animation progress: that turns an intentional glass surface into a hole.
The terminal must expand throughout closing rather than wait for the end. Reversing the animation preserves the current geometry and
velocity, and reduced motion reaches both layout and material endpoints in the
same frame. Native presentation still uses the existing frame-scoped sync.
Input caching is used only at the settled endpoint. The tinted corner cache
is bounded and keyed by physical size, orientation, chrome alpha and material;
moving a card reuses its existing image rather than uploading another texture.

One tab with one pane and both sidebars hidden returns to the original clean terminal:
no canvas inset, no corner mask and no input-island margin. These dimensions
follow sidebar motion continuously rather than snapping when a boolean changes.
Split panes keep the decorated canvas even without sidebars or additional tabs,
so their outer clearance cannot collapse below the gap between their cards.

Expanded vertical navigation owns the macOS caption controls. The toolbar
occupies its top row while the global caption row gives its height back to
terminal content. Its material is supplied
by the sidebar, not by a second translucent header rectangle.
Files/Search can also own the caption when their combined rail and tool-column
width fits the window controls. A compact rail on its own, hidden navigation
and horizontal tabs retain the global row. Native traffic lights must never
overlap a 44pt rail or a terminal pane.
The left-sidebar visibility control remains available when docked. The caption
must fit the traffic lights and all three visibility controls throughout width
changes. Switching to horizontal tabs restores the global row immediately;
outgoing docking motion must not briefly expose a terminal behind that row.
Docking is based on the effective navigation layout and its actual available
width, not the saved pinned preference. A temporary rail-only override is still
a rail. Caption progress is derived from the existing sidebar width/visibility
spring samples, with no independent caption spring. Restore the full caption
row by the point where the sidebar can no longer contain it; never interpolate the toolbar across native
terminal content. Compact controls use 24pt targets and a tighter shared gap
budget, without changing session tile or row hit targets.

Pane promotion follows the current tab orientation on macOS. Vertical tabs
accept panes in the sidebar; dragging does not temporarily create horizontal
tabs or undock the caption. Horizontal mode retains its existing strip drop
destination. Moving a pane or promoting its existing terminal into a tab marks
native layout pending before the next frame; a reused terminal is not a newly
created surface. The floating drag label uses the same 8pt rounding and UI
typography as the canvas, with a quiet terminal icon and an opaque fill so it
stays legible above native content.
Pane headers, surface tabs and tab navigation share a retained application
identity. The existing process observer supplies retained process observations; rendering
does not enumerate processes or scan terminal output. Editors outrank their
plugin runtimes; conflicting applications at the same rank keep the generic
identity. Agent identities keep their existing precedence and SSH tabs retain
their remote identity. Unknown applications fall back to the terminal asset.
Common editors and runtime ecosystems use the already bundled Nerd Font, just
like language identities in the file tree; actions continue to use Phosphor.
Application identities use restrained application-tinted colors across themes;
unknown shells retain the shared terminal identity rather than a random logo.
Font identities are optically normalized to 80% of their slot from the bundled
glyph's ink bounds, then centered by ink rather than its monospace advance cell.
The face is parsed once and GPUI retains shaped lines; do not add per-frame font
file reads or application/process queries to draw icons.
These are presentation facts, never authority for agent execution or control.

The embedded macOS renderer defaults to Ghostty's `window-padding-color = extend`
so alternate-screen applications carry their edge cells into fractional grid
padding. This is loaded before the user's native configuration, not as immutable
host policy. An explicit `background` or `extend-always` choice stays effective.
Ghostty's own prompt/powerline safeguards remain in charge; Con does not sample
terminal pixels or repaint program backgrounds in GPUI.
Drop regions follow the same 2pt inset and 8pt corners as pane cards. Their
bounds deduct every 1pt split divider. Resizing retains the full gutter hit
target and shows the short centered grip described above, not a full-height bar.

Every macOS terminal leaf, including an undecorated single pane, updates its
native pane clip during prepaint. Do not bypass this with a single-pane early
return: closing a sibling or promoting a pane reuses a surface that may still
have its old split-sized mask.

## Ownership and presentation

- GPUI determines the canvas rectangle before terminal children prepaint.
- GPUI paints four narrow gutter rectangles and four small corner cutouts,
  cached by bounds, depth and backing scale. Do not replace this with
  a full-area fill or a window-sized path: GPUI's path compositing cost follows
  the path's bounds even if it has a hole. The caption row owns its own material,
  so docking controls does not double-composite the sidebar background.
- AppKit clips terminal host layers against the shared rectangular viewport
  and each leaf's rounded card bounds. Terminal title/surface header material
  is a noninteractive AppKit sibling below GPUI, with the same upper corner
  radius. GPUI retains header text, controls and the common card surround;
  it must not add another rounded header fill to its transparent overlay.
  A translucent exterior cutout cannot erase rectangular terminal or
  header pixels underneath it: exclude those pixels before compositing the
  workspace material. A view-backed NSScrollView must first
  receive its own backing layer when a clip is required. A Metal-backed
  terminal descendant does not
  guarantee that the scroll host has a layer. Leave the window parent alone.
- Corner cutouts use one circular curve with tangent-continuous endpoints.
  Do not expand it to compensate for masks: displaced coverage boundaries make
  contour thickness uneven. Keep the native mask, header and contour on the
  same snapped card bounds and radius. Native header material remains opaque
  inside the card, with square lower corners. One material spans the title and
  any surface strip: derive its height from the actual terminal viewport's
  top relative to the pane, not duplicate fixed header heights. Keep the
  terminal host's 2pt seam overdraw out of that measurement.
  Keep the rest of the terminal untouched. Leave AppKit's
  container's backing-store composition alone: Con owns host masks, not the
  parent's backing layer.
- The native safe-area view is disabled for the input tray. Its former fill
  made the input look like a second strip, and appeared as a separate stage at
  the reveal endpoint. GPUI paints only the reserved workspace gap and tray,
  never a full-area fill over terminal content. Keep the native safe-area
  primitive's linked tests until the prototype settles; it is not used to fake
  terminal translucency behind the tray.
- The clip path is cached per host. A window-scoped geometry generation
  invalidates otherwise unchanged terminal layouts when the outer canvas moves.
- Resolve each card's four corner radii from its snapped bounds and use those
  same values for GPUI contours/cutouts and the native pane mask. Forward its
  upper pair to native header material; a uniform native radius beneath a
  larger translucent exterior cutout can expose native pixels outside the curve.
  Radius-only changes invalidate cached paths. Clamp radii to half the smaller
  card dimension, including during small-pane layouts. The native header's
  material path is independent of the shared workspace clip.
- A header below another header has square top corners, avoiding holes at the
  title/surface-strip join. An undecorated single terminal keeps a zero radius.
- Header material follows its host's visibility, including tab/surface changes
  and pane zoom. No-header layouts hide the cached material, even if a later
  visibility update reveals the terminal. Detachment removes the sibling.
  Cache unchanged geometry, color, scale and visibility before allocating color
  objects or changing layers; do not rebuild native decoration on terminal output.
- Existing frame-scoped native presentation synchronization remains in place.
  A shape mask must not introduce an opaque terminal backing, a second glass
  fill or a delayed geometry update.
- Each macOS terminal host intersects its pane-local rounded rectangle with the
  workspace mask. Cache both paths and remove unnecessary masks once the host
  is fully contained in rectangular pane and workspace bounds. A zero-radius
  smaller pane still needs clipping. Never attach pane-local clipping to the shared
  window transition underlay. Keep this path macOS-only until other backends
  have equivalent native acceptance; GPUI rounding alone does not clip AppKit.
- GPUI owns host and surface view frames; disable their AppKit autoresizing
  masks. A cached layout is not authority for the actual native frame or the
  current backing layer's mask attachment. Check those before skipping sync,
  and reattach cached paths if AppKit replaces a backing layer at equal bounds.
  `RUST_LOG=con::canvas=trace` records prepaint/paint bounds and native host,
  clip-view and surface-layer geometry for diagnosis; it does not log terminal
  text. Keep this opt-in trace off during normal performance measurements.
- Caption motion has one owner: the sidebar's existing geometry spring. Reserved global-row height is the complement
  of docking progress, but docking can start only when the actual sidebar can
  contain all controls. The toolbar stays inside that sidebar, not somewhere
  between it and the window edge. Reduce Motion resolves
  directly to a valid endpoint.
  Dragging a pane into a new tab restores the global drop row immediately,
  without making an interaction wait for decorative motion.
- Pane resizing uses measured canvas bounds, including its inset, rather than
  estimating terminal geometry from window dimensions.

## Scope and acceptance

Native canvas geometry, clipping, padding defaults and fine outlines are
macOS-only. Application identity classification and its bundled colored glyphs
are shared across platforms. Windows and Linux retain their existing caption,
terminal reservation and input-bar layout. A portable canvas implementation needs its
own renderer clipping and caption acceptance; macOS layer masks are not such
an implementation.

Before committing, inspect light and dark themes, horizontal tabs, compact and
expanded vertical navigation, Files/Search, split panes and the input bar's
three modes. Exercise collapse/reveal reversal, pane divider dragging, native
window dragging, focus recovery, scrolling and narrow windows. Terminal output
and selections must stay inside the canvas, and the input bar must not cover
the final terminal row.

This visual change is not a resolution of the intermittent native presentation
seam. Retain the display/power-state acceptance matrix in
[Sidebar Motion](sidebar-motion.md), including the internal-display/battery
reproduction. External-display checks alone cannot close that regression.
