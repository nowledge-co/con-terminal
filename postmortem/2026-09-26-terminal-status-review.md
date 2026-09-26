# Terminal status review corrections

## What happened

Review of the terminal-status branch found that ordinary injected shell commands
could animate indefinitely, versioned native Claude executables were missed,
title spinner updates performed whole-workspace collection, and unrelated child
process churn reopened screen scanning. Completed or cancelled agent requests
could leave stale approval cards and NeedsInput status. Linux host bridge
selection also accidentally required a new optional capability.

## Root cause

Presentation consumed control-plane busy bookkeeping rather than explicit
activity evidence. Executable matching assumed the resolved basename was the
command name. Collection and title observation shared an entry point, and scan
invalidation compared complete process lists rather than agent identity.
Approval UI had no request-end cleanup event. Bridge capability probing coupled
terminal I/O support to optional metadata support; its Linux test assumed a
host without /proc.

## Fix applied

- Removed shell-write busy evidence from presentation without changing execution
  or shell-prompt tracking. Recognize the native Claude version-directory layout.
- Update title evidence only on the originating surface. Preserve scan budgets
  across unrelated child churn and retain screen identity during bounded retries.
- Retire approvals using per-request channel identity, including out-of-order
  request completion; explicitly deny approvals on Stop. Observe panel activity
  changes without refreshing workspace chrome for every panel notification.
- Invalidate cached agent-panel rendering only when its presentation inputs change.
- Preserve older literal-command-capable host bridges, advertise actual host
  observation limits, and drain oversized optional metadata without allocating
  its advertised length. Correct the Linux metadata assertion.
- Timestamp accepted host observations at receipt, not request dispatch.

## What we learned

An OS process name is not necessarily the command invoked by the user. A PTY
write is not an activity protocol. Animation events must not be collection
events, and concurrent requests need real ownership keys rather than inferred
FIFO order. Platform-specific tests must run on their named platform before
claiming runtime coverage; cross-compilation only checks types.
