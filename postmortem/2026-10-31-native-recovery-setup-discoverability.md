# Native recovery setup was not discoverable

## What happened

After launching Claude Code and Codex normally, a user quit and reopened con and
expected their conversations to reopen. No conversations resumed. The installed
native profiles had no con session hooks, automatic recovery was not enabled,
and the saved layout contained no native conversation references.

## Root cause

Exact native recovery requires two independent opt-ins: installing the native
integration and enabling automatic resume. The implementation and isolated
restart tests covered the configured path, but the product exposed integration
only through CLI documentation. A Settings toggle mentioning a prerequisite
without providing the action made a working recovery engine appear broken.
Terminal text replay and agent identity icons also did not imply a saved native
session, a distinction the ordinary launch flow failed to make discoverable.

## Fix

Native recovery now defaults on, while explicit off preferences remain off.
con prepares private shell bridges and launchers in its own data directory. They
chain the user startup files and shipped Ghostty integration. Normal Claude Code
and Codex launches receive session-only hook declarations through native CLI
facilities; no native profile, settings file, credential, or shell rc file is
changed. Native policy and trust remain native. Batch commands and remote
clients retain their behavior. Additional native adapters retain their explicit
hook setup until an equivalent session-local injection contract is verified.

Continuity settings keep the off switch and an actionable retry for con-owned
integration preparation failures.
Horizontal tabs also use the same live detail card as vertical tabs, including
native link and recovery diagnostics, instead of a separate plain-text tooltip.

## Lessons

- Validate the unconfigured user journey as well as configured restart recovery.
- Expected continuity should be the default; configuration should offer an exit,
  rather than force users to discover prerequisites before ordinary use works.
- Prepare launch policy before terminal creation, then resolve native profiles
  after the user's shell environment is established.
- Keep identity authority explicit: process names and icons cannot establish a
  native link, and recovery must never choose a recent session by guess.
- Reuse live tab detail presentation across orientations to avoid divergent UX.

- Default-on integration must stay launch-scoped; convenience does not authorize
  rewriting the user’s native agent or shell configuration.
