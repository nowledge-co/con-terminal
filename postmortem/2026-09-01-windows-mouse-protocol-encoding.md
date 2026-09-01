# Windows mouse reports ignored the terminal's active protocol

## What happened

The Windows backend formatted every captured mouse event as an SGR report.
Applications using legacy X10, UTF-8, URXVT, or SGR-pixel reporting therefore
received the wrong bytes. The view also omitted no-button motion in any-event
mode and middle-button input, and repeated motion events were not deduplicated.

## Root cause

Con routed mouse input from libghostty-vt's DEC mode bitset but encoded the
result independently in the Windows host. That duplicated only one part of
Ghostty's mouse protocol state machine and bypassed the mouse encoder already
exported by the pinned libghostty-vt ABI.

## Fix applied

The shared VT layer now owns a reusable Ghostty mouse encoder and event. It
synchronizes effective tracking and format state after terminal output is
parsed, updates physical geometry only when it changes, enables same-cell
motion deduplication, and serializes encoded writes with key input and parser
replies. Windows passes physical pointer positions and typed button identities
through that path for presses, releases, drags, hover motion, and wheel input.
The view now forwards middle-button sequences and any-event hover motion.

Terminal-captured events remain consumed when the active protocol legitimately
emits no bytes. This preserves X10 coordinate limits and Ghostty's other event
gates instead of incorrectly falling back to local selection or scrolling.

## What we learned

- Protocol routing and encoding must use the same effective terminal state;
  mode bits alone cannot reconstruct Ghostty's last-write-wins flags.
- Mouse button identities need a typed boundary because Ghostty's ABI orders
  right and middle differently from SGR button codes.
- Encoder geometry and mode synchronization reset deduplication in the pinned
  Ghostty revision, so geometry updates must be cached. PTY output still resets
  deduplication until Ghostty can preserve it when effective state is unchanged.
- The current public tracking query exposes requested mode bits, not effective
  flags. A conflicting DECSET/DECRST sequence can therefore consume one event
  after effective reporting has stopped, but the encoder now prevents stale
  bytes from reaching the child. An effective-state getter is the clean
  upstream follow-up.
- Linux still has a separate handwritten SGR path and should reuse the shared
  encoder in a follow-up change.
