# macOS Focus & First Responder — Key Lessons

## Background

Con embeds Ghostty terminal surfaces as native `NSScrollView` subtrees inside the
GPUI Metal window. GPUI renders its own UI (workspace chrome, editor panes, agent
panel) via a `GPUIView` (Metal-backed `NSView`) that is the window's initial first
responder. Ghostty surfaces are added as siblings below `GPUIView` in the view
hierarchy.

This creates two separate focus systems that must be kept in sync:

| Layer | Mechanism | Controls |
|---|---|---|
| macOS | `NSWindow.firstResponder` | Which NSView receives native key events |
| GPUI | `FocusHandle` / `window.focused()` | Which GPUI element receives `on_action` dispatch |

Both must be correct for keyboard shortcuts (Cmd+W, Cmd+T, etc.) to work.

---

## Problem 1 — GPUI has no focused element in editor-only tabs

### Symptom
Cmd+W / Cmd+T do nothing when an editor pane is the only pane in a tab, or after
clicking an editor pane.

### Root cause
GPUI's `on_action` dispatch walks the focus chain starting from `window.focused()`.
If `window.focused()` returns `None`, no `on_action` handler fires — silently.

Terminal panes have a real GPUI `FocusHandle` (owned by the `GhosttyView` entity).
Clicking a terminal pane calls `terminal.focus(window, cx)` which sets
`window.focused()` to that handle, enabling action dispatch.

Editor panes had no GPUI entity and no `FocusHandle`, so clicking them left
`window.focused()` as `None`. All keyboard actions were silently dropped.

### Fix (commit `c1db4a2`)
Add `workspace_focus: FocusHandle` to `ConWorkspace`. When an editor pane is
clicked, call `workspace_focus.focus(window, cx)`. Add `track_focus(&workspace_focus)`
to the root workspace div so GPUI tracks it in the element tree.

```rust
// In ConWorkspace::new / from_session:
workspace_focus: cx.focus_handle(),

// In focus_pane_in_active_tab (when pane is an editor):
self.workspace_focus.clone().focus(window, cx);

// In render.rs root div:
.track_focus(&self.workspace_focus)
```

---

## Problem 2 — macOS first responder not restored after Ghostty NSView removal

### Symptom
After closing the last terminal pane (leaving only editor panes), keyboard shortcuts
stop working entirely — even after clicking the editor pane to set GPUI focus.
The issue persists until the user clicks somewhere that triggers a native macOS
focus event.

### Root cause
When `detach_host_view` calls `host_view.removeFromSuperview()`, macOS removes the
`NSScrollView` (which contained the Ghostty surface) from the window. macOS does
**not** automatically transfer `firstResponder` to another view. The window ends up
with no first responder (`window.firstResponder == window` itself, which doesn't
forward key events to GPUI).

GPUI's key event pipeline depends on the `GPUIView` being the macOS first responder.
If it isn't, native `keyDown:` events never reach GPUI's event handler, so even a
correctly-set GPUI `FocusHandle` is irrelevant — the events don't arrive.

### Fix (commit `489b883`)
After `removeFromSuperview`, explicitly restore first responder to the cached
`GPUIView` pointer:

```rust
// In GhosttyView::detach_host_view (macos):
let _: () = msg_send![host_view, removeFromSuperview];
if let Some(gpui_view) = self.gpui_nsview {
    let window: id = msg_send![superview, window];
    if !window.is_null() {
        let _: () = msg_send![window, makeFirstResponder: gpui_view];
    }
}
```

`gpui_nsview` is cached in `ensure_initialized` from the `AppKitWindowHandle`:

```rust
let gpui_nsview = match raw_handle.as_raw() {
    raw_window_handle::RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr() as id,
    _ => return,
};
self.gpui_nsview = Some(gpui_nsview);
```

---

## Problem 3 — workspace_focus must be re-asserted after terminal close

### Symptom
Closing the last terminal pane leaves the editor pane unable to receive Cmd+W even
though `workspace_focus` was focused at click time.

### Root cause
The terminal close path calls `sync_active_terminal_focus_states` which calls
`terminal.set_focus_state(false)` on all terminals. On macOS, Ghostty's focus
handling can interact with the responder chain. Additionally, the deferred
`shutdown_surface` (via `cx.on_next_frame`) runs after the synchronous focus
assignment, potentially disturbing the responder state.

### Fix (commit `941ae3a`)
After the deferred `shutdown_surface` completes, re-assert `workspace_focus` in the
`else` branch of `remove_pane_in_tab` when no terminal survives:

```rust
} else if tab_idx == self.active_tab {
    // No terminal survived — keep keyboard focus on workspace.
    self.workspace_focus.clone().focus(window, cx);
    self.sync_active_terminal_focus_states(cx);
}
```

---

## Summary: checklist for focus correctness

The workspace root tracks `workspace_focus` for shortcut ancestry and explicit
editor-only fallback. GPUI's `track_focus` also enables default mouse-down focus.
Suppress that default on the root: empty titlebar and layout-handle clicks must
not move keyboard focus away from the terminal, editor, or input field. Child
input targets still focus normally before the root's bubbling handler runs.
Do not compensate by focusing a terminal on every resize; that would steal
focus from other inputs and dialogs.

When adding a new non-terminal pane type:

1. **GPUI focus**: ensure clicking the pane calls `workspace_focus.focus(window, cx)`
   (or gives the pane its own `FocusHandle` entity).
2. **macOS first responder**: ensure any native `NSView` removal calls
   `makeFirstResponder: gpui_nsview` afterward.
3. **Post-close re-assert**: after closing a pane that was the last native-view pane,
   re-assert `workspace_focus` so the GPUI action chain stays live.
