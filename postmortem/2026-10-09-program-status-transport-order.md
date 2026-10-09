# Program-status transport ordering

## What happened

Before merging OSC 7501 transport, review found that its 1024-event FIFO
could evict a clear/reset during a burst. A previously displayed record could
then survive even though the program had removed it. Native key paths also
acknowledged completions before knowing whether input was accepted.

## Root cause

The queue bounded event count rather than protocol state. Lifecycle operations
were treated like disposable progress updates. Message storage was duplicated
between the queue and reducer, and input acknowledgement preceded fallible
host delivery.

## Fix

Move the pure status model to con-terminal, preserving con-core's exports.
Backends reduce events at ingress and hand off a dirty, bounded snapshot;
no second parser and no dependency on the agent harness is introduced.
Apply accepted user input in order with the handoff. Rejecting a host write
does not acknowledge completion; raw tool writes and empty input do not either.
Callback copies validate size, field limits and UTF-8 before retaining text.
Terminal-incarnation checks precede both live and exited-terminal handling.

## Verification and limits

Regression tests cover report floods around subtree clear and RIS, prompt/input
ordering, and ordinary input without protocol traffic. Native VT tests cover
fragmented reports, rejected host writes, IME text and surface isolation.
Native Windows/Linux tests must run in CI; a macOS build does not exercise
those paths. The maintainer authorized a temporary public-fork pin on
2026-10-09; official upstream adoption remains tracked in #444. New-revision
linked and live verification remains required before merging transport.

## Upstream bridge blocker

Review of the proposed bridge at `4e48c76e04` also found that the newly
allocated `Owned` report is freed only by `Surface.handleMessage`. When the
surface has already closed, `App.surfaceMessage` drops the message without
freeing that payload. App shutdown does not drain those allocations either.
The corrected proposal releases undelivered reports on close and shutdown.
Reports, prompt and reset events carry immutable surface IDs, checked before
delivery to prevent allocator address reuse from changing their attribution.
Allocator-backed tests cover discarded reports, shutdown and replaced IDs.

The original transport also put every report into the shared 64-entry app
mailbox. On a full queue, `surfaceMessageWriter` waits uncancelably for space;
surface teardown joins that producer on the consuming thread. A status flood
can therefore introduce a close deadlock, independently of Rust's bounded
reducer. Con instead opts into a synchronous IO callback. Ghostty decodes into
stack scratch space; Con copies the borrowed fields and reduces in order,
without a heap-owned Ghostty report or a per-report UI queue. Prompt and reset
use the same ingress. Userdata is freed only after surface IO joins.

## Follow-up

- Consume an official Ghostty revision once its embedder API is accepted;
  remove the scoped public-fork exception, keeping lifecycle and burst tests.
- Verify native GUI behavior on Windows/Linux in addition to linked CI tests.
- Record live query/report forwarding through SSH and tmux before declaring
  the full acceptance tracker complete.

## Lessons

Bound the semantic state, not a queue whose discarded events can alter that
state. Input acknowledgement is a delivery fact, not an attempted UI action.
Keep experimental upstream integration distinct from merge/release readiness.
