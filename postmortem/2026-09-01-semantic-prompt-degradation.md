# Semantic prompt loss delayed visible commands

## What happened

After Con began using Ghostty's semantic prompt state for visible command
completion, a shell that removed its OSC 133 hooks during a command could draw
an ordinary prompt without emitting another completion or prompt marker. Con
kept waiting for semantic confirmation until its polling deadline.

## Root cause

The per-command tracker remembered that semantic prompt state had been
observed, but represented every later `cursor_at_prompt = false` sample as the
same waiting state. It could not distinguish a running command from a finished
command whose shell integration had disappeared, so it also suppressed the
existing no-integration heuristic without reporting that completion had become
unprovable.

## Fix applied

Stable prompt-like output now remains advisory after semantic markers have
been observed. Instead of claiming completion or waiting for the full polling
deadline, Con returns an explicit unconfirmed result and preserves the recent
output. Agent, batch, remote, and control-plane callers treat that result as an
error and do not receive it as shell-ready. Poll exhaustion is handled the same
way. Authoritative command-finished events still take priority, normal semantic
prompt returns still complete, and panes that never exposed semantic markers
retain the existing fallback.

Con does not recover shell state from an unconfirmed result and does not append
sentinels or wrappers to the user's command.

## What we learned

- A sticky capability signal also needs an explicit degraded outcome.
- Prompt-shaped text cannot prove that an interactive shell is ready.
- When completion is unprovable, returning uncertainty is safer than either
  reporting success or silently waiting until a deadline.
