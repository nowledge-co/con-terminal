# Portable terminal search without UI stalls

## What happened

Find in Terminal was available on macOS through the embedded Ghostty surface,
but Windows and Linux had no equivalent. Adding search to the portable VT path
also exposed a concurrency constraint: a native search can scan scrollback for
longer than a frame, while terminal output, rendering, resize, selection, and
text capture must continue to use the same terminal state safely.

## Root cause

The portable backends had no owned search lifecycle or way to turn Ghostty
search results into renderer highlights. Running native search directly from
GPUI would block interaction. Returning an empty snapshot when search briefly
held its lock would instead make terminal content disappear, and retaining
Ghostty grid references after releasing the terminal lock would be unsafe.

## Fix applied

Each open find bar now owns one FIFO background driver. Query replacement,
navigation, incremental search ticks, and cleanup all run away from GPUI's
thread. Results carry a generation so an older query cannot overwrite a newer
one. Search copies viewport match geometry while holding the terminal lock;
no native grid reference escapes it.

Rendering uses a non-blocking search lock and keeps the previous frame during
brief contention. Text readers wait for an accurate snapshot without holding
the render lock. Highlights are applied to the copied snapshot, merge
overlapping matches, cover both cells of wide glyphs, and yield to the user's
selection. Closing the bar waits for its driver to end the native search before
the bar is removed, so a stale cleanup cannot clear a newly opened search.

## What we learned

A responsive terminal cannot put scrollback work on the UI thread, but making
that work asynchronous is only half the contract. Cancellation order, stale
results, snapshot behavior under contention, and native-reference lifetimes
must be designed together. Regression coverage now exercises those boundaries
alongside live output, resize, alternate screens, overlapping matches, and
wide characters.
