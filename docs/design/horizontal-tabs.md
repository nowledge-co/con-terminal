# Horizontal Tabs

Tabs divide available width evenly, shrinking from a 200-point maximum to a
120-point minimum before horizontal overflow. Titles must not determine width.
Keep the caption's reserved drag gap outside tab hit targets. Explicit selection
reveals the selected tab; ordinary repaints must preserve manual scrolling.
Tab targets retain caption hit-test occlusion for native window behavior;
forward their wheel events to the tracked viewport because occlusion prevents
the ancestor's default scroll listener from receiving them.

The compact All tabs menu beside the title-bar controls lists every tab and
marks the active one. It provides direct access without relying on discovering
a horizontal gesture, including when the first tab is outside the viewport.
Resolve a menu choice by stable session identity, not a captured array index.
The menu is scrollable when the list exceeds its available height.

## Overflow Edges

Overflow allocates two 24-point navigation gutters outside the rounded scroll
viewport. Only directions with hidden content show an 18-point Phosphor caret.
Both gutters remain allocated at endpoints, so the viewport never jumps when
an arrow disappears. During overflow, the resting fill belongs to the fixed
viewport and its rounded outer gutters, not the moving tray. This prevents
scrolling from cutting the tray's rounded background into a hard rectangular
leading edge. Empty endpoint gutters remain transparent; rounding moves to
the viewport at that endpoint. Otherwise a hidden arrow's slot looks like
excessive capsule padding before the first tab. Do not paint a gradient over clipped labels: translucent
letter fragments look erased rather than indicating depth. Clicking moves 70%
of the visible viewport, clamped to the endpoints.

Endpoint visibility is observed after layout. Notify chrome only when those
two booleans change, never schedule an unconditional next-frame callback or
poll scroll state while idle. Preserve manual scrolling and explicit reveals.

## Addition and Removal Motion

`TabSlotMotion` animates slot weights from zero to one on arrival (220 ms)
and from the current weight to zero on departure (160 ms). The existing
`MotionValue` deceleration samples elapsed time, not frame count. Weighted
minimum widths, flex growth and strip width caps use the same sampled units;
titles and icons are clipped as space changes, never scaled.

Ordinary idle renders reuse the retained live-ID list. Only topology changes
reconcile slots; departure anchors are resolved then, not searched on every
frame. Request animation frames only while a slot remains unsettled.

Logical creation and closure remain immediate. A departing slot retains only
a title, icon identifier, last painted colors and neighboring IDs, never a
terminal entity or PTY.
Discard it when its weight reaches zero. Retarget an interrupted arrival from
its current width. Use session IDs for element identity and callbacks.

Selection reveal runs once slots settle, so a zero-width arrival cannot poison
the retained reveal key. Prepaint records the actual interpolated bounds.
Starting a drag or rename snaps to final allocation and clears departing
chrome; sorting and hit testing must not mix animated and final positions.
Reduce Motion also snaps without requesting a further frame. Vertical tabs
and the one-tab/no-strip layout keep their existing behavior. Wholesale
workspace replacement snaps to its new scene rather than animating every
restored/imported tab from zero width.

The maintainer's reference recording shows tab creation at 00:02–00:04 and
pane expansion at 00:25–00:27. Borrow their spatial continuity, not an isolated
opacity effect. Pane identities and a compact tab-level identity stack merit
separate exploration: active identity first, bounded count, no repeated icons
for ordinary one-pane tabs, and no ambiguity with surface tabs.

Tests cover time-based arrival, interrupted closure, consecutive middle
closures, idle/reduced-motion frame demand, and real GPUI intermediate slot
geometry. The existing overflow, reveal and drag tests remain applicable.

The identity assets and their generation prompts are documented in
[`assets/tab-icons.md`](../../assets/tab-icons.md). Generic identities do not
replace application brand icons or semantic activity indicators.
