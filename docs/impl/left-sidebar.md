# Left Sidebar

Status: Implemented

The left sidebar is the workspace's navigation and project panel area. It is not
the workspace tab identity by itself; vertical tabs remain the primary tab
rail/panel inside the sidebar.

## Layout

```text
[optional tool/session rail] [active sidebar panel] [pane tree] [agent panel]
```

When the left sidebar is hidden, none of the three sidebar columns render. This
preserves the clean terminal view users expect from the sidebar toggle.

When the left sidebar is visible:

- vertical tabs keep a 44 px rail for Files/Search, session controls, and
  session icons when `appearance.tabs_orientation = "vertical"`,
- the adjacent panel shows either Files, Search, or the unfolded session list,
- Files/Search and sessions are one sidebar system: opening Files/Search keeps a
  thin session/tool rail and swaps the adjacent panel to Files or Search.

Each tab appears in one navigation list at a time. With the session list
expanded, the rail contains only the section controls and New Tab; it does not
repeat the tab icons. Collapsing the list or opening Files/Search restores the
session tiles so tabs remain reachable without changing sidebar sections.
Selected tabs use a surface fill and foreground ink, not an extra green stripe.
Custom tab colors are preserved as fills in both compact and expanded views.
Expanded rows own their drag-reorder bounds; an empty controls-only rail is not
a tab drop target. Activity and attention markers follow the visible list.

Sidebar text and icons use solid semantic foreground/muted-foreground colors.
Secondary information is distinguished by color, size and placement, not an
extra opacity multiplier over translucent chrome. Surface fills remain
translucent. This is a contrast rule, not a replacement for the pending GPUI
Metal compositing correction tracked in #439.

Compact tab hover cards use one borderless surface with equal 12 px padding;
there is no external edge strip. Names and pane/status metadata use the UI font,
while paths and terminal titles use the mono font. Text follows UI font sizing,
and cards clamp to the available window width and height. Expanded rows already
show tab identity, so they do not need a second floating identity card.

## Slots

`ActivitySlot::Files` renders `FileTreeView`.

`ActivitySlot::Search` renders `SidebarSearchView`.

## Toggle Rules

- `Cmd+B` dispatches `ToggleLeftPanel`.
- The top bar sidebar button dispatches the same behavior.
- The toggle hides or unhides the whole sidebar, including vertical tabs and
  file/search panel controls.
- The vertical tab collapse/expand button folds any active sidebar panel back to
  the 44 px rail, or unfolds the session list when already folded.
- Clicking Files or Search switches the active sidebar panel.
- Clicking a session switches the active sidebar panel back to tabs/sessions.
- `FocusFiles` (`Cmd+Option+E` on macOS, `Ctrl+Shift+E` on Windows/Linux)
  opens the Files panel. `SearchFiles` (`Cmd/Ctrl+Shift+F`) opens Search and
  focuses its query input. Both actions are also exposed in the command
  palette.

## Width Rules

The sidebar width is one shared budget. In vertical-tab mode, the 44 px rail
plus whichever panel is active must equal the unfolded session-list width, so
switching between Sessions, Files, and Search never changes the terminal/editor
origin. In horizontal-tab mode the file/search panel uses that same saved width.

The file/search panel can resize across the available window width. It
participates in layout, so the terminal/editor starts after the sidebar instead
of being covered by it. The workspace owns this drag state because it has the
full layout budget. A capture overlay is rendered during drag so releasing the
mouse outside the handle exits resize mode reliably.

## Root Sync

Files and Search share the same root:

- terminal focus uses terminal cwd,
- editor focus uses active editor file parent,
- existing roots are preserved when they already contain the focused file,
- render fallback syncs the root if no folder has been set yet.

This keeps the sidebar useful when the user opens a tab that only contains an
editor pane or when focus moves between terminal and editor surfaces.
