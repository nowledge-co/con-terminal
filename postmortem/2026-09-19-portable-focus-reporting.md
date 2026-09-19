# Portable terminals dropped focus reports

## What happened

Linux and Windows terminals never emitted DECSET 1004 focus reports. TUIs
that requested focus notifications could not observe pane or window focus
changes. macOS uses the full embedded Ghostty surface and was not affected
by this missing portable implementation.

## Root cause

Both portable backend `set_focus` methods were empty. The existing workspace
focus-state API also describes input-bar broadcast targets, not necessarily
the GPUI keyboard focus. Connecting that API directly would incorrectly report
multiple panes as focused and miss window deactivation and modal blur.

## Fix applied

Subscribe portable panes to GPUI focus, blur, and window activation. Combine
leaf focus with window activation, retain it until lazy session creation, and
keep broadcast targeting separate. The shared VT layer deduplicates transitions,
checks mode 1004, and uses Ghostty's focus encoder. Reports use the existing
reserved control capacity and ordering lock; enqueue failures desynchronize the
session instead of silently dropping state. Focus changes do not scroll the
viewport or advance user-input generation.

## What we learned

Host focus and command-routing scope are different contracts. Protocol mode
queries and reserving output order must share the parser lock. The regression
tests cover disabled-mode state tracking, duplicates, exact report bytes,
reserved-write failure, and reply ordering. Removing deduplication, mode gating,
or the ordering lock makes the corresponding test fail. The ordering test also
checks lock ownership explicitly: thread scheduling alone can hide a missing
lock.

Shared VT tests can run against the real pinned library on macOS, but this does
not validate Linux/Windows GPUI event delivery. Native acceptance must include
switching panes, opening a modal, moving to the input bar (including broadcast
scope), deactivating/reactivating the window, and focusing a lazily created pane
with a TUI that enables mode 1004.
