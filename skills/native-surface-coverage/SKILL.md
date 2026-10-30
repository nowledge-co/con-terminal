---
name: native-surface-coverage
description: Prevent and investigate backdrop leaks, seams, stale clipping and rounded-corner artifacts when changing con pane/sidebar geometry, animation, transparency or native terminal composition.
---

# Native surface coverage

Use this for terminal-adjacent layout and motion changes, including regressions
that appear for only a few frames. Preserve con's intended glass material;
a darker screenshot alone is not evidence of a coverage fix.

## Establish the failing boundary

Record the transition and direction, tab orientation, compact/pinned/hover state,
pane/surface layout, theme, background opacity and display scale. Distinguish
these cases before editing:

- Intended transparency: the terminal and surrounding chrome naturally reveal
  different parts of the same backdrop. Compare at the same position/backdrop.
- Missing coverage: a strip or corner briefly shows the backdrop through an area
  that its current material should cover.
- Mismatched alpha/rounding: geometry is present but overlapping or independently
  antialiased layers create a seam. Making everything opaque can conceal this.
- Stale presentation: masks, native frames and GPUI paint describe different
  layout samples, particularly on reversal or the last animation frame.

Remote desktop, different macOS versions and 1x/2x screens can change observation
quality. Record them as verification limits, not an assumed cause.

## Trace ownership and prepaint order

Read the relevant parts of:

- [Pane composition](../../crates/con-app/src/pane_tree.rs): native clip prepaint,
  pane headers and local surrounds, including the undecorated single-pane path.
- [Frame composition](../../crates/con-app/src/workspace/frame.rs): canvas and
  pane surrounds, shared corner coverage and physical-pixel snapping.
- [Native host](../../crates/con-app/src/ghostty_view.rs) and
  [AppKit masks](../../crates/con-app/src/objc/ghostty_surface_trampoline.m): host
  frames, canvas/pane mask intersection, coverage tiles, layer replacement.
- [Workspace layout](../../crates/con-app/src/workspace/render.rs) and
  [presentation contract](../../docs/design/native-terminal-presentation.md).

Write down who covers each affected pixel and when its geometry becomes final.
GPUI `.occlude()` controls hit testing; it does not paint an opaque cover.
GPUI `.overflow_hidden()` does not by itself clip an AppKit terminal sibling.

The canvas surround prepaints after terminal children. If it changes the shared
canvas clip, already-registered native masks must consume that clip **in the same
frame**, even if a terminal has already completed prepaint or is otherwise idle.
Keep native frame/mask changes within the existing frame-scoped presentation
sync contract. A request for another frame does not repair the current frame.

Preserve these ownership rules:

- Window canvas clips constrain every registered consumer; local pane clips
  must not escape into other panes, the shared underlay or GPUI itself.
- Reused hosts still need valid clipping after split closure, zoom, tab/surface
  switches and same-size AppKit layer replacement. Model state alone does not
  prove that the layer currently has the expected mask.
- Native and GPUI curves share physical-pixel endpoints and corner coverage.
  A corner tile snapping to zero area is omitted individually; it must not
  discard the other visible corners or hide real raster errors.
- Keep configured glass alpha throughout transitions. The window-wide opaque
  transition underlay is restricted to opaque terminals. Do not extend it to
  translucent terminals as a shortcut.
- Keep surrounds narrow and avoid overlapping material fills. Do not enlarge
  masks, thicken borders, add full-pane covers or lengthen release timers to
  disguise a lifecycle or geometry mismatch.

For caching or inherited opacity changes, also read
[GPUI cache-aware guidance](../gpui-cache-aware/SKILL.md).

## Prove the corrected invariant

Choose tests for the affected boundary, rather than adding a broad screenshot
checklist to every UI edit. Existing linked tests live in `workspace/frame.rs`,
`ghostty_view.rs` and `pane_tree.rs`.

For a clip-order fix, reproduce terminal prepaint against the old canvas, then
update the canvas without a second host callback. Inspect the actual attached
mask/path immediately. Exercise both directions and reversal; ensure unrelated
siblings remain untouched. The regression is
`late_canvas_prepaint_refreshes_registered_masks_in_the_same_frame`.

For corner changes, include fractional/end-of-animation radii, mixed visible and
empty tiles, material alpha and physical scale. For host reuse, exercise the real
render/prepaint path rather than only calling a native setter in isolation.

Run linked macOS tests for native changes (`cargo test -p con --bin con`, with the
project's pinned toolchain), plus Rust formatting. Type checking cannot establish
AppKit mask attachment or FFI linkage. Inspect the relevant CI jobs for portable
paths; a macOS result does not validate Windows/Linux rendering.

## Native confirmation and stopping point

Use an isolated app/home/session with a contrasting backdrop. Batch the relevant
states: dark/light, compact↔pinned, hover pinning, tools and whole-panel toggles,
rapid reversal, single/split panes and affected surface headers. Include opacity
and scale endpoints in logic/native tests; only claim hardware actually used.
Do not change the user's live session or OS appearance for a fixture.

Inspect moving edges as well as settled corners. Use transition captures when
available; a final screenshot or successful socket command cannot prove that
intermediate frames were covered. Check logs for shared corner-coverage errors.
After a bounded fix/confirmation pass, report the invariant tested, native
interactions performed and remaining capture/hardware limits. Do not claim a
frame-rate guarantee from manual switching or sampled screenshots.
