# Invalid configuration prevented Con from opening

## What happened

An older installed beta could not start after a newer development build had
written `con.experimental.handoff = true` to the shared `con.conf`. The older
binary rejected the unfamiliar key, printed an error to stderr, and exited
before opening any window. When the app was already running, the same class of
error made New Window fail with only a log entry. To someone opening Con from
Finder or the Dock, both cases could look like an unresponsive application.

This explains a reproducible startup failure, not the cause of the earlier
forced-quit hang. We do not have evidence that the hang itself came from the
configuration parser.

## Root cause

Strict validation was correct: silently discarding an unknown Con setting
could change behavior without the user's knowledge. The missing part was an
interactive recovery path. Configuration validation ran before GPUI startup,
and its error path terminated the process with exit code 2. The new-window
path returned `None` with no UI feedback.

Development and installed builds use the same user configuration directory,
so version skew can surface this failure even when neither file is corrupt.

## Fix

On invalid configuration, Con opens a small recovery window showing the source
file and validation error. It can open the file, copy details, check for an
update (or open the latest release), and retry validation after a correction.
Cold-start recovery relaunches the app only after validation succeeds. An
already running app can open a new window after retry. The recovery path never
starts a terminal session or resets the configuration. The error remains on
stderr if the UI itself cannot initialize.

## What we learned

- A strict parser needs a recoverable user-facing error, especially when
  installed and development binaries can read the same configuration.
- Recovery must preserve the authored file and explain version skew; falling
  back to defaults would hide the problem and risk overwriting user choices.
- This only helps versions that include the recovery UI. Older installed
  binaries still require a manual update or configuration edit.
