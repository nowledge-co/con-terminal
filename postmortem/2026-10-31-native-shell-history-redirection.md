# Native shell bridge redirected zsh command history

## What happened

After upgrading to beta.167, a user found that earlier shell commands were no
longer available in con. Their original `~/.zsh_history` remained intact, but
new commands were saved in con's private `native-session/runtime/zsh` directory.
The user-specific recovery backed up both files and appended the 18 distinct
recent history records to the original without changing its existing bytes.

## Root cause

The session-local bridge temporarily selects a con-owned ZDOTDIR to chain zsh's
startup stages. macOS runs `/etc/zshrc` before the selected `.zshrc`; that system
file assigns `HISTFILE=${ZDOTDIR:-$HOME}/.zsh_history`. Restoring ZDOTDIR at the
start of con's `.zshrc` was too late to undo the derived history path. Oh My Zsh
retains an already-set HISTFILE, so it did not correct that system default.

Earlier verification checked startup order, final ZDOTDIR and unchanged user
configuration files, but did not check command history reads and writes.

## Fix

Before sourcing the user `.zshrc`, translate only the system default pointing
at con's private bridge back to the effective user ZDOTDIR or HOME. Other
history paths are untouched, and the user's `.zshrc` can still override it.
No user shell startup file or native agent profile is edited. The old bridge
history is retained rather than deleted.

For affected installations, a con-owned startup helper automatically appends
missing records to the shell's final HISTFILE after user startup overrides.
It operates on bytes, preserving multiline commands and non-UTF-8 history;
it never evaluates history or truncates the target. Private source/target
backups precede each append. A cumulative checkpoint prevents duplicate imports
after restart or native history trimming. Both zsh's default `.LOCK` protocol
and its optional fcntl locks are honored; contention retains the source and
reports a retry rather than overwriting a live writer. Incomplete trailing
records remain in the source for a later retry.

The recovery bridge also runs when affected users disabled agent auto-resume
while troubleshooting. Their auto-resume preference stays off and automatic
agent hooks remain disabled.

## Verification and lessons

The regression test fails on the original bridge with the private HISTFILE.
It covers login/non-login shells, custom ZDOTDIR, explicit user history paths,
reading an existing command and appending a new command without losing the old
one. It also checks that no history file appears in the bridge directory.
Additional tests exercise automatic startup recovery, repeated shell launches,
byte-preserving backups, deduplication and subsequent writes from old shells.

Temporary environment changes can alter system startup defaults before user
startup files run. Validate derived behavior, especially durable user data,
instead of treating restored environment values as sufficient isolation.
