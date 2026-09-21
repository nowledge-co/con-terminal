# Ghostty import theme discovery

## What happened

Review of Settings import found that a custom Ghostty theme could be left as a
name instead of copied into the independent snapshot on macOS.

## Root cause

Configuration discovery honored `XDG_CONFIG_HOME`, but theme discovery used
`dirs::config_dir()`, which returns Application Support on macOS, followed by a
fixed `~/.config` fallback. Neither matched a custom XDG directory.

## Fix applied

Configuration and theme discovery share Ghostty's XDG resolution. Import still
copies themes before publishing the destination. A CLI integration test sets the
child process's XDG directory, imports a named custom theme, removes its source,
and reads the snapshotted colors. It fails on the old macOS implementation.

## What we learned

Sharing a configuration syntax does not mean sharing platform directory helpers.
Match upstream resource lookup, and test environment-dependent behavior in an
isolated child process rather than changing the test runner's environment.
