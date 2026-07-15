# con: The Native Terminal Emulator with a Built-in AI Harness

## Vision

An open-source, GPU-accelerated terminal emulator that treats AI agents as first-class citizens. It aims for high terminal correctness, native performance, and a deeply integrated agent harness — all in Rust.

**Why con exists:**

- Existing terminals bolt AI on as an afterthought
- Agent-native workflows deserve deep terminal integration, not wrapper hacks
- The terminal is the last IDE-free surface that hasn't been reinvented for the AI era

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                     con (binary)                        │
│                                                         │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │  GPUI Shell  │  │  Agent Panel │  │  Command Bar  │  │
│  │  (windows,   │  │  (chat, tool │  │  (palette,    │  │
│  │   tabs,      │  │   calls,     │  │   search,     │  │
│  │   splits)    │  │   context)   │  │   actions)    │  │
│  └──────┬───────┘  └──────┬───────┘  └──────┬────────┘  │
│         │                 │                  │           │
│  ┌──────┴─────────────────┴──────────────────┴────────┐  │
│  │                    con-core                        │  │
│  │  ┌────────────┐ ┌────────────┐ ┌────────────────┐  │  │
│  │  │ Terminal    │ │ Agent      │ │ Session        │  │  │
│  │  │ Manager    │ │ Harness    │ │ Manager        │  │  │
│  │  │            │ │ (shared)   │ │                │  │  │
│  │  │            │ │ + per-tab  │ │                │  │  │
│  │  │            │ │ Sessions   │ │                │  │  │
│  │  └─────┬──────┘ └─────┬──────┘ └────────────────┘  │  │
│  └────────┼──────────────┼────────────────────────────┘  │
│           │              │                               │
│  ┌────────┴────────┐  ┌──┴──────────────┐               │
│  │ con-ghostty     │  │ con-agent       │               │
│  │ (platform       │  │ (rig 0.34,      │               │
│  │  terminal       │  │  multi-provider)│               │
│  │  backend)       │  │                 │               │
│  ├─────────────────┤  └─────────────────┘               │
│  │ con-terminal    │                                    │
│  │ (themes +       │                                    │
│  │  palette data)  │                                    │
│  └─────────────────┘                                    │
└─────────────────────────────────────────────────────────┘
```

---

## Crate Structure

```
kingston/
├── Cargo.toml                 # workspace root
├── crates/
│   ├── con/                   # main binary — GPUI app shell
│   │   └── src/
│   │       ├── main.rs         # Application bootstrap + keybindings
│   │       ├── workspace/      # window, tabs, splits, render, session wiring
│   │       ├── ghostty_view.rs  # embedded Ghostty surface view
│   │       ├── agent_panel.rs  # side panel for AI chat / tool output
│   │       ├── input_bar.rs    # smart input bar (NLP/shell/skill modes)
│   │       ├── pane_tree.rs    # split pane layout tree
│   │       ├── settings_panel.rs # provider config UI (Cmd+,)
│   │       ├── sidebar.rs      # vertical-tabs side panel (rail / hover-peek / pinned)
│   │       └── theme.rs        # Flexoki themes (Light default, Dark available)
│   │
│   ├── con-core/              # shared logic, no UI dependency
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── harness.rs     # AgentHarness (shared) + AgentSession (per-tab)
│   │       ├── session.rs     # persist/restore workspace state
│   │       ├── config.rs      # con config (TOML)
│   │       └── suggestions.rs # AI shell command suggestions
│   │
│   ├── con-terminal/          # terminal themes and palette helpers
│   │   └── src/
│   │       ├── lib.rs
│   │       └── theme.rs       # built-in themes + Ghostty theme import
│   │
│   ├── con-ghostty/           # ghostty FFI — primary macOS backend
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ffi.rs         # C API bindings (libghostty types, functions)
│   │       └── terminal.rs    # GhosttyApp, GhosttyTerminal, TerminalState, action callbacks
│   │
│   ├── con-agent/             # AI agent harness (Rig 0.40)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── provider.rs    # Multi-provider Rig agent construction and streaming
│   │       ├── hook.rs        # ConHook — AgentHook lifecycle bridge
│   │       ├── tools.rs       # Rig Tool impls for terminals, tmux, files, and workspaces
│   │       ├── context.rs     # terminal context extraction for agent
│   │       ├── conversation.rs # conversation state + Rig Message conversion
│   │       └── skills.rs      # skill registry + AGENTS.md parser
│   │
│   └── con-cli/               # headless CLI + socket client (stub)
│       └── src/main.rs
│
├── postmortem/                # incident & integration postmortems
│
└── docs/
    ├── study/                 # technology study notes
    └── impl/                  # implementation notes
```

See `docs/impl/workspace-modules.md` for the workspace module ownership map.

---

## Key Technical Decisions

### 1. UI Framework: GPUI

**Why GPUI over alternatives (Tauri, Iced, egui, Slint):**

| Criteria | GPUI | Tauri | Iced | egui |
|----------|------|-------|------|------|
| GPU rendering | Metal + Blade (OpenGL) | WebView | wgpu | wgpu |
| Text quality | Production (Zed-grade) | Web fonts | Good | Basic |
| Product target today | macOS-native app | All | All | All |
| Maturity | Zed production + 23+ apps | Very mature | Growing | Mature |
| Terminal canvas | canvas() API | DOM hacks | Custom widget | Custom paint |
| Rust-native | Yes | Hybrid JS/Rust | Yes | Yes |

GPUI gives us Zed-level text rendering quality (critical for a terminal) and a proven canvas API for custom grid drawing. The `termy` project in awesome-gpui proves terminal embedding works.

### 2. Terminal Core: Ghostty-Only Runtime

con now runs one terminal runtime: full Ghostty embedded via its C API.

- Ghostty owns PTY lifecycle, VT parsing, scrollback, and rendering
- con embeds one shared Ghostty app per window and one Ghostty surface per split inside GPUI layouts
- `con-ghostty` stays thin: app lifecycle, surface lifecycle, callbacks, and safe Rust accessors
- `con-terminal` now only owns theme and palette data used to drive Ghostty config

Native Ghostty splits do not imply a single monolithic surface. Ghostty's own macOS architecture also uses a host-managed surface tree with one surface per split. The right migration path for con is therefore a Ghostty-driven surface-tree controller, not collapsing the whole window into one terminal surface.

`TerminalPane` still exists, but it is now a product abstraction around one kind of pane, not a multi-backend compatibility layer.

### 3. Pane Runtime Observer

The long-term product requirement is not "tmux detection."
It is durable pane runtime observability.

A pane may contain a nested stack:

- local shell
- SSH connection
- remote shell
- tmux
- another shell
- agent CLI / vim / htop

The architecture must model that explicitly.

**Three-layer design:**

1. **Backend facts** — Ghostty action callbacks, process-exited state, titles, PWD, command-finished events, PTY input generations, visible text
2. **Pane runtime tracker** — reducer over backend observations, typed shell probes, and con-originated actions; command-boundary freshness; scope-stack inference; confidence
3. **Consumers** — agent prompt, `list_panes`, approvals, sidebar, notifications, resume surfaces

**Important constraint:** shell metadata and visible-app identity are not the same thing. If con cannot prove the foreground runtime, it must say `unknown` instead of promoting title or screen-pattern guesses into product state.

**Current foundation:** Ghostty already gives con strong terminal facts, but the embedded C API does not yet expose foreground-process identity, authoritative command text, alternate-screen state, remote-host identity, or Ghostty's richer internal semantic prompt/runtime state. con therefore needs its own pane runtime tracker instead of pushing more product logic into prompt heuristics.

**Reducer model:** every pane now has a stateful tracker that merges three classes of input:

- backend observations from Ghostty
- typed shell-scope probes such as `probe_shell_context`
- con-originated action history such as pane creation, visible shell exec, raw input, and process exit

This lets con preserve causal history without confusing it for current foreground truth. A fresh shell prompt clears active tmux/app identity unless a fresh typed probe re-establishes it. Historical actions remain visible as evidence for how the pane was reached.

One important middle layer now exists for remote work: **con-managed SSH continuity**. When con created or recently drove an SSH pane, and the current screen still looks prompt-like without tmux or TUI contradictions, that pane remains a reusable remote workspace even if fresh shell integration is absent. This is not promoted to foreground truth, but it is strong enough to prevent duplicate remote panes across follow-up turns.

**Long-term direction:** if con needs process-group identity or richer semantic prompt exports, the right move is to extend libghostty's C API upstream rather than reintroducing a second terminal runtime locally.

See `docs/impl/pane-runtime-observer.md`.

### 3b. Agent Runtime Control Plane

Observability alone is not enough.

The agent also needs an explicit control plane that separates:

- con pane addresses
- nested runtime targets such as SSH scopes, tmux sessions, tmux panes, and editors
- control channels such as visible shell exec, local hidden exec, tmux control, and raw TUI input

This is the architectural boundary that prevents failures like:

- running a local hidden command for a remote task
- confusing a tmux pane with a con pane
- typing shell commands into `nvim`

The long-term tool model should therefore be split by control layer, not just by "pane plus command." See `docs/impl/agent-runtime-control-plane.md`.

There is no single terminal-wide protocol comparable to Chrome DevTools Protocol.
con therefore needs a layered control plane with explicit protocol attachments:

- Ghostty surface and VT facts
- shell prompt attachments
- tmux control attachments
- app-native RPC attachments
- OS and PTY process facts

The first concrete attachment after the Ghostty surface is a proven shell-prompt attachment. That is where con can safely expose read-only shell probes such as `$TMUX`, `$SSH_CONNECTION`, tmux session/window/pane ids, and editor socket hints.

The harness now only preloads **silent** attachments automatically. Visible shell probes stay explicit tool calls. This keeps the terminal calm on the first turn while still letting con reuse tmux state and remote workspace anchors across follow-up turns.

See `docs/study/terminal-control-plane.md`.

### 4. AI Agent Harness: Rig

**Why [Rig](https://rig.rs/) over alternatives:**

| Option | Verdict |
|--------|---------|
| **Rig** | Most mature Rust AI agent framework. Multi-provider (OpenAI, Anthropic, Cohere, etc.). Tool-use, RAG, agent abstractions. 24.3% CPU — most efficient in benchmarks. |
| **rust-genai** | Multi-provider client but no agent abstractions (tools, chains). Just an API client. |
| **Embed Bun + ai-sdk** | Not feasible — Bun has no embedding API. Would need to spawn a subprocess. |
| **AutoAgents** | Multi-agent focused, heavier weight. Good for orchestration but overkill for terminal agent. |
| **Raw HTTP** | Maximum control but months of provider-specific work. |

**Rig gives us:**

- `CompletionClient::agent()` builder — preamble + tools + model config in one chain
- `Tool` trait — type-safe tool definitions with `Args` (Deserialize), `Output` (Serialize), `Error`
- streaming prompt requests with explicit `Vec<Message>` history
- `AgentHook` events for text deltas, tool approval, and tool results
- `anthropic::Client::new(api_key)` — direct Anthropic provider with model constants (`CLAUDE_4_SONNET`)
- Agent loop with automatic tool calling (up to N turns)

**Current integration:** con-agent implements 34 Rig tools spanning visible terminal control, tmux, local files, remote workspaces, and agent CLI orchestration. Provider-native clients and OpenAI-compatible endpoints share one harness path, with configurable models, credentials, and base URLs. The harness runs on a shared tokio runtime. Provider settings are configurable from the settings window.

**Agent lifecycle (AgentHook):** Each agent request builds a streaming prompt with its conversation history and a `ConHook` registered on the agent. The hook maps Rig's typed lifecycle events into Con's UI events for tool-call start, tool result, and text deltas. For dangerous tools (`shell_exec`, `terminal_exec`, `file_write`, `edit_file`), the hook waits on a per-request approval channel before execution. Safe tools (`file_read`, `list_files`, `search`) execute immediately. A 5-minute timeout prevents indefinite hangs if the UI becomes unresponsive. Rig 0.40 uses an exact total model-call budget; Con converts its existing `max_turns` setting through a compatibility function that preserves the previously shipped ceiling.

**Visible terminal tool:** The `terminal_exec` tool is con's core differentiator. Instead of running commands in a hidden subprocess, it writes commands to the user's visible Ghostty pane. Output is captured via Ghostty's command-finished signal, with a bounded recent-output fallback when shell integration is unavailable. The user sees every command the agent runs in real time — full transparency.

**Streaming cancellation:** Agent requests can be cancelled mid-stream. The harness maintains an `Arc<AtomicBool>` cancellation flag checked between stream items. The agent panel shows a "Stop" button during streaming. Cancellation preserves the partial response accumulated so far.

**Per-request approval channels:** Each `send_message()` creates a fresh crossbeam channel pair. The sender is delivered to the UI via `ToolApprovalNeeded` events. The receiver is owned by the `ConHook` for that request. This eliminates race conditions between concurrent agent requests — only one hook instance reads from each channel.

**Per-tab agent sessions:** Each tab owns an independent `AgentSession` — its own conversation, event channels, terminal exec channels, and cancellation flag. The `AgentHarness` (shared per window) holds only infrastructure: the tokio runtime (2 worker threads), config, and skill registry. When the user switches tabs, the agent panel swaps its `PanelState` in and out via `std::mem::replace()`, preserving each tab's conversation history and in-flight state. Background tabs continue processing agent events into their cached `PanelState`. Terminal exec requests route to the originating tab's pane tree, not the active tab — so an agent running in Tab 2 executes commands in Tab 2 even if the user has switched to Tab 1.

**Conversation round-trip:** The conversation state (`Arc<Mutex<Conversation>>`) is shared between the main thread and spawned tasks. After the agent completes, the assistant message is added back to the conversation, enabling multi-turn context. Each tab persists its own `conversation_id` across restarts.

**Escape hatch:** If we later need ai-sdk features, we can spawn a Bun/Node sidecar process and communicate over IPC. This is a pragmatic fallback, not a primary architecture.

### 5. Platform Strategy

con now has three platform states:

- macOS: shipped, using the embedded libghostty + AppKit path
- Windows: beta, using `libghostty-vt` + ConPTY + a local D3D11/DirectWrite renderer
- Linux: preview, using Unix PTY + shared `libghostty-vt` + a GPUI-owned per-row `StyledText` paint path. SGR colors / bold / italic / underline / inverse + block cursor all work, the window ships with client-side decorations, transparent ARGB visual, rounded corners, and KWin-Wayland backdrop blur where the compositor exposes it. The long-term GPUI-owned glyph-atlas grid renderer (matching the D3D11/DirectWrite path Windows uses) is the remaining Linux work.

The platform architecture is no longer "macOS only," but it is still not
"one backend everywhere."

- GPUI provides the host app shell on all supported platforms
- Ghostty remains the preferred terminal truth source whenever its
  embedding surface is available
- platform-specific backend glue still matters:
  - AppKit on macOS
  - D3D11/ConPTY on Windows today
  - Unix PTY + GPUI-owned renderer on Linux; see `docs/impl/linux-port.md`

Important consequence:

- Windows proved that con can ship a local backend when upstream
  embedding is unavailable.
- Linux feasibility work showed that upstream Ghostty and GPUI both have
  real Linux stacks, but their embedding boundaries do not line up for
  con today.
- con therefore takes the same delivery stance on Linux that it used on
  Windows: ship a local backend instead of waiting on upstream embed
  hooks.

---

## Core Features (MVP → v0.1)

### Phase 1: Terminal That Works

- [x] GPUI window with single terminal pane
- [x] Embedded Ghostty surface lifecycle and input forwarding
- [x] Dynamic terminal resize (fills available window space)
- [x] Scrollback buffer with mouse wheel navigation
- [x] 256-color + truecolor
- [x] DEC private modes: DECCKM, DECAWM, alt screen, bracketed paste
- [x] DA/DSR responses written back to PTY
- [x] Application cursor keys (SS3 mode for vim/less/top)
- [x] Text style rendering: italic, underline, strikethrough, dim, inverse
- [x] Basic tabs (Cmd+T / Cmd+W) with tab bar and OSC title
- [x] Font size from config (config.toml terminal.font_size)
- [x] CWD display in input bar from OSC 7
- [x] Mouse text selection (click-drag, auto-copy, Cmd+C copy)
- [x] Clipboard paste (Cmd+V) with bracketed paste mode support
- [x] Terminal media ingress: file drops / file-path clipboard paste as quoted paths, and image clipboard paste forwarded to TUI apps via raw Ctrl+V
- [x] Cmd+1..9 tab switching
- [x] Session persistence (tabs, active tab, agent panel state)
- [x] Kitty keyboard protocol (CSI u encoding, push/pop/query flags)
- [x] Split panes (macOS: Cmd+D / Cmd+Shift+D, Windows/Linux: Alt+D / Alt+Shift+D, pane tree)

### Phase 2: Agent Harness

- [x] Side panel for AI chat (Cmd+L to toggle)
- [x] Terminal context injection (last N lines, current command, cwd)
- [x] Tool execution: agent can run shell commands in terminal
- [x] Multi-provider config via Rig 0.40
- [x] Settings panel (Cmd+,) with provider selector and model config
- [x] Smart input bar (shell/agent/smart modes)
- [x] Agent notification system (blue dot on tab when agent responds)

### Phase 3: Agent Lifecycle & Tool Transparency

- [x] AgentHook integration — tool calls visible in agent panel
- [x] Tool danger classification (safe: file_read, search; dangerous: shell_exec, file_write)
- [x] Per-request approval channels (no cross-request interference)
- [x] Approval timeout (5 minutes, denies on expiry)
- [x] Conversation round-trip (assistant messages added back for multi-turn)
- [x] Agent status indicators (Idle/Thinking/Responding)
- [x] Tool call rendering (in-progress dots, completed results)
- [x] Approval dialog UI (inline approval cards with Allow/Deny)
- [x] Streaming via `stream_prompt()` with real-time token rendering

### Phase 4: Agent Chat Polish

- [x] Structured tool call cards (icon, name, formatted args, result)
- [x] Inline approval cards for dangerous tools (Allow/Deny buttons)
- [x] Scrollable agent panel
- [x] Collapsible step timeline (click to expand/collapse)
- [x] Auto-approve toggle in settings UI (switch with live config propagation)
- [x] Streaming text rendering via `stream_prompt()`

### Phase 5: Deep Integration

- [x] OSC 133 command block tracking (prompt/command/exit code detection)
- [x] Command palette (Cmd+Shift+P) with fuzzy search and keyboard nav
- [x] Command history in agent context (last 10 commands with exit codes)
- [x] Inline AI suggestions (debounced completion engine with caching)
- [x] Command block actions (copy output, re-run, explain via agent)
- [x] Pane-safe agent execution guards and typed control-plane state
- [ ] Authoritative SSH scope detection on embedded Ghostty
- [ ] Authoritative tmux / TUI runtime detection on embedded Ghostty
- [x] Conversation history + search (save/load/list, new chat, history panel)

### Phase 6: Polish

- [x] Session persistence and restore
- [x] Cursor blink (500ms timer, resets on keypress)
- [x] Scrollback indicator (floating pill showing "N lines up")
- [x] Agent panel auto-scroll (ScrollHandle on messages container)
- [x] Double-click word selection, triple-click line selection
- [x] Theme-aware cursor and selection colors
- [x] Sidebar synced with tab state, new session button
- [x] Centered settings and command palette overlays with shadows
- [x] Code block rendering in agent panel (triple-backtick fences)
- [x] Shell mode refocuses terminal after command submit
- [x] Unified input routing (smart mode classifies shell/agent/skill)
- [x] Skills wired end-to-end (/explain, /fix, /commit, /test, /review)
- [x] Command palette expanded (clear, focus, toggle sidebar, cycle mode)
- [x] Terminal settings in Settings UI (font size, theme)
- [x] Cmd+A select all, Cmd+K clear scrollback
- [x] Configurable keybindings (config.toml keybindings section)
- [ ] Plugin system (Lua or WASM)
- [ ] Auto-update (Sparkle on macOS)
- [ ] CLI tool (`con` command for scripting)

### Phase 7: Agent Capabilities & Terminal Polish

- [x] Rig 0.36 → 0.40 migration (hooks v2, structured tool metadata, unified prompt responses)
- [x] Rich context injection (git diff, project structure, XML-tagged system prompt)
- [x] Visible terminal tool (terminal_exec — commands execute in user's visible PTY)
- [x] Surgical file editing tool (edit_file — find & replace, not full overwrite)
- [x] Directory listing tool (list_files — .gitignore-aware)
- [x] Streaming cancellation (Stop button, partial response preserved)
- [x] Resizable agent panel (drag divider, width persisted in session)
- [x] Extended thinking display (collapsible sections in agent panel)
- [x] Theme configurability (4 built-in themes, live switching, settings picker)
- [x] Per-tab agent sessions (each tab owns its own conversation, context, and approval state)

---

## Agent ↔ Terminal Integration Design

This is what makes con different from "terminal + chatbot sidebar."

### Terminal Context Awareness

```
Terminal Output
     │
     ▼
┌─────────────┐     ┌──────────────┐
│ OSC 133     │────▶│ Command      │
│ Detection   │     │ Block Parser │
└─────────────┘     └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │ Context      │
                    │ Extractor    │
                    │ - cwd        │
                    │ - last cmd   │
                    │ - exit code  │
                    │ - output     │
                    │ - git branch │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │ Agent        │
                    │ Harness      │
                    │ (Rig)        │
                    └──────────────┘
```

The agent always knows:

- What directory the user is in
- What command they just ran and its output
- Whether they're in SSH/tmux/docker
- Git status if in a repo

This is the current baseline. The next step is a runtime scope stack with evidence and confidence so the agent can distinguish shell metadata from the visible foreground runtime.

### Agent Tool Execution

When the agent needs to run a command:

1. Agent produces a `ShellExec` tool call via Rig
2. con-core creates a new (or reuses) terminal pane
3. Command is written to PTY
4. Output is captured via Ghostty `COMMAND_FINISHED`, with bounded recent-output fallback when shell integration is unavailable
5. Result is fed back to agent with exit code and duration metadata

This means the user **sees** what the agent does — no hidden subprocess. Full transparency.

### Compatibility with Existing Agents

For agent CLIs and other tools that run *inside* the terminal:

- con should detect them through the pane runtime tracker, using strong evidence first:
  - shell/runtime transitions
  - typed shell probes and reducer-backed action history
  - backend facts such as command lifecycle and future alternate-screen exports
- con must not promote pane-title or screen-structure heuristics into typed runtime state
- Provides enhanced UX: notification rings, focus management
- Does NOT try to "take over" — con is the host, not the agent
- Optional: pipe agent's stderr/notifications to con's notification system

This is a product boundary, not a convenience feature. con must support these tools without pretending that con itself owns the pane.

---

## Build & Development

### Prerequisites

```bash
# macOS
brew install rustup cmake
```

### Build

```bash
cargo build                    # debug build
cargo build --release          # release build
cargo run -p con               # run the terminal
cargo test --workspace         # test everything
```

### Build Pipeline

1. Cargo resolves workspace deps from upstream git sources and crates.io as declared in the workspace manifest
2. GPUI compiles Metal shaders at runtime (`runtime_shaders` feature — no Xcode.app needed for dev)
3. `cargo build` produces the `con` binary with all crates linked

---

## Config

```toml
# ~/.config/con/config.toml

[terminal]
font-family = "JetBrains Mono"
font-size = 14
theme = "catppuccin-mocha"        # or any ghostty theme
scrollback-lines = 10000
cursor-style = "block"

[agent]
provider = "anthropic"             # anthropic, openai, openai-compatible, deepseek,
                                   # groq, gemini, ollama, openrouter, mistral,
                                   # together, cohere, perplexity, xai
model = "claude-sonnet-4-0"        # leave empty for provider default
api_key_env = "ANTHROPIC_API_KEY"  # reads from env var
base_url = ""                      # optional: custom/proxy endpoint
max_tokens = 4096
max_turns = 10
auto_context = true                # inject terminal context automatically
auto_approve_tools = false         # require approval for shell_exec, file_write

[keybindings]
toggle-agent = "cmd+l"
command-palette = "cmd+shift+p"
new-tab = "cmd+t"
```

---

## Why This Stack Wins

1. **Ghostty's terminal correctness** — full Ghostty embedded via libghostty, Metal GPU rendering, complete VT compliance including Kitty keyboard and OSC 133 shell integration
2. **GPUI's rendering quality** — Zed proves it handles text beautifully at scale
3. **Rig's agent abstractions** — swap providers, define tools in Rust, stream tokens
4. **Rust's performance** — sub-millisecond input latency, <100MB RAM
5. **One runtime, one behavior model** — every pane uses Ghostty, which keeps terminal behavior and agent integration consistent
6. **Open source** — keep the product auditable, hackable, and durable

---

## Resolved Decisions

### 1. Terminal Backend: Full Ghostty

**Decision:** Embed full Ghostty via libghostty C API as the only terminal runtime.

**Rationale:**

- Ghostty provides production-grade VT compliance (Kitty keyboard, sixel, hyperlinks, OSC 133) without reimplementing it
- GPU-accelerated Metal rendering via native NSView — superior performance to software rasterization
- Action callback system gives us COMMAND_FINISHED (exit code + duration), eliminating blind timeouts for agent command execution
- The `con-ghostty` crate is a thin FFI wrapper (~800 lines), not a fork — upstream Ghostty updates flow through cleanly
- `TerminalPane` remains as a stable pane-facing API for the workspace and agent layers

### 2. GPUI IME: Production-Ready

GPUI implements the full `InputHandler` trait (modeled after `NSTextInputClient`):

- `marked_text_range()` / `replace_and_mark_text_in_range()` for IME composition
- `bounds_for_range()` for candidate window positioning
- GPUI has broader platform support, and con now ships macOS plus a
  Windows beta and a Linux preview; see `docs/impl/linux-port.md`.
- CJK input works. No blocker.

### 3. GPU Fallback: Not Needed

Ghostty has no software renderer — GPU-only. Same for con.

**Rationale:** A desktop terminal emulator always has a GPU. The scenarios where it doesn't:

- **Headless servers** — users SSH in, they use the remote machine's terminal, not con
- **Containers** — con doesn't run inside Docker; it runs on the host

If we ever need headless testing, we add a test-only software rasterizer. Not a user feature.

### 4. Plugin System: Node.js + Python via Sidecar IPC

**Decision:** Plugins run as external processes. con communicates via JSON-RPC over Unix domain sockets.

**Why Node.js + Python:**

- Largest developer ecosystems — lowest friction for plugin authors
- Runtimes installed by the user (system Node/Python, nvm, pyenv — their choice)
- No embedded runtime = no binary bloat, no version conflicts
- Security: plugins run in separate processes with explicit capability grants

**How it works:**

```
con (Rust)  ─── Unix socket (JSON-RPC) ───  plugin process (Node/Python/any)
```

- con exposes a socket API: `notification.create`, `terminal.write`, `context.get`, etc.
- Plugin manifest declares: name, runtime, entry point, requested capabilities
- con spawns the plugin process, passes socket path via env var
- Plugin SDK: thin npm package (`@con/sdk`) and pip package (`con-sdk`) wrapping the JSON-RPC protocol

**Phase 4 deliverable.** Socket API comes from the same external automation work introduced earlier in the product plan.

### 5. Licensing: MIT

- **GPUI**: Apache 2.0 (compatible with MIT, allows sublicensing)
- **Ghostty libghostty**: MIT
- **Rig**: MIT
- **con**: MIT (already in LICENSE)

All clear. No copyleft, no GPL contamination.

---

## Key Insight

The product should not depend on a built-in model to be useful. A socket control API for external agents keeps the system composable and lets built-in automation remain only one client of the platform.

**con should have both:**

1. **Built-in agent** (via Rig) — for users who want native AI without installing anything else
2. **Socket API** — for external agents and plugins to control con

The socket API is the foundation. The built-in agent is just the first client of that API.
