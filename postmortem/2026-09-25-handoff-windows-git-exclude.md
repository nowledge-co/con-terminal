# Handoff staging changed the Windows workspace snapshot

## What happened

Windows portable CI passed compilation but 23 handoff lifecycle tests failed with
`Workspace changed; send a new handoff` after staging a private export.

## Root cause

The export's path was serialized from a native `Path` into Git's
`info/exclude`. On Windows that produced backslashes. Git ignore patterns use
forward slashes on every platform, so Git still reported the staged export as
untracked. The workspace snapshot correctly detected the extra files, but they
were Con's own files rather than user edits.

## Fix

Build the Git pattern from path components joined with `/`, without changing
the native filesystem path. Assert the exact exclude pattern in the staging
test; keep the workspace-change guard intact.

## What we learned

Paths crossing into another tool's syntax need that tool's separators, not the
host OS separators. Portable compilation did not catch this behavioral failure;
the Windows test job did.
