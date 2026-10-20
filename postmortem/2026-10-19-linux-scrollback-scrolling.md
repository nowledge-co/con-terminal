# Linux terminal scrollback could not be scrolled

**Date**: 2026-10-19
**Issue**: #497

## What happened

On Linux, nothing scrolled a terminal pane back into its scrollback. A command
could print more lines than the pane holds, and the mouse wheel, a two-finger
touchpad scroll and Shift+PgUp all left the view on the last screen. There was
no scrollbar. The wheel inside `less` did nothing either. The reporter found
it on KDE Plasma under Wayland. An isolated-profile run of
`scripts/linux/scrollback-repro.sh` against `main` (18bd74f7) confirmed every
scrolling step failed.

## Root cause

It was never built for Linux, so no earlier change broke it. The `on_scroll_wheel`
handler in `crates/con-app/src/linux_view.rs` only turned wheel events into
mouse reports for the application. At a shell prompt no application asks for
mouse reports, so libghostty-vt's encoder wrote nothing and the event was
dropped. The Linux view drew no scrollbar, and Shift+PgUp was encoded and sent
to the shell like any other key. `docs/impl/linux-port.md` listed "local
scrollback wheel gestures and alternate-screen wheel-to-arrow fallback" as
missing work.

Windows already had all of it, through the same libghostty-vt `VtScreen` type
the Linux backend uses, but the pieces lived in Windows-only files
(`windows/host_view.rs`, `windows_view.rs`). macOS gets the same behaviour from
full libghostty.

## Fix applied

- The wheel accumulator and cursor-key helpers moved from the Windows host view
  to `crates/con-ghostty/src/scroll.rs`, shared by both backends.
- `LinuxPtySession` gained viewport scrolling, offset jumps, scrollbar state and
  the alternate-screen cursor-key fallback. Input that returns the view to the
  prompt also drops any partial row of wheel movement.
- The Linux view routes the wheel by the application's mouse-tracking mode, as
  Windows does. Shift always scrolls Con's view. Deltas use the painted logical
  row height, following the #486 lesson about fractional scales.
- Shift+PgUp/PgDn page the primary screen's scrollback without writing to the
  terminal. On the alternate screen they still go to the application.
- The Linux pane draws the Windows overlay thumb, with drag and track-click
  paging. The thumb geometry lives in `crates/con-app/src/terminal_scrollbar.rs`.

## What we learned

A feature that one portable backend has and the other lacks is easy to miss
when it sits in platform-named files. Here the code depended only on the shared
`VtScreen`, so moving it to a shared module was enough to reuse it. Before
adding Windows or Linux input behaviour, check whether the other backend's view
already has it.

The first version of the Shift+wheel path read only the vertical delta, and
its test built a vertical Shift+wheel event. GPUI never sends one on Linux: its
X11 and Wayland backends move a Shift+wheel onto the horizontal axis. The review
of the branch caught it. Tests for input handling should build events the way
the platform layer does, so check GPUI's backend before writing one.

`con-cli panes read` returns the transcript tail, not the rows on screen, so the
control socket cannot check the scroll position. The repro script asks the
tester for the lowest visible row instead. The automated checks are view tests
that drive a real pseudo-terminal (PTY).

Isolating a Con profile takes more than the `XDG_*` variables. At start-up
`inherit_shell_env` in `crates/con-app/src/main.rs` runs `$SHELL -l -c env` and
lets the login shell override Con's environment. The repro script points
`SHELL` at a stub for that reason.
