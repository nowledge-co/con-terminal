# SSH disconnect left a dead terminal pane interactive

## What happened

When an SSH session disconnected abruptly, the terminal could remain on the old
screen and appear frozen. Keystrokes and control-plane writes targeted the same
pane, but there was no live child process or shell-ready state behind it.

## Root cause

The macOS workspace pump only drained Ghostty surface state after
`wake_generation` changed. That was correct for render work, but process exit is
a lifecycle event: if Ghostty did not emit a wake for a disconnect path, the
workspace did not call `is_alive()` and therefore did not emit
`GhosttyProcessExited`.

The command busy bit had a similar failure mode. `write_to_pty()` marked input
containing a newline as busy, and only the shell-integration
`COMMAND_FINISHED` action cleared it. Abrupt SSH disconnects can skip that
normal completion path, so command/control logic could keep treating the dead
pane as busy.

On Linux, the PTY reader treated EOF as a local thread exit only. The shared
session state was updated later only if another caller happened to poll child
status.

## Fix applied

- Child-exit actions now clear the busy bit, refresh the finished input
  generation, and mark the terminal as needing render.
- Linux PTY reader EOF and read errors now publish a session-exited state and
  wake the UI directly.
- The workspace pump drains lightweight surface state even when Ghostty's
  render wake generation has not changed, while still skipping native scroll
  synchronization on that fallback path.

## What we learned

Render wakeups are not a reliable carrier for lifecycle state. A terminal pane
must be able to observe child death independently of repaint scheduling, and
abnormal exits must clear optimistic shell-integration state because the normal
prompt/completion protocol is best-effort across SSH and PTY boundaries.
