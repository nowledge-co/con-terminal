# Pane Title Bar — Design Spec

**Date**: 2026-05-06
**Branch**: `wey-gu/pane-title-bar`

---

## Overview

Add a persistent title bar to every pane when there are 2+ panes in the split tree. The bar shows the pane title, direct fullscreen/restore and close controls, and supports drag-to-tab promotion.

> **Update (2026-10-09)**: terminal panes keep this design. **Editor panes no
> longer render this title bar** — their file tab bar and pane controls are
> merged into a single 28px chrome row (`render_editor_merged_bar` in
> `crates/con-app/src/pane_tree.rs`), rendered by PaneTree with data from
> `EditorView`'s public API. File name appears once (the active tab capsule);
> ⛶/✕ pane controls and drag-to-tab promotion live in the merged row and only
> appear with 2+ panes. An editor pane with no files open renders no chrome row.
> File tabs retain their readable widths in a horizontal scroll area; pane
> controls remain outside that area. Labels and icons use solid semantic theme
> ink, with focus conveyed by the active fill rather than glyph opacity.
> The merged bar and label metrics follow the UI mono-font size.

---

## Scope

- **Only shown when `pane_count > 1`** — single-pane layout has no title bar.
- The existing surface strip (shown when `surfaces.len() > 1`) is preserved and sits below the title bar.
- No title editing in this iteration.

---

## Visual Design

```
┌─────────────────────────────────────────────┐
│      sundy@m1max: ~/work/con      [⛶][✕] │  ← title bar, h=28px
├─────────────────────────────────────────────┤
│  [surface strip — only when surfaces > 1]   │  ← existing, h=28px
├─────────────────────────────────────────────┤
│                                             │
│            terminal content                 │
│                                             │
└─────────────────────────────────────────────┘
```

### Metrics

| Property | Value |
|---|---|
| Height | `px(28)` |
| Background (focused) | `theme.tab_bar_segmented` |
| Background (unfocused) | `theme.tab_bar_segmented.opacity(0.78)` |
| Title font | `theme.font_family`, `px(12)`, `FontWeight::MEDIUM` |
| Title color (focused) | `theme.foreground.opacity(0.72)` |
| Title color (unfocused) | `theme.foreground.opacity(0.52)` |
| Button size | `px(20)` × `px(20)` |
| Button icon size | `px(11)` |
| Button hover bg | `theme.foreground.opacity(0.08)` |
| Button rounding | `px(4)` |
| Button icon color | `theme.foreground.opacity(0.52)` |

### Icons (Phosphor)

- Fullscreen: `phosphor/corners-out.svg`
- Restore: `phosphor/frame-corners.svg`
- Close: `phosphor/x.svg`

### Layout

```
[px(6) pad] [flex-1 centered title] [fullscreen/restore 20px] [✕ 20px] [px(6) pad]
```

- Title is centered in the remaining space using `flex-1` + `text_align(center)` (or `justify_center` on a flex row).
- Close button is hidden (`display: none` equivalent — just omit from render) when `pane_count == 1`.

---

## Controls

Pane title controls are direct buttons rather than an overflow menu. This keeps the title bar usable during drag workflows and avoids a second menu layer for two high-frequency actions.

| Item | Condition | Action |
|---|---|---|
| **Fullscreen / Restore** | always shown; icon changes when already zoomed | `toggle_zoom_cb(pane_id)` |
| **Close Pane** | only when `pane_count > 1` | `close_pane_cb(pane_id)` |

"Fullscreen / Restore" maps to the existing `toggle_zoom_focused` / `TogglePaneZoom` path.
"Close Pane" maps to the existing `close_pane_in_tab` path.

---

## Close Button

- Calls `close_pane_cb(pane_id, window, cx)`.
- Hidden when `pane_count == 1` (no split to close into).
- Uses `window.prevent_default()` + `cx.stop_propagation()` on mouse-down, same pattern as the surface tab close button.

---

## Drag-to-Tab

### Interaction

1. User drags from the pane title bar.
2. Over pane content, Con previews the nearest split edge and moves the pane there on drop.
3. Over the horizontal tab strip, Con renders a ghost tab at the live insertion slot.
4. On drop in the tab strip, the pane is detached and promoted to a new tab at that slot.
5. On drop outside an actionable target, the drag state is cleared and the pane stays put.

### State

Pane title drags use `PaneTitleDragState` in `crates/con-app/src/workspace/types.rs` as the authoritative workspace-level state:

```rust
struct PaneTitleDragState {
    title: SharedString,
    current_pos: Point<Pixels>,
    active: bool,
    target: Option<PaneDropTarget>,
}
```

`title` drives the workspace-owned floating title overlay, `current_pos` keeps that overlay centered under the live cursor, and `target` records whether the pane is currently targeting a split drop or a new tab slot.

### Drop Indicator

Rendered in the horizontal tab strip as a primary-tinted ghost tab. Existing tabs shift around it so the final tab order is visible before the user releases.

### Promotion Logic

`ConWorkspace::detach_pane_to_new_tab_at_slot(pane_id, slot, window, cx)`:

1. Collect all surface terminals for `pane_id` from the active tab's `pane_tree`.
2. Remove the pane from the split tree without shutting down its terminals.
3. Create a new tab using the active surface terminal from step 1 as the root pane.
4. If the pane had multiple surfaces, restore them into the new tab's pane tree via `create_surface_in_pane`.
5. Insert the new tab at the requested slot and switch to it through the normal tab activation path.

---

## Callback additions to `PaneTree::render()`

Two new callbacks added alongside the existing ones:

```rust
pub fn render(
    &self,
    session_id: u64,
    begin_drag_cb: impl Fn(SplitId, f32) + 'static,
    focus_surface_cb: impl Fn(SurfaceId, &mut Window, &mut App) + 'static,
    rename_surface_cb: impl Fn(SurfaceId, &mut Window, &mut App) + 'static,
    close_surface_cb: impl Fn(SurfaceId, &mut Window, &mut App) + 'static,
    close_pane_cb: impl Fn(PaneId, &mut Window, &mut App) + 'static,
    toggle_zoom_cb: impl Fn(PaneId, &mut Window, &mut App) + 'static,
    rename_editor: Option<SurfaceRenameEditor>,
    divider_color: Hsla,
    hide_pane_title_bar: bool,
    cx: &App,
) -> AnyElement
```

`PaneTree` starts pane-title drags by creating a `DraggedTab` with `origin = DraggedTabOrigin::Pane`; workspace-level drag handlers own live target tracking and drop behavior.

---

## Files Changed

| File | Change |
|---|---|
| `crates/con-app/src/pane_tree.rs` | Add `render_pane_title_bar()`, update `render_leaf()`, update `render()` / `render_node()` / `render_zoomed_leaf()` signatures, start pane title drags with `DraggedTabOrigin::Pane` |
| `crates/con-app/src/workspace/` | Wire `pane_tree.render(...)`, add pane split drag state, render workspace-owned floating pane title preview, insert pane-to-tab ghost tabs in the horizontal tab strip, promote dropped panes to new tabs |

---

## Out of Scope

- Title editing from the title bar (double-click rename) — use existing surface rename flow.
- Drag between windows.
