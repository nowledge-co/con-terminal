# Workspace Module Map

`crates/con-app/src/workspace/` owns the live GPUI window: tabs, pane trees,
agent/input chrome, settings, session persistence, terminal event wiring, and
control-plane dispatch.

The module split keeps those responsibilities explicit:

| Module | Responsibility |
|---|---|
| `mod.rs` | `ConWorkspace` field definitions and shared imports. Keep this file structural. |
| `types.rs` | Workspace-local data shapes such as tabs, pane drag state, pending control requests, and suggestion results. |
| `lifecycle.rs` | Workspace construction, terminal creation, native view visibility, and deferred window-aware work. |
| `control_surfaces.rs`, `control_requests.rs`, `control_agent_tools.rs` | Socket/control-plane routing, pane/surface targeting, and agent-visible terminal operations. |
| `session_worker.rs`, `session_state.rs`, `terminal_factory.rs` | Session persistence, layout/profile restore, and Ghostty terminal construction helpers. |
| `agent_panel_events.rs`, `input_events.rs`, `sidebar_settings.rs`, `editor_actions.rs` | UI event handlers for agent panel, input bar, sidebar, editor actions, palette, settings, and theme changes. |
| `chrome.rs`, `chrome_actions.rs`, `caption.rs` | Window chrome math, transparency seam guards, pane scope picker, layout profile actions, and non-macOS caption buttons. |
| `pane_actions.rs`, `tab_actions.rs`, `suggestions.rs` | Terminal commands, pane/surface/tab lifecycle, shell history, suggestions, and tab summaries. |
| `render.rs`, `render/top_bar.rs`, `render/popups.rs` | GPUI render tree, top chrome/tab strip composition, and workspace-level popup overlays. Rendering stays isolated from behavior modules. |
| `tab_presentation.rs`, `tab_motion.rs`, `helpers.rs`, `tests.rs` | Tab/pane naming, presentation-only tab slot motion, geometry helpers, rename helpers, and workspace unit tests. |

Rules for future changes:

- Put behavior in the owning action/control/session module; do not grow
  `mod.rs`.
- Keep editor text-key handling in `editor_actions.rs`; `render.rs` should only
  wire events to the focused entity.
- Keep render-only composition in `render.rs` or a focused `render/`
  submodule; if a render change needs state mutation, move that mutation to the
  owning behavior module first.
- If a workspace helper is used by multiple sibling modules, expose it as
  `pub(super)` and keep it module-private to `workspace`.
- Keep each workspace module small enough for one review pass. If a file nears
  the line-count budget again, extract a coherent ownership seam instead of
  adding another mixed-responsibility block.
