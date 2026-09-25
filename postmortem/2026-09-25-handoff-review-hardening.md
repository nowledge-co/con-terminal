# Handoff review hardening

## What happened

The handoff review at `546ef19c` found inherited credentials in subprocesses,
incomplete credential filtering, and all-target Clippy failures. Only
SUGGESTION-1, SUGGESTION-2 and SUGGESTION-3 were authorized for this fix.

## Root cause

`Command` inherits the parent environment by default, including for version/help,
history readers, model listing, Git snapshots and helper protocol discovery.
The line sanitizer lacked Slack/Stripe token prefixes, non-PEM private-key
assignments and compact JWTs. The usual lint command omitted test targets;
fixing its first errors exposed additional existing errors in downstream crates.

## Fix applied

- Clear subprocess environments and construct explicit probe, reader and launch
  allowlists. Only Cursor authenticated reads receive `CURSOR_API_KEY`; Codex
  local history retains `CODEX_HOME`. Preserve native config locations and, for
  authenticated Cursor reads, network/TLS settings. Interactive launch additionally
  retains terminal settings, SSH-agent access and named target-provider variables.
  The allowlists and their reasons live in `handoff/environment.rs`; no login,
  permission, version or source validation was removed.
- Extend whole-line credential filtering and test each added pattern, API-key
  spelling variants and normal logs/images/commit hashes.
- Use equivalent lint fixes and narrow test-only allowances instead of moving
  large existing test modules or changing UI-thread-only production types.

## What we learned

Run all-target Clippy through all dependency layers: the first batch of errors
is not necessarily the complete debt. Environment tests should poison a separate
parent process and exercise real capture/probe paths without modifying global
process state during parallel tests. Explicit environment allowlists intentionally
exclude arbitrary custom provider variables; supporting another native variable
requires an explicit, documented addition. Pattern filtering remains heuristic.
