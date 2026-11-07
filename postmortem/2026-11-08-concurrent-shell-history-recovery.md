# Concurrent pane startups reported history recovery as a failure

## What happened

After restoring a workspace with many panes, several zsh terminals printed
`Another shell is recovering history`, a Rust backtrace and con’s history
recovery warning. The companion CLI returned an error for ordinary contention
on the recovery lock. This failure occurs before any history append; the
message itself is not evidence that history was lost.

## Root cause

The beta.168 migration intentionally retains beta.167’s private history source
so still-open old shells can contribute later commands. Every new zsh therefore
calls the recovery helper while that source is nonempty. Recovery used an
immediate `try_lock()` and treated `WouldBlock` as an actionable error. Restoring
many panes starts those helpers concurrently. Even after migration finished,
helpers also took native history locks and read the target before discovering
from the cumulative checkpoint that there was nothing left to import.

## Fix

- Wait at most 250 ms per recovery/default-zsh lock, then quietly defer normal
  contention. Distinguish lock contention from actual I/O failures rather than
  swallowing errors or redirecting all stderr from the shell bridge.
- Honor native fcntl writer locks on source and target. Contention leaves data
  and the recovery checkpoint unchanged; a later terminal can retry.
- Check complete source records against the cumulative checkpoint before
  acquiring or reading the target. Keep source data and private backups; never
  truncate, replace, evaluate or automatically delete a user’s history.
- Preserve byte handling, multiline records, incomplete-tail deferral and
  append/checkpoint ordering. No user startup file, agent profile or installed
  application is modified by this change or its tests.

## Verification

Isolated tests force an already-held recovery lock with backtraces enabled,
start 16 real zsh shells together for both login and non-login startup, and
require quiet startup, original bytes plus one import, and exactly one backup
pair. Independent processes hold real fcntl locks on both history files;
deferred recovery must not write/checkpoint, and retry must import once.
Unit coverage also retains non-UTF-8/multiline recovery and trimming behavior,
checks the bounded busy path, and ensures a checkpointed source does not recreate
history a user removed. All fixtures use temporary HOME/history paths.
The complete macOS `con-cli` suite passed: 39 unit tests, one config import
test and nine native-session integration tests (49 total). Rust formatting,
diff checks and the documentation manifest validator also passed. This does
not claim Windows/Linux runtime verification or production delivery.

## Lessons

Contention is expected when a workspace restores many shells; it is not an
exceptional startup failure. Migration retention needs a cheap completed path
and a safe deferred path. Never silence every error to hide normal contention,
and never delete retained history or another process’s locks to make startup
look successful. A deferred zero-add result means “nothing imported now,” not
“migration complete”; only an actual checkpoint establishes completion.
