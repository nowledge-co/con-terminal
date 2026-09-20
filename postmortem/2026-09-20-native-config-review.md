# Native configuration refactor review

## What happened

Pre-commit review of the Ghostty configuration migration found three failure
paths: assigning a Con section could panic, transfer output could exceed the
input budget after rewriting resource paths, and standalone Settings theme
previews updated chrome without updating the native terminal.

## Root cause

The schema lookup accepted object nodes as scalar settings, invalidating the
insertion routine's object invariant. Transfer accounting measured source bytes
but not expanded destination paths. Standalone and embedded Settings dispatched
theme previews through different handlers.

## Fix applied

Reject section assignments before mutation; bound both source and rewritten
bytes, including intermediate theme values; route both Settings hosts through
the same theme-preview handler. Check native configuration before opening a
layout in a new window as well as ordinary window creation.

Regression tests exercise section prefixes and repeated cached resource
references with long output paths. Removing the corresponding guards in an
isolated worktree makes both regressions fail. That run also exposed timestamp
collisions between test temporary directories; an atomic sequence now isolates
parallel tests.

## What we learned

Bound transformed data, not just inputs. Validate schema leaves before relying
on internal insertion invariants. Share event handling across UI hosts, and
check every entry point that creates a native terminal.
