# Portable terminal snapshots dropped grapheme suffixes

## What happened

Windows and Linux rendered only a cell's first Unicode scalar. Combining
accents, variation selectors, and ZWJ sequences present in libghostty-vt's
render state were lost before shaping. Snapshot-based transcript and plain
URL extraction had the same loss.

## Root cause

`read_cell` requested `Codepoint`, but not the render iterator's complete
grapheme. Windows keyed its atlas by that scalar; Linux concatenated scalars
into one shaped row. Neither representation retained native wide-cell tails.

Passing complete strings to Linux's old whole-row shaper was insufficient:
with DEC 2027 off, separate native emoji cells could recombine in the host
shaper and occupy fewer columns than the terminal assigned.

## Fix applied

- Read complete graphemes and native widths from the existing render iterator.
  Single-scalar cells keep an allocation-free path; multi-scalar text is owned
  by shared strings, independent of native iterator lifetime.
- Shape complete Windows clusters and include their text and native width in
  atlas keys. Reclaim offscreen entries before constructing frame instances;
  never clear slots already referenced by the current frame. If the live set
  exceeds atlas capacity, preserve existing glyphs rather than corrupting them.
- Paint Linux rows with cached text spans on the native column grid. Batch
  ASCII where glyph indices permit it; shape non-ASCII cells independently.
  Preserve cluster-internal glyph offsets, and never mutate GPUI's cached layouts.
  Unchanged row spans retain their shaped layouts across terminal updates.
- Use the same text representation for transcripts and links. Normalize wide
  tail selection to its head, while conceal removes text only at paint time.

## Verification and limits

Regression cases cover accents, ZWJ and variation selectors, mode 2027 on/off,
overwrite and old-snapshot ownership, conceal, wide-tail links, and selection.
Removing grapheme extraction reproduced the scalar-only output and failed the
regression test; restoring it passed.

A temporary macOS harness linked the pinned libghostty-vt and exercised the
portable VT tests. Its existing Alt-key encoding failure also reproduced with
the unchanged baseline. Production Linux row code and link tests were compiled
against GPUI, and the actual row canvas was rendered and inspected on macOS.
Windows backend tests were cross-type-checked. These checks do not replace
Windows DirectWrite or Linux font-backend runtime validation.

On this host, a release-mode 160×50 ASCII probe measured full feed plus snapshot
at approximately 109 µs before and 111 µs after. Cached snapshot copying rose
from approximately 4.4 µs to 13.1 µs because cells now carry optional shared
text and width metadata. These are local microbenchmarks, not renderer latency
or a claim of platform-wide performance parity.

## What we learned

Preserving UTF-8 bytes and preserving terminal cell boundaries are separate
contracts. Font shaping must not redefine the terminal's column allocation.
Test both native grapheme modes, and distinguish upstream normalization from
data lost by the host. Cache identities must include full cluster text, while
cache eviction must respect every instance already built for a frame.
