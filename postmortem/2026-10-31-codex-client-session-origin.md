# Codex shared-daemon hooks lost terminal origin

## What happened

The initial native-session integration required users to launch interactive
Codex with `--no-daemon`. Ordinary `codex` could reuse a shared backend, so its
SessionStart hook could not reliably link the current con surface. Requiring an
extra flag on every first launch made a baseline continuity feature fragile.

## Root cause

Codex 0.166 captures hook environment in the backend. A persistent daemon does
not receive each TUI client's terminal-local con origin/socket. Native IDs from
the hook are exact, but the inherited origin can be absent or belong to another
client. Directory, recency and process-name matching cannot repair that loss.
Herdr's equivalent hook uses inherited environment and has the same boundary.

## Fix

With the maintainer's explicit acceptance, one-time Codex integration installs
a marked shell block and con-only launcher. Normal local interactive `codex`
uses native terminal-independent execution automatically. Noninteractive and
explicit remote commands, nested tools and profiles without the integration
pass through. Other terminals retain their native daemon behavior. The launcher
uses exec and preserves arguments; the installer does not approve native hooks.

Each profile owns its marked shell block. Preflight validates native and shell
configuration before writes, private backups are retained and failures roll
back native settings. Existing aliases/functions are preserved. An explicit
launcher-directory argument prevents recursion when application and terminal
home/data directories differ.

The durable replacement is a native TUI client attachment callback, documented
in `docs/design/native-agent-client-link.md`. Until an upstream release provides
that contract, con must not claim daemon-compatible automatic association.

## Lessons

- A hook's exact session ID does not prove the terminal that displayed it.
- Decide client versus server ownership before relying on environment metadata.
- Treat ordinary first launch as an acceptance case, alongside restart recovery.
- Test the installed entry point in a real terminal, including different home
  directories, profile isolation and a pre-existing daemon.
- Separate protocol fixtures, actual CLI behavior and paid-model verification.

## Verification

Core/CLI unit tests, real-PTY installer/launcher tests and the macOS application
suite cover the affected paths. An isolated con window ran actual Codex 0.166.0
with GPT-5.6 Luna and Claude Code 2.1.314 with Haiku 5.5. Each remembered a marker,
automatically resumed after con restarted and recalled the correct marker with
the same native ID and a fresh terminal origin. Four short model turns, no model
tool calls. Host reboot and Windows/Linux execution were not exercised.
