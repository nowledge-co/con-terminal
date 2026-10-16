# Release builds stalled downloading Zig

## What happened

On 2026-10-16, beta.136 macOS ARM and Intel release jobs remained in
Install Zig 0.16.0 for more than four hours. beta.137 also stopped at this
step. Rust compilation and Apple signing had not started. The corresponding
Linux release workflows also remained running and used the same download loop.
The maintainer requested cancellation of the remaining release jobs and a new
release after the fix. Existing tags and completed artifacts are preserved;
incomplete drafts must not be promoted manually.

## Root cause and evidence

The release download loops specified only curl's connection timeout. That does
not bound an established connection that stops transferring data. Repeated
attempts and mirror fallback cannot start until the current request returns.
There was no step-level deadline either. Portable/E2E CI already had total
transfer limits, but those duplicated scripts had diverged from release CI.

At investigation time, a bounded local request to the first mirror timed out
after 15 seconds without response bytes; the official source and another mirror
responded in about one second. GitHub reported Actions operational. Running-job
logs were unavailable, so the precise runner-side stalled request is not proven.
The missing transfer bound is confirmed directly in both tagged workflows.

## Fix

One shared macOS/Linux installer now bounds connection, total transfer and low
speed time, tries independent sources without repeated retries on a dead host,
and has a workflow installation deadline. It logs source changes and validates
pinned official checksums plus the installed version. Failed downloads are
removed, and PATH is updated only after successful validation. Windows runtime,
application code, signing, appcasts and publication gates are unchanged.

## Lessons

- Test release setup itself, not just application compilation.
- Keep portable CI and release bootstrap policy in the same implementation.
- A connection timeout is not a download deadline; every external transfer needs
  a bounded completion time and visible failure path.
- New commits cannot repair old immutable tags. Release a new tag from the
  corrected main branch rather than moving old tags or publishing partial drafts.

## Verification

The installer regression suite covers timeout and corrupt-content fallback,
exhausted sources, stale-file cleanup, early success and unsupported-target PATH
safety, extraction failures and wrong executable versions. The suite passed
locally, as did ShellCheck and actionlint for all four changed workflows. A real
native macOS ARM download, pinned checksum, extraction and executable version
smoke test passed against the official source. Linux and Intel execution remain
native CI coverage; this fix
does not cross-compile or claim local execution on those platforms.
