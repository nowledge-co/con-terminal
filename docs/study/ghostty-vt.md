# Study: Ghostty Integration

## Decision: Full Ghostty via libghostty C API

We initially planned to use **libghostty-vt** — the standalone VT parser library extracted from Ghostty — and build our own Grid, renderer, and scrollback on top. After evaluation, we chose to embed **full Ghostty** instead via its libghostty C API.

### Why Full Ghostty Over libghostty-vt

| Consideration | libghostty-vt (parser only) | libghostty (full terminal) |
|---|---|---|
| VT compliance | Parser actions only — we build Grid, screen state, scrollback | Complete: parser + screen + scrollback + renderer |
| Rendering | Our own GPUI canvas rendering | Metal GPU rendering via NSView |
| Shell integration | We implement OSC 133 handling | Built-in: COMMAND_FINISHED with exit code + duration |
| Clipboard | We implement via PTY + pasteboard | Built-in: read/write callbacks |
| Maintenance | We maintain ~3000 lines of Grid + rendering | ~800 line FFI wrapper |
| Build | Zig toolchain + bindgen | Pre-built libghostty, Rust FFI only |

**The deciding factor:** COMMAND_FINISHED. Ghostty fires this action when OSC 133;D arrives, providing the exit code and nanosecond-precision duration of the completed command. This single capability eliminates the 3-second blind timeout in `terminal_exec`, making agent command execution instant and reliable. Building this from libghostty-vt would have required reimplementing Ghostty's shell integration handling.

### Architecture

```
con process
├── GhosttyApp (singleton)
│   └── ghostty_app_t — runtime config, font discovery
│
├── GhosttyTerminal (per pane)
│   ├── ghostty_surface_t — owns PTY, parser, screen, renderer
│   ├── NSView — Metal rendering surface, composited in GPUI window
│   ├── TerminalState — action-driven state (title, pwd, exit code, busy)
│   └── Callbacks:
│       ├── action_callback — 66 actions (7 handled, 59 acknowledged)
│       ├── read_clipboard_callback — paste from NSPasteboard
│       ├── write_clipboard_callback — copy to NSPasteboard
│       ├── close_surface_callback — child process exited
│       └── size_report_callback — terminal dimensions
│
└── GhosttyView (GPUI Element)
    ├── Forwards key/mouse events to ghostty_surface_key/mouse
    ├── Manages NSView lifecycle (add/remove from window)
    └── Handles focus tracking
```

### What libghostty-vt Would Have Given Us

(Preserved for reference — this was our original plan.)

| Component | What it does | C API header |
|-----------|-------------|--------------|
| **Parser** | DEC ANSI state machine. States: ground, escape, CSI, DCS, OSC. | via Zig module |
| **OSC parser** | Window title, hyperlinks (OSC 8), semantic prompts (OSC 133) | `vt/osc.h` |
| **SGR parser** | Text attributes: bold, italic, underline, 256-color, RGB | `vt/sgr.h` |
| **Key encoder** | Kitty keyboard protocol, xterm/VT102 escape sequences | `vt/key.h` |
| **Paste validator** | Bracketed paste safety checks | `vt/paste.h` |

It does **not** provide: Screen/Grid state, rendering, PTY management, or scrollback. All of those would have been our responsibility.

### Current Action Coverage

7 of 66 ghostty actions are handled with specific logic. The remaining 59 return `true` (acknowledged) to prevent ghostty from logging unhandled-action warnings.

**Handled:**
- `SET_TITLE` — updates terminal tab title
- `PWD` — updates working directory (OSC 7)
- `RENDER` — signals frame rendered, sets `needs_render`
- `COMMAND_FINISHED` — captures exit code + duration, clears busy state, records history
- `SHOW_CHILD_EXITED` — marks terminal as exited
- `COLOR_CHANGE` — triggers re-render
- `RING_BELL` — plays system beep via `NSBeep()`

**Not yet handled (future work):**
- `OPEN_URL` — hyperlink clicks
- `START_SEARCH` / `END_SEARCH` — native terminal search
- `MOUSE_SHAPE` — cursor shape changes
- `DESKTOP_NOTIFICATION` — system notifications from terminal apps
- `SET_MOUSE_VISIBILITY` — show/hide cursor during typing

### Observability Limits For Pane Intelligence

Ghostty is excellent at terminal behavior, but the current embedded C API is not yet a full pane-runtime introspection API.

What the C API gives us well:

- surface config inputs such as `font_size` and `working_directory`
- live app and surface config updates via `ghostty_app_update_config` / `ghostty_surface_update_config`
- native binding actions such as `clear_screen`
- local foreground process-group ID via `ghostty_surface_foreground_pid`
- local PTY slave name via `ghostty_surface_tty_name`
- `SET_TITLE`
- `PWD`
- `COMMAND_FINISHED`
- visible and scrollback text via `ghostty_surface_read_text`
- selection access
- inspector lifecycle access
- process-exited state

What Ghostty clearly tracks internally in Zig:

- semantic prompt state
- PTY and process-group ownership
- prompt/input/output boundaries
- prompt click movement
- richer screen semantics around command output

What the embedded API does **not** currently give us as a stable product contract:

- the exact foreground program identity
- a nested scope stack such as `ssh -> tmux -> agent CLI`
- a direct export of Ghostty's richer semantic prompt model for host applications

This matters for con:

- Ghostty should be treated as a strong source of terminal facts
- con still needs its own pane runtime observer
- the foreground process-group ID must not be presented as an exact process identity
- we should not design external-agent or tmux awareness around assumptions that Ghostty will directly tell us the whole runtime state

These process and TTY functions belong to the full embedded surface API used on macOS. `libghostty-vt` does not own a PTY, so Windows and Linux must obtain equivalent platform facts from ConPTY or the Unix PTY owner when available.

One more important limit: Ghostty's OSC 7 handling validates host information against the local system when reporting `PWD`. That means `PWD` is not a durable embedded signal for remote host identity in the way a naive reader might expect.

## Ghostty Is Not A Full Terminal Control Plane

For con's product goal, the important conclusion is:

Ghostty is one layer in the stack, not the whole stack.

The current embedded C API is a strong host-embedding API.
The VT API is a strong emulator-state API.
Neither is a universal runtime-control protocol analogous to browser CDP.

The durable model for con is therefore:

- Ghostty for emulator truth
- shell integration for shell truth
- tmux control mode for tmux truth
- app-native RPC for app truth
- OS and PTY inspection for process truth

See `docs/study/terminal-control-plane.md`.
