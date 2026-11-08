# Portable updates reused first-install scripts

## What happened

The Windows update action spawned a new PowerShell console and evaluated the
remote installer. Linux ran the shell installer in the background. Both exited
the application after a fixed delay, before download or verification completed.

## Root cause

First installation and updating an existing installation were treated as the
same lifecycle. Windows rediscovered latest instead of pinning the offered
version, killed same-named processes and removed the old directory. Linux pinned
the version and atomically replaced individual binaries, but hid failure and
did not relaunch. Neither path exposed a download/ready/restart state machine
or respected installation ownership explicitly.

## Fix

Use the existing signed feed and a shared Rust preparation/verification layer.
Require signed archive identity as well as Ed25519 verification. Download while
the app stays open, then hand off to a copied installed CLI only after explicit
restart. Save a pre-update snapshot, wait for live clients to exit, retain
backups, recover interrupted transactions and report failures. Externally
managed installations do not enter the self-update path.

## Lessons

A spawned installer is not an applied update. Readiness, installation ownership,
normal application shutdown, signature verification, recoverable replacement
and user-visible failure are separate contracts. Keep first-install scripts
separate from application updates. Directory-name prefixes alone must never
authorize cleanup or recovery; require an owned, validated plan. Never delete originals before a complete,
verified replacement and usable rollback exist.

## Verification

Local suites passed: 744 app tests, 49 CLI tests, 17 updater tests and 3 path
tests. The updater suite exercises both ZIP and tar extraction on every host,
real loopback HTTP staging, truncation, signature and identity errors, shared
locks, signed handoff, second-file rollback and interrupted recovery. Release
notes/publication/workflow suites passed 50 tests; docs manifest and actionlint
passed. Windows and Linux updater code and test targets cross-check successfully.

Native macOS fixtures reviewed Light/Dark available, downloading, ready,
failure and skipped states. Skip was clicked and its version persisted only in
a private test HOME. This does not certify native Windows/Linux UI or the actual
OS replacement lifecycle; those jobs are included in portable CI and still
need their platform runtime results before release.

The final manual-menu routing compiled and passed the affected Settings and
updater tests. Its additional native click-through could not run because the
computer-use native pipe failed to start; earlier visual-state checks above
remain valid. This final entry-point interaction remains a verification gap.

Follow-up native verification on Windows 11 Pro and Ubuntu 24.04 WSL2 passed
17 updater and 3 path tests on each host. The disposable-key subprocess fixture
also passed on both: an installed executable holds the live-instance lock,
replacement waits for normal exit, the helper replaces both files, and the
replacement executable actually relaunches. An unrelated preferences file is
retained and each temporary installation is removed. This closes the native
transaction gap; the full graphical Settings-to-update flow remains separate.
