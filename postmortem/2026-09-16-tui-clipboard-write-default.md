# TUI copies blocked by the clipboard-write default

## What happened

Amp reported a successful copy after transcript selection in Con, but the
system clipboard did not change. The same workflow worked in Ghostty.

## Root cause

PR #331 introduced `terminal.clipboard_write = false` by default and mapped it
to Ghostty's `clipboard-write = deny`. Ghostty defaults to allowing writes.
OSC 52 writes have no success acknowledgement, so a TUI's copy notification
does not prove that the terminal accepted the write. The reported Amp request
was not captured at runtime; the default-policy rejection is confirmed in code.

The copy-on-select fix in PR #345 covered terminal-owned selections, not
selections maintained by a mouse-reporting TUI.

## Fix applied

- Default application configuration and the macOS Ghostty config fallback to
  allowing clipboard writes.
- Preserve explicit `false`, including values saved by older Con versions.
  There is no reliable way to distinguish those from a deliberate opt-out.
- Retain plain-text validation, size limits, clipboard read behavior, and
  the independent user-gesture copy path. Keep uninitialized backend guards
  closed until the application supplies its configured policy.
- Cover missing configuration, a legacy terminal table without the setting,
  explicit opt-in/opt-out round trips, and generated Ghostty allow/deny limits.

## What we learned

A default that silently rejects a terminal protocol can break ordinary user
gestures inside TUIs. Terminal-owned and TUI-owned selections need separate
coverage. Allowing writes is a compatibility choice, not a claim that writes
are harmless: terminal output can replace clipboard contents. Users handling
untrusted output can still disable writes in Settings → General → Security.
