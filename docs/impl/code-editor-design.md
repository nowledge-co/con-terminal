# Code Editor and Left Sidebar

Status: Implemented
Scope: editor pane surface, left sidebar tools, file explorer, search,
vertical-tab coexistence, keybinding/focus integration.

## Current Model

The code editor is a normal pane surface inside the workspace `PaneTree`. It is
not a separate top-level editor area and there is no standalone `con-editor`
crate. Terminal panes, editor panes, the input bar, and the agent panel all
share the existing workspace layout and close/focus machinery.

The left side of the window has two tab-orientation modes:

- horizontal tabs: file/search is the left sidebar panel,
- vertical tabs: the left sidebar has one rail and one active panel; Files,
  Search, and Sessions switch that panel without overlaying terminal content.

Hiding the left sidebar removes all left chrome. Unhiding restores the selected
tab orientation, the previous vertical-tab folded/unfolded mode, and the active
file/search slot.

## Left Sidebar

`crates/con-app/src/activity_bar.rs` owns the compact section switcher.
`ActivitySlot::Files` shows the file explorer and `ActivitySlot::Search` shows
workspace search. Clicking a different icon switches content and opens the
panel. In vertical-tabs mode the same section choices are surfaced in
`SessionSidebar` so the rail remains a single navigation surface.

`Cmd+B` is bound to `ToggleLeftPanel` and the user-facing label is "Toggle Left
Sidebar". The top bar sidebar button remains a first-class toggle for the same
left panel. The toggle hides or unhides the whole sidebar so terminal-only
workflows can keep a clean pane area.

The panel width is stored as `left_panel_width` in session state; old
`vertical_tabs_width` session files load through a serde alias. The vertical tab
folded/unfolded state persists as `vertical_tabs_pinned`. The active resize
gesture is owned by the workspace because it needs the full window width, agent
panel width, and pane layout constraints. While resizing, `render.rs` installs a
capture overlay so mouse movement and mouse-up events end the drag even if the
cursor leaves the handle.

## File Explorer

`FileTreeView` has an optional root. The workspace keeps it in sync with the
active focus:

- Terminal focus uses the active terminal cwd.
- Editor focus uses the active editor file's parent directory.
- If an editor file is inside the existing root, the root is preserved.
- If the root is missing at render time, the workspace performs a fallback sync
  from the currently focused pane.

Opening a file from the explorer routes through
`ConWorkspace::open_path_in_active_editor`, which reuses the active tab's shared
editor pane when possible.

## Search Panel

`SidebarSearchView` searches below the same root used by the file explorer. The
query input auto-grows from one to three lines and supports case-sensitive and
regular-expression modes.

Search intentionally has bounded work:

- `MAX_SEARCH_FILES = 800`
- `MAX_FILE_BYTES = 512 KiB`
- `MAX_RESULTS = 200`
- `MAX_MATCHES_PER_FILE = 20`

Results are grouped by file, show a per-file match count, and highlight the
matched text. The result list uses a real vertical scrollbar; it only becomes
visually relevant when the result content overflows.

## Editor Pane

`EditorView` is a lightweight multi-file editor pane:

- `EditorTab` pairs a `PathBuf`, `EditorBuffer`, and render cache.
- `EditorBuffer` owns text, cursor, selection, undo/redo, and revision state.
- Rendering uses GPUI `uniform_list` so only visible rows are laid out.
- Syntax highlighting is provided by `editor_syntax`.
- Basic language-server diagnostics are provided by `editor_lsp` when a server
  is available.
- Font family and size follow the terminal/code font settings instead of using
  a separate editor default.

The editor supports long single lines with horizontal scrolling. Cursor movement
and line-boundary actions scroll the cursor into view, including `Ctrl+A` and
`Ctrl+E`. The current cursor line renders with a subtle background, and double
click selects the word under the cursor.

Closing follows the pane model: `Cmd+W` closes editor files one by one. When the
last editor file in an editor pane closes, the pane is closed instead of
rendering a "No file open" placeholder.

## Markdown Preview

Markdown tabs (`language_for_path == "markdown"`) can switch in place between
source editing and a rendered preview. The toggle lives at the right end of the
editor tab bar (eye/code phosphor icon) and on `Cmd+Shift+V`
(`EditorTogglePreview`, scoped to `EditorView` like the other editor bindings).

- Preview state is per `EditorTab` and session-only — it is not serialized into
  workspace layouts, and the pane tree is untouched.
- The rendered view reuses the agent panel's markdown renderer
  (`chat_markdown.rs`) through `render_parsed_chat_markdown_file_preview`,
  wrapped in a scroll container by `editor_preview.rs`. Whole-document
  rendering, no block virtualization.
- Reparsing is live: `schedule_preview_parse` debounces 300 ms, parses on the
  background executor, and caches the result per tab keyed on the buffer
  `revision`. A generation counter drops stale results, mirroring the LSP
  did-change debounce. The render pass reschedules whenever the cached revision
  lags the buffer.
- Local images render inline: `mdast::Node::Image` parses into a dedicated
  `MarkdownInline::Image`, and a paragraph holding a single image renders as a
  GPUI `img()` element with the path resolved against the markdown file's
  directory (`resolve_image_source`). Remote `http(s)` images also render
  inline through GPUI's async image asset loader (alt text shows while
  loading and as the failure fallback); only `data:` and empty URLs degrade
  to alt text. Everything keeps degrading to alt text when no base dir is
  set — so agent panel rendering is unchanged.
- Raw HTML renders structurally instead of leaking source text: block-level
  HTML goes through `html5ever` (`parse_html_blocks` — headings, paragraphs,
  lists, quotes, `<pre>`, and linked `<img>` badges map onto markdown blocks),
  while inline HTML arrives as single tag tokens and is reconstructed with a
  container stack in `parse_inline_nodes` (`<strong>`/`<em>`/`<s>`/`<code>`/
  `<kbd>`/`<a>`/`<img>`/`<br>`; `<script>`/`<style>` content is dropped and
  unknown tags degrade to their text).
- Preview mode is read-only: text mutation methods (`insert_text`,
  `delete_*`, `cut_selection`, `undo`) no-op, and editor mouse hit-testing is
  skipped while a preview is showing.

## Focus and Keybindings

Editor text-editing bindings are scoped to `EditorView` so terminal keys such as
Enter and Backspace are not intercepted globally. App-level shortcuts remain
global by default so `Cmd+T`, `Cmd+W`, tab navigation, command palette, and
left-sidebar toggles still work when an editor pane is focused or when a tab
contains only an editor pane.

See `docs/impl/keybindings.md` for the binding-spec table and scope rules.

## Code Map

```text
crates/con-app/src/activity_bar.rs
  File/search section switcher and slot events.

crates/con-app/src/sidebar.rs
  Folded/unfolded vertical tabs, tab hover cards, drag/drop, and tab actions.

crates/con-app/src/file_tree_view.rs
  File explorer rows and OpenFile events.

crates/con-app/src/sidebar_search_view.rs
  Sidebar search query/options/results rendering and bounded filesystem scan.

crates/con-app/src/editor_buffer.rs
  Text, cursor, selection, undo/redo, and line movement primitives.

crates/con-app/src/editor_view.rs
  Multi-file editor pane, tabs, hit-testing, scrolling, rendering, LSP events.

crates/con-app/src/editor_syntax.rs
  File type detection and syntax highlight runs.

crates/con-app/src/editor_lsp.rs
  Best-effort language-server process integration and diagnostics parsing.

crates/con-app/src/editor_preview.rs
  Scrollable markdown preview body on top of the chat markdown renderer.

crates/con-app/src/workspace/editor_actions.rs
  Editor action dispatch and text-key fallback handling.

crates/con-app/src/workspace/render.rs
  Activity rail, left panel layout, resize overlay, editor pane composition.
```

## Validation

Relevant checks:

- `cargo check -p con`
- `cargo test -p con workspace -- --nocapture`
- `cargo test -p con sidebar_search -- --nocapture`
- `cargo test -p con editor_view -- --nocapture`
