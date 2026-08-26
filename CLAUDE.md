# con — Development Guide

## What is con?

con is an open-source, GPU-accelerated terminal emulator with a built-in AI agent harness. Built in Rust.

- **macOS** — primary target, shipped. Metal-backed libghostty, signed DMG, Sparkle auto-update.
- **Windows** — first beta shipped as `v0.1.0-beta.25`. D3D11/DirectWrite renderer over ConPTY + libghostty-vt, unsigned ZIP distribution, notify-only update checker. Plan + open work: `docs/impl/windows-port.md`; status tracker: issue #34.
- **Linux** — preview. Real PTY pane via `libghostty-vt`, styled-cell paint (SGR colors / bold / italic / underline / inverse + block cursor), client-side decorations with the same caption cluster Windows uses, transparent ARGB window with rounded corners, and KWin-Wayland backdrop blur via `org_kde_kwin_blur` where the compositor exposes it. Plan + open work: `docs/impl/linux-port.md`; tracker: issue #18.

## Stack

- **UI**: upstream Zed GPUI (git dependency on `zed-industries/zed`, Apache 2.0). Windows backend is D3D11/DirectComposition; HWND child-embedding is the known gap for the Windows port.
- **Terminal runtime**: libghostty — full Ghostty terminal via C API, Metal GPU rendering, embedded as native NSView. macOS uses the full embedded libghostty; Windows and Linux consume the carved-out `libghostty-vt` parser instead and pair it with their own renderers (D3D11/DirectWrite on Windows, GPUI per-row `StyledText` on the Linux preview today / GPUI-owned glyph-atlas grid renderer in the long term).
- **Terminal FFI**: con-ghostty crate — thin Rust wrapper over libghostty C API on macOS (surface lifecycle, action callbacks, clipboard, key/mouse input). On Windows + Linux it wraps `libghostty-vt` plus per-platform PTY (`ConPTY` / Unix PTY) and renderer plumbing. Per-platform code lives in `con-ghostty/src/{terminal,windows,linux}/`; the workspace consumes the same `GhosttyApp` / `GhosttyTerminal` / `TerminalColors` type names from each.
- **Terminal support crate**: con-terminal — theme and palette helpers only
- **AI agent**: Rig v0.40.0 (from crates.io, multi-provider clients, Tool and AgentHook traits)
- **Socket API**: JSON-RPC 2.0 with a platform-specific transport — Unix domain sockets on Unix, Windows Named Pipes (`\\.\pipe\con`) on Windows. Served by the app and consumed first by `con-cli`.

## Repository Layout

```
kingston/
├── DESIGN.md          # Architecture and design decisions
├── CLAUDE.md          # This file — development guide
├── docs/
│   ├── impl/          # Implementation notes per crate/subsystem
│   └── study/         # Research notes on 3pp dependencies
├── postmortem/        # Issue postmortems (YYYY-MM-DD-title.md)
├── crates/
│   ├── con/           # Main binary (GPUI app shell)
│   ├── con-core/      # Shared logic (harness, config, session)
│   ├── con-terminal/  # Terminal themes and palette helpers
│   ├── con-ghostty/   # Ghostty FFI wrapper — primary macOS backend (libghostty C API)
│   ├── con-agent/     # AI harness (Rig 0.40, tools, conversation)
│   └── con-cli/       # CLI + socket client for the live local control plane
├── assets/            # Themes, fonts, icons
└── 3pp/               # Third-party source (READ-ONLY reference, .gitignored)
```

## 3pp Policy

The `3pp/` directory contains third-party source checkouts for **read-only reference only**. It is `.gitignored` — never modify, commit, or depend on files in `3pp/`.

- All third-party dependencies come from **crates.io** (or git URLs in Cargo.toml).
- If a 3pp library has a bug, **upstream the fix** to the library's GitHub repo. Do not patch locally.
- `3pp/` exists solely so you can read and study how dependencies work internally.

## Build

```bash
# Prerequisites: rust (stable, edition 2024), cmake, Zig 0.16.0 exactly (for libghostty / libghostty-vt)
cargo build            # debug (macOS)
cargo build --release  # release (macOS)
cargo run -p con       # run the terminal (macOS)
cargo test --workspace # test
```

The `con` UI binary builds on macOS, Linux, and Windows. macOS uses
the embedded full libghostty + Metal renderer; Windows ships a
ConPTY + libghostty-vt + D3D11/DirectWrite renderer; Linux ships a
Unix PTY + libghostty-vt + GPUI-owned `StyledText` paint path. The
agent panel, settings, command palette, and control socket
(`\\.\pipe\con` on Windows, `/tmp/con.sock` on Unix) are fully
wired on every platform. See `docs/impl/windows-port.md` and
`docs/impl/linux-port.md` for the per-platform porting plans and
the path to the long-term GPU-accelerated grid renderer on each
non-macOS target.

```bash
# Windows (from a Developer Command Prompt for VS 2022; needs Zig 0.16.0 exactly on PATH
# for libghostty-vt; the binary ships as `con-app.exe` because `CON` is a
# reserved DOS device name):
cargo wbuild -p con --release          # produces target\release\con-app.exe
cargo wrun   -p con
cargo wtest  -p con-core -p con-cli -p con-agent -p con-terminal -p con-ghostty

# Linux (needs the GPUI linux runtime deps — see .github/workflows/ci-portable.yml):
cargo build -p con --release
```

The `w*` aliases (declared in `.cargo/config.toml`) wrap the
`--no-default-features --features con/bin-con-app` incantation the
Windows-named binary requires.

## Control Plane

- `con-cli` is a real client for Con's local control socket, not a stub.
- Implementation details live in `docs/impl/socket-api.md`.
- The current live E2E workflow lives in `docs/impl/con-cli-e2e.md` and `docs/impl/con-test.md`.

## Local Skills

- `skills/con-cli-e2e/SKILL.md` — use when validating the control plane from
  an external agent, writing eval automation against a real running Con session,
  or writing/fixing integration tests in `crates/con-test/testdata/`. Covers
  both manual `con-cli` E2E and the `con-test` runner (test file format,
  assertion types, common failure patterns).
- `skills/gpui-cache-aware/SKILL.md` — use when reviewing or changing UI
  performance, especially markdown/chat rendering, terminal-adjacent UI, or
  resize/animation paths.
- `skills/changelog-release-notes/SKILL.md` — use before editing
  `CHANGELOG.md`, release notes, or PR descriptions that summarize
  release-visible work. Always check the latest shipped beta and add PR +
  GitHub author credit for every PR-derived changelog item.

## Design Language

- **Font**: IoskeleyMono (embedded, all weights, Nerd-Font patched) for terminal chrome — tabs, sidebar, input bar. Patched TTFs carry ~10,400 PUA glyphs (Powerline, devicons, Font Awesome, Octicons, Codicons, Material Design) so oh-my-posh / Starship / Powerlevel10k prompts render out of the box. System font (`.SystemUIFont` GPUI virtual alias / SF Pro on macOS) for AI panel prose text, settings panel, and any non-terminal UI. Code blocks and terminal previews use IoskeleyMono via `mono_font.family` in theme JSON. Dot-prefixed GPUI virtual font aliases are valid for GPUI UI text only; terminal backends must receive concrete font families and sanitize pseudo families to the default terminal font.
- **Default theme**: Flexoki Light. Dark available as Flexoki Dark.
- **Icons**: Phosphor Icons only (phosphoricons.com). Copy SVGs from `3pp/phosphor-icons/SVGs/regular/` into `assets/icons/phosphor/`. Never draw icons manually — always use the Phosphor library. Reference as `"phosphor/icon-name.svg"` in code. **Critical**: always set `.text_color()` directly on every `svg()` element — parent container color does NOT propagate to SVG stroke colors in GPUI.
- **Borderless**: No `border_1()`, `border_r_1()`, etc. Use opacity-based fills for surface separation.
- **Shadowless**: No `shadow_sm()`, `shadow_lg()`, etc. Use bg opacity for elevation.
- **Color by meaning only**: Monochrome surfaces by default. Accent color for semantic states (focus, active, warning, error).
- **Typography as hierarchy**: Size, weight, and opacity create structure — not boxes and borders.

See `docs/design/con-design-language.md` for full design system.

## UI/UX Principles

These principles govern all UI iteration. Follow them proactively — don't wait for the user to point out violations.

### Use gpui-component library first

Before building custom UI, check `3pp/gpui-component/` for an existing component. The library has 60+ components including:

- **Select** (`select::Select`, `SelectState<SearchableVec<String>>`) — searchable dropdowns. Use for any list selection instead of hand-rolling clickable divs.
- **Button** (`button::Button`) — use `.ghost()`, `.primary()`, `.small()`, `.icon()` variants. Never hand-roll clickable divs for buttons.
- **Input** (`input::Input`, `InputState`) — text fields with `.appearance(false)` for inline, `.cleanable()`, `.placeholder()`.
- **Switch** (`switch::Switch`) — toggles. Never hand-roll toggle divs.
- **Icon** (`Icon::default().path("phosphor/name.svg")`) — use with Button via `.icon()`.
- **Clipboard** (`clipboard::Clipboard`) — copy-to-clipboard with auto check-icon feedback.
- **Sidebar** (`sidebar::Sidebar`) — collapsible sidebar with icon-only mode.
- **Settings** (`setting::SettingPage`) — native settings layouts.

Read the component's source in `3pp/gpui-component/crates/ui/src/` to understand its API before using it. The `CLAUDE.md` in `3pp/gpui-component/` documents the full architecture.

### Visual normalization

- **Rounding consistency**: If one surface is flat (e.g., embedded terminal NSView), adjacent surfaces should also be flat. Don't mix rounded bubbles next to sharp-edged content.
- **Uniform widths**: When multiple panels/cards share a container and the user switches between them, use the same width for all to prevent layout jumping.
- **Icons over text labels** for mode indicators and compact controls. Text labels are for headings and settings fields.
- **Font context-switching**: Use mono font (Ioskeley Mono) for shell/command contexts. Use system font (.SystemUIFont) for natural-language/agent contexts and settings UI.

### Focus behavior

- After submitting input from the input bar, keep focus on the input bar — the user is in a "command flow" and will likely type more. They can click the terminal to focus it.
- Modal dialogs (settings, command palette) capture and return focus cleanly.
- Use `FocusInput` action (keybinding) as the explicit "focus the input bar" gesture.

### Density and wording

- Prefer compact, terse UI labels. "Browse Themes" not "Open Theme Catalog". "Load from Clipboard" not "Load Clipboard Theme".
- Remove anything the user can derive from context (e.g., don't show CWD when it's visible in the terminal).
- Placeholders should be action-oriented and short: "Run a command…", "Ask anything…".
- Avoid numbered step wizards for simple 2-3 step flows — inline everything flat.

## Key Conventions

- **Crate boundaries matter.** con-terminal has zero UI deps. con-agent has zero terminal deps. con-core glues them.
- **Real Rig integration.** Tools implement `rig::tool::Tool` trait. Agent built via `client.agent(model).tool(T).build()`. Chat via `Chat::chat()` trait.
- **Agent transparency.** When the built-in agent runs a command, it executes visibly. No hidden subprocesses.
- **Shared tokio runtime.** The harness owns a single multi-thread tokio runtime — no thread-per-message.
- **Config is TOML.** User config resolved at runtime via `con-paths::config_file()` → `dirs::config_dir()`:
  - **macOS**: `~/Library/Application Support/con/config.toml` (fallback: `~/.config/con/config.toml` if `dirs::config_dir()` returns None — effectively never on macOS)
  - **Linux**: `$XDG_CONFIG_HOME/con/config.toml` (defaults to `~/.config/con/config.toml` when `XDG_CONFIG_HOME` is unset)
  - **Windows**: `%APPDATA%\con-terminal\config.toml` (fallback: `~/.config/con-terminal/config.toml`)
- **GPUI patterns.** Use `cx.spawn(async move |this, cx| { ... })` for async work. Use if/else for conditional UI (FluentBuilder::when() is not re-exported).

## Branching

- `main` — stable
- Feature branches: `wey-gu/<short-name>`

## Testing Strategy

After implementing a PR, write tests following this priority order:

- **Unit tests first** — test functions and logic directly in the same crate using `#[cfg(test)]` modules. Cover non-trivial logic, edge cases, and error paths.
- **con-test for integration** — use `con-test` only for integration and interactive behavior that requires a running session (e.g., pane lifecycle, socket API, agent interactions).
- **No low-value tests** — don't write tests just to hit coverage. Skip tests for trivial getters, one-liner wrappers, or behavior already covered by the type system. Every test should catch a real bug or document a real contract.

## Postmortems

When solving a non-trivial bug or issue, create `postmortem/YYYY-MM-DD-title.md` with:
- What happened
- Root cause
- Fix applied
- What we learned
