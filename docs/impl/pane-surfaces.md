# Pane Surfaces

## Summary

Con's public pane model now has two layers:

- **Pane**: a visible split rectangle in a tab.
- **Surface**: a live terminal session hosted by a pane.

Every pane always has one active surface. A pane may also host additional
inactive surfaces, which behave like pane-local terminal tabs. The existing
pane APIs, built-in agent tools, and terminal-agent benchmarks continue to see
only the active surface of each pane.

This is an additive control-plane feature for external orchestrators that need
interactive subagent patterns without creating unbounded visible splits.

## Why This Exists

Interactive subagent tools often want this lifecycle:

1. First worker gets a visible split, usually to the right of the caller.
2. Later workers join that same worker pane as tabs.
3. The orchestrator can read, send keys, focus, rename, and close those tabs.
4. When the last owned worker tab exits, the worker pane can be closed.

cmux calls the inner unit a `surface`, so Con uses the same word for the
control-plane concept. In product terms, a surface is simply a terminal session
inside a pane.

This is distinct from the lower-level Ghostty/native rendering surface used by
the terminal backend. In this document, "surface" always means the
control-plane terminal session inside a pane.

## Compatibility Contract

The compatibility rule is strict:

- `panes.*` targets panes and operates on the active surface only.
- The built-in agent prompt/tool set keeps the existing pane model.
- Existing benchmark fixtures should keep using `panes.*` unless a test is
  explicitly about pane-local surfaces.
- Surface APIs are opt-in under `surfaces.*`.

This prevents a new abstraction from changing the behavior of Con's benchmarked
agent harness or the user-facing pane picker.

## Control API

The JSON-RPC methods are:

- `tree.get`: returns tabs, panes, and pane-local surfaces.
- `surfaces.list`: lists surfaces in a tab, optionally scoped to one pane.
- `surfaces.create`: creates a new surface inside an existing pane.
- `surfaces.split`: creates a split pane with an initial surface.
- `surfaces.focus`: activates a surface in its pane.
- `surfaces.rename`: updates the surface title shown in the pane-local strip.
- `surfaces.close`: closes a surface; it refuses to close the last surface in a
  pane unless the caller opts into closing an owned ephemeral pane.
- `surfaces.read`: reads recent terminal content from a surface.
- `surfaces.send_text`: writes literal text bytes to a surface.
- `surfaces.send_key`: sends a small named key set (`escape`, `enter`, `tab`,
  `backspace`, `ctrl-c`, `ctrl-d`) to a surface.
- `surfaces.wait_ready`: waits until the surface has a live terminal session
  with shell integration or the timeout expires, then returns readiness
  metadata.

Surface targeting accepts:

- `surface_id`: stable within the tab for the lifetime of the surface.
- `pane_id`: stable within the tab for the lifetime of the pane.
- `pane_index`: current visible pane position, useful for humans but not ideal
  for follow-up automation.

Prefer `surface_id` for follow-up automation.

## Human Entry Points

Surface control is also available from Command Palette and the terminal
right-click menu so humans can discover and exercise the same pane-local model
without using `con-cli`.

The visual rule is intentionally scoped:

- A **pane** is the visible split region.
- A **surface** is a tab-like terminal session selected inside one pane.
- Ordinary one-surface panes show no surface chrome, except while their surface
  name is being edited. Starting rename reveals the same compact header and
  input used by named surfaces, even with pane titles hidden. Cancelling returns
  to the original chrome; confirming an empty name removes the temporary header
  unless other surfaces or an owner still require it.
  Palette dismissal must preserve the rename input's explicit focus; Enter and
  Escape end the edit and restore terminal focus.
- Panes with multiple surfaces, an orchestrator owner, or an explicit surface
  title show compact inset tabs in the pane header, replacing its ordinary
  title rather than adding a second row. The default header height is 24 points,
  shared with file tabs and scaled with UI size, not terminal font size. Pane
  dividers own layout structure, while surface chrome only identifies terminal
  sessions inside that pane. The
  rail is local to that pane; it never switches surfaces in sibling panes.
  Tabs scroll within the space before pane zoom/close controls. Surface close
  buttons use the component control, visible on the active tab or row hover;
  double-click rename and the context menu remain available. Hiding pane titles
  does not hide surface navigation. Retain horizontal scroll state for each
  selected surface. Reveal it after viewport, title-width or surface-count
  changes, not on every repaint, so manual scrolling is not pulled back.

- `New Surface Tab`: creates a new terminal session inside the focused
  pane, gives it the next stable `Surface N` label, and focuses it.
- `New Surface Pane Right`: creates a new right split from the focused pane,
  then initializes that visible pane with its first `Surface 1` tab.
- `New Surface Pane Down`: creates a new down split from the focused pane,
  then initializes that visible pane with its first `Surface 1` tab.
- `Next Surface Tab`: cycles forward through surfaces hosted by the
  focused pane.
- `Previous Surface Tab`: cycles backward through surfaces hosted by the
  focused pane.
- `Rename Current Surface`: starts inline rename for the active
  surface in the focused pane. The same rename action is available from the app
  menu, terminal context menu, and by double-clicking a surface tab in the
  pane-local strip.
- `Close Current Surface`: closes the active surface when the focused pane has
  more than one surface. If the surface was created as an owned palette split,
  closing that last surface also closes the worker pane. It intentionally does
  nothing for the last non-owned surface in a pane; ordinary pane closing
  remains the pane-level command.

Surface tabs support direct manipulation:

- Click a surface tab to focus that surface in its pane.
- Double-click a surface tab to rename it inline. Press Enter to commit or
  Escape to cancel.
- Right-click a surface tab for Rename / Close.
- Use the close glyph on a closable surface tab to remove that surface without
  opening the full terminal context menu.

Default surface shortcuts are configurable in Settings -> Keyboard Shortcuts:

- macOS: Cmd-Option-T creates a surface in the focused pane; Cmd-Option-D /
  Cmd-Option-Shift-D create surface splits; Cmd-Control-[ and
  Cmd-Control-] cycle; Cmd-Option-R renames; Cmd-Option-Shift-W closes.
- Windows/Linux: Alt-Shift-T creates a surface in the focused pane;
  Alt-Shift-Right / Alt-Shift-Down create surface splits; Alt-Shift-[ and
  Alt-Shift-] cycle; Alt-Shift-R renames; Alt-Shift-X closes.

These palette actions are deliberately pane-local. They do not change the
existing `panes.*` control-plane contract, the built-in agent harness target
model, or terminal-agent benchmark assumptions.

The terminal context menu uses the same GPUI actions as the Command Palette and
app menu instead of custom callbacks. That keeps right-click behavior aligned
with keyboard and automation entry points: the menu first restores focus to the
clicked terminal, then dispatches the selected action through the normal window
action path.

This mirrors the interactive-subagent flow: create the first worker as a
visible split, then add later workers as surfaces inside that worker pane so
parallel agents do not keep shrinking the main terminal layout.

## pi-interactive-subagents Readiness

Con is API-ready for the pi-interactive-subagents pattern, but it is not a
byte-for-byte cmux CLI clone. The preferred integration is a Con-specific
backend that uses `con-cli --json` instead of parsing cmux text handles:

- First worker: `con-cli --json surfaces split --location right --title <name>
  --owner pi-interactive-subagents`
- Remember the returned `pane_id` and `surface_id`.
- Later workers: `con-cli --json surfaces create --pane-id <worker_pane_id>
  --title <name> --owner pi-interactive-subagents`
- Drive workers with `surfaces wait-ready`, `surfaces send-text`,
  `surfaces send-key`, and `surfaces read`.
- Clean up with `surfaces close --surface-id <surface_id>
  --close-empty-owned-pane`.

This avoids cmux's `identify --surface` round trip because Con returns the
worker pane id directly from `surfaces split`.

Release artifacts now include `con-cli` as part of the core install. Homebrew
and the one-line installers expose it on PATH, so an orchestrator adapter can
depend on `con-cli` being present after a normal Con install instead of asking
users to build the workspace from source.

## CLI Examples

First create a visible worker pane:

```bash
con-cli --json surfaces split \
  --tab 1 \
  --pane-id 0 \
  --location right \
  --title worker-1 \
  --owner pi-interactive-subagents \
  --command "codex"
```

Then create more worker sessions inside the same pane:

```bash
con-cli --json surfaces create \
  --tab 1 \
  --pane-id <worker_pane_id> \
  --title worker-2 \
  --owner pi-interactive-subagents \
  --command "codex"
```

Wait for the worker before driving it:

```bash
con-cli --json surfaces wait-ready --surface-id <surface_id> --timeout 10
```

Drive a worker:

```bash
con-cli --json surfaces send-text --surface-id <surface_id> "explain this repo"
con-cli --json surfaces send-key --surface-id <surface_id> enter
con-cli --json surfaces read --surface-id <surface_id> --lines 120
```

Close a worker surface:

```bash
con-cli --json surfaces close --surface-id <surface_id>
```

Close the last surface and its owned worker pane:

```bash
con-cli --json surfaces close \
  --surface-id <surface_id> \
  --close-empty-owned-pane
```

`surfaces split` marks its initial pane as closeable by default. Use
`--keep-pane-when-last` when the pane should survive after its last surface
closes.

## Lifecycle Rules

- Only active surfaces are visible.
- Hidden surfaces keep running and continue receiving terminal output.
- Hidden surfaces keep the same PTY/grid size as their host pane. Con resizes
  inactive surfaces to the visible terminal host bounds even while they are not
  rendered, so TUI agents that start in background surfaces see the correct
  rows and columns before focus moves to them.
- On macOS, surface focus is also a native geometry checkpoint. Activating a
  hidden surface must revalidate the embedded Ghostty surface size against the
  current pane bounds before exposing it, because rows and columns are part of
  the TUI protocol. Selecting the already-active surface runs the same
  checkpoint.
- Tab, pane, window, and app close paths tear down every surface, not just the
  active surface.
- If a non-last surface exits, Con removes only that surface.
- If the last surface in a pane exits, Con follows the existing pane close
  escalation: close pane, then close tab, then close the workspace window.

## Debugging Geometry Drift

If a macOS user reports that a TUI is clipped after pane or surface operations,
ask for one opt-in diagnostic run before guessing at the layout path. A normal
beta/dev build is enough; no special instrumented build is required.

For an installed beta app:

```bash
CON_LOG_FILE="$HOME/Desktop/con-surface-geometry.log" \
CON_GHOSTTY_PROFILE=1 \
RUST_LOG=con::perf=info,con_ghostty::perf=info,con=warn,con_core=warn,con_agent=warn \
"/Applications/con Beta.app/Contents/MacOS/con"
```

After reproducing the clipping, capture the live surface state:

```bash
con-cli --json surfaces list > "$HOME/Desktop/con-surfaces.json"
```

Ask for both files privately if paths or terminal titles are sensitive. Useful
signals in the log are `surface geometry mismatch`, `surface resize request`,
and `update_frame`; together with `con-surfaces.json`, they show whether the
visible pane bounds, Ghostty pixel size, and TUI rows/columns agree.

## Current Limit

Session restore persists the split layout, every surface's ID/name/owner/cwd,
and the selected surface. Optional restored screen text is private runtime
continuity data and is never exported in workspace profiles. Restored surfaces
start new terminal processes; restoring their shape or text does not resume an
agent's native conversation or preserve its original process.
