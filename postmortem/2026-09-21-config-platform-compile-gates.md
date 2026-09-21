# Native configuration platform compile gates

## What happened

PR #373 failed Linux and Windows CI under `-D warnings`. Linux reported unused
native import state, startup mutability, and a theme-preview parameter. Windows
stopped earlier on the Unix-only mutation of the import directory builder.

## Root cause

Declarations were shared across targets while their consumers were compiled only
on macOS or Unix. macOS-only testing did not exercise the portable compile paths.
The same review also found portable-only preview methods compiled unused on macOS.

## Fix applied

Keep import preparation and native validation in one macOS block; unsupported
platforms return an error before creating snapshots. Return native startup values
through immutable shadow bindings. Scope directory-builder mutation to Unix,
retaining its `0700` mode, and compile portable preview methods only where used.
Destructure the preview event so each platform can consume the input it needs.

## What we learned

Cross-platform warning failures need a review of both sides of each `cfg`, not
lint suppression or removal of permissions and validation. The existing CI matrix
is the compilation regression guard. The import test now also asserts directory
permissions so a portability fix cannot silently weaken snapshot privacy.
