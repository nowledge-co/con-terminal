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

## Addition and Removal Motion: Next Phase

Individual tabs currently enter and leave immediately. The strip's overall
appearance/disappearance already uses `MotionValue` through
`sync_tab_strip_motion`; that is separate from per-tab motion.

The maintainer's reference recording shows tab creation at 00:02–00:04 and
pane expansion at 00:25–00:27. Borrow their spatial continuity, not an isolated
opacity effect. Pane identities and a compact tab-level identity stack merit
separate exploration: active identity first, bounded count, no repeated icons
for ordinary one-pane tabs, and no ambiguity with surface tabs.

The next implementation should commit logical creation/closure immediately,
then animate only bounded, metadata-only chrome. Do not keep a closing PTY
alive for an exit animation. Use stable session IDs for transition state,
retarget from the current visual position during rapid actions, keep selection
and hit targets in agreement, and preserve the last-tab/window-close behavior.
Respect Reduce Motion and request frames only during unsettled transitions.

Before shipping, test overflow, first/last/middle closure, rapid reversals,
dragging during transitions, and the one-tab/two-tab height boundary. Do not
animate individual width allocation until reveal and drag geometry consume
the same interpolated bounds.

The identity assets and their generation prompts are documented in
[`assets/tab-icons.md`](../../assets/tab-icons.md). Generic identities do not
replace application brand icons or semantic activity indicators.
