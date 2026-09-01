# Render update failure recovery

## What Happened

The incremental Render ABI review found that Con treated every render-state
update failure as a temporary extraction failure. It returned an empty
`ScreenSnapshot` for the current terminal generation and retried later with
the same `GhosttyRenderState`.

That behavior was unsafe for an out-of-memory failure from
`ghostty_render_state_begin_update`. It could leave the renderer with a
partial render state, and platform consumers could mistake the empty
snapshot for a successfully extracted frame.

## Root Cause

Con assumed `begin_update` was transactional. Ghostty's implementation can
clear terminal page and row dirty flags before a later row rebuild or style
allocation fails. Retrying the same render state therefore cannot reliably
reconstruct the consumed rows.

The snapshot API also had no internal failure boundary. Extraction failures
were represented as valid empty snapshots carrying the current generation,
so Linux or Windows could cache that generation instead of preserving the
last valid frame and retrying it.

## Fix Applied

- Added an internal fallible snapshot path for platform renderers while
  preserving the existing owned `snapshot()` interface for compatibility.
- Invalidated the render state after a failed begin or end phase. The next
  attempt creates a new render state, which forces Ghostty to rebuild the
  complete viewport even if terminal dirty flags were already consumed.
- Made Windows return `Pending` when extraction fails so the current image is
  retained and another prepaint is scheduled.
- Made Linux restore its `needs_render` flag and wake the workspace loop when
  extraction fails.
- Added a regression test that consumes terminal dirty state, discards the
  render state, and verifies that the same generation is rebuilt correctly.

## What We Learned

- A foreign update function that mutates state before returning an error must
  not be assumed transactional unless its contract explicitly guarantees it.
- Renderer acknowledgment must represent successful consumption, not merely
  an attempted snapshot for a generation.
- Retryable extraction failures need an explicit internal boundary so callers
  can preserve the last valid frame without changing stable public snapshot
  ownership.
- Recovery tests should start from consumed terminal damage; rebuilding only
  after new output would hide the state-loss case.
