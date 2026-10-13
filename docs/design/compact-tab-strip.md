# Compact Horizontal Tabs

Horizontal tabs are window navigation, not terminal content. They use the
configured UI font, while terminal text and code keep their monospace fonts.

## Layout

- Keep the existing titlebar heights: 28 points on macOS and 36 elsewhere.
- Keep tab hit targets at their existing height. Use a twenty-point close control.
- Center tab hit targets vertically, with matching space above and below.
  One restrained outer capsule groups the targets. Resting tabs do not add
  another fill; selection and hover use inner capsules with equal two-point
  clearance. Targets are 24 points high on macOS, 30 elsewhere.
  Center the row in the titlebar, leaving two points above/below on macOS
  and three elsewhere.
- Divide the available tab region equally, independent of title length or
  selection. AI summaries and inline renaming must not move neighbouring tabs.
  Names truncate after the fixed identity slot and retain full-name tooltips.
  Keep a 140-point minimum and a 200-point maximum target width; all visible
  targets still share the same allocation. Cap the viewport by the number of
  rendered targets, including a pane-promotion placeholder only while it is
  actually present. Surplus width remains unpainted, draggable titlebar space.
  Leave at least 64 points between the strip and the action controls.
  The region scrolls horizontally when
  it cannot fit all tabs. Activating a tab reveals it, but ordinary repaints
  must not override manual scrolling. Window controls stay outside this region;
  blank titlebar space outside the tab strip remains draggable.
  Reveal requests use the laid-out viewport width, tab identity, position and
  count. Ignore queued requests superseded by another selection, manual scroll
  or drag. Explicitly selecting the already-active tab reveals it again.
  Keep the twelve-point trailing reorder hit region outside the painted tray.
  It must not add blank width to the last tab's visible end.
- Fill each equal-width hit target with one capsule, inset by two points.
  Keep its width independent of the name. Within it, the identity slot stays at
  a fixed eight-point left inset. The title begins six points after the identity
  slot. The right side reserves the close hit target, its concentric inset and
  six points of label clearance, even while the close control is hidden.
  This keeps icon and title together rather than distributing them across a
  wide tab. Icons never move when names change; long names truncate without
  overlapping either control. Two-point
  target padding separates adjacent groups without separator rules.
  The selected surface wraps the icon and name as one unit. Use the same
  `TAB_SELECTION_INSET` for target horizontal padding and selection-layer
  vertical padding. Narrow targets retain two-point clearance on all four sides.
  Wider targets retain the same clearance and fixed leading content positions.
  Its height is 20 points on macOS, 26 elsewhere. Both surfaces use half-height
  corner radii. Attention marks keep their semantic color.
  The identity row fills the capsule width, including after truncation.
- Keep the twenty-point close control at the capsule's right end, separate from
  the fixed identity/title group. Reserve the close control's space, so revealing
  the close control never shifts or replaces the identity. The capsule never
  moves. Attention marks remain visible while hovered.
  Use a twenty-point component button with a fourteen-point circular fill,
  centered vertically in the identity slot. The visual circle is smaller than
  the hit target and the name capsule.
  Derive the close slot's right inset from half the difference between capsule
  and slot heights. Its visible circle has equal right/top/bottom clearance.
  Check against the painted capsule, not the outer hit target.
  Attention dots sit at the identity slot's upper-right corner without
  shifting the identity slot or label.
  The Phosphor thin close glyph uses a ten-point square at default UI size and
  scales in even-point steps. It stays subordinate to the identity
  icon but remains readable. A quiet circular fill is visible whenever the
  close control appears, then strengthens when the control itself is hovered.
  Keep the button explicitly square and check the inner circle and SVG centres,
  not only the button's outer bounds, including with a larger UI font size.
  The overlay slot is also a centred flex container: an inline button in a
  block container follows the inherited text baseline rather than its centre.
  Check the circle against the actual tab, not just against the button.
  Its icon becomes clearer on hover, without a destructive red treatment.
  The close control appears only while its tab is hovered, including the
  selected tab. Empty space still activates and drags the tab.
- New-tab drag placeholders share the equal-width allocation. Reordering uses measured
  target bounds rather than assuming equal widths. Reorder, rename,
  middle-click close, context menus, and native caption hit testing retain
  their existing handlers.

## States

The outer capsule tints the actual translucent chrome and groups the entire
strip without rules or borders. Ordinary resting tabs remain transparent inside
it, avoiding three stacked fills. The selected inner capsule is lighter in both themes:
a slight lightness lift in light themes, a foreground mix in dark themes.
Its title uses normal weight. Hover strengthens that tab's fill, never adding
a separate name badge or rectangular outer frame.
Labels use solid theme-derived ink at normal weight. Selection must not depend
on making unselected titles hard to read.
Identity glyphs use a sixteen-point frame at default UI size. Generic terminal,
code, file, remote, monitor, and reader identities use generated freestanding PNG objects, explicitly
requested for tab navigation. Specific application identity assets remain
unchanged; action controls and semantic status glyphs retain Phosphor SVGs.
The PNGs are embedded in the binary and share the existing premultiplied-alpha,
physical-pixel-sized preview cache. Do not decode or resize them during paint,
read them from the filesystem at runtime, or apply translucent glyph opacity.
Labels use the UI font at
11.5 points scaled from the configured UI size, preserving original case.
The six-point gap groups the icon and label
without adding padding inside the label itself. Larger UI sizes cap the frame at the
twenty-point identity slot; hover never shifts the slot or label.

User-assigned accent fills and configured inactive accent opacity remain
supported. Activity and attention indicators retain their semantic colors.
Labels do not inherit glyph opacity from these fills. SVG icons use solid,
theme-derived ink. Bitmap identities keep their authored colors: teal for terminal,
connection and monitor shapes, amber for code and documents, and ivory for books.
Distinct silhouettes replace shared square backplates. Shallow chamfers provide
definition without external shadows or glow; negative spaces remain transparent.
Judge details at the actual sixteen-point display size, not the source resolution.
The current local design trial uses a front-facing teal terminal frame with a
larger ivory prompt and restrained depth, without a monitor stand;
the remaining identities keep their freestanding forms pending visual acceptance.

Names come from the same retained presentation as the sidebar, including user
renames. Do not shorten by character count: the label truncates according to
its actual available width. The tab tooltip includes the full name and existing
subtitle/status when distinct. No path lookup, terminal query, or generated
summary is performed while rendering. The close button has its own label and
tooltip, and consumes mouse-down without moving terminal focus or dragging the
window.
Do not occlude the parent tab's hover hitbox: a hover-only close control would
hide itself as the pointer enters it. Consume mouse-down to keep activation
separate, and test hover followed by a click, not just the button in isolation.

The shared tray fills the tab region, not the window's action buttons.
No nested name capsules, divider rules, borders, or shadows are added.

## Dragging

The floating horizontal-tab preview uses the measured source surface width,
the same capsule height/radius, identity slot, UI font, and solid ink as resting
tabs. Pointer offset is relative to the painted surface, not its padded target.
The preview does not retain the older fixed-width rectangular card.

One workspace-root drag-move handler owns horizontal reordering. It probes the
floating capsule's centre and compares it with the actual prepainted neighbour
bounds, excluding the dragged tab's own placeholder. Convert the resulting
insertion position back to the original tab order before committing a drop.
The source's visual index belongs to the layout snapshot, not a newly requested
drop slot that may not have painted yet. Keep the last valid target through
missing or partial snapshots; do not reset or reorder based on incomplete data.

Pane-to-tab promotion retains its separate ghost/drop flow. Resting mouse moves
and pane-resize handling do not run horizontal reorder calculations.

## Scope

Titlebar actions retain their twenty-two-point hit targets and use fourteen-point
Phosphor icons at default UI size, scaling in even-point steps. Fractional icon
dimensions can snap their origin and size differently and move their centre.
Open-panel states have a persistent neutral fill and solid
foreground icons instead of accent-colored glyphs. Hover and press strengthen
the same fill, with six-point rounding and centered placement. Accessibility
toggle metadata and shortcut tooltips remain unchanged.

This change affects tab paint, layout and overflow navigation. It adds no timers, theme parsing,
terminal queries, native-surface changes, or new animation. Window glass,
presentation synchronization, single-tab chrome, and vertical tabs are not
changed.

## Acceptance

Inspect dark and light themes, assigned tab colors, hover and selected states,
long titles, increased UI font size, narrow windows, and drag placeholders.
Exercise click and keyboard activation, close, rename, and reorder. Native
macOS verification does not substitute for Windows/Linux visual acceptance.
