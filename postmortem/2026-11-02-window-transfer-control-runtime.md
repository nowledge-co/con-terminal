# A transferred control listener must retain its runtime

## What happened

During isolated verification of session drag transfers, native agent reports
successfully reached a moved terminal through the original window's socket.
Closing that original window then left the socket path present but connections
were refused. The terminal processes and their new window remained alive.
This was found before shipping the drag-transfer feature.

## Root cause

`ControlSocketHandle` owned only the endpoint path. Its listener task ran on the
originating workspace harness's Tokio runtime. Moving the handle and request
receiver did not retain that runtime; dropping the original harness stopped the
listener even though its apparent owner survived.

A second ownership detail affects persistence: a temporary replacement tab in a
source window that is about to disappear must not overwrite the surviving
window's session snapshot.

## Fix

The control handle retains the existing runtime and task cancellation authority.
Dropping the final handle aborts its listener and removes the Unix endpoint.
When the source window closes, the destination receives the handle and receiver;
its session is flushed after the source's final snapshot. A last-tab transfer
flushes pending source saves before publishing the destination snapshot, and
does not queue a later save of the temporary replacement tab.

## Verification

A real Unix socket regression test drops the original runtime owner, exchanges
an identify request through the retained handle, then checks runtime and socket
cleanup when the transferred owner is dropped. Native-window testing also
checks the original agent PID/TTY, hidden surfaces and session-report routing.
macOS linked tests do not certify Windows named-pipe runtime behavior.

## Lessons

Resource ownership includes the executor of a listener, not just a path or
channel. Session migration must audit rendering, subscriptions, transport,
runtime lifetime and persistence together. A successful first callback is not
proof that closing the original owner is safe.
