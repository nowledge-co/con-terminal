# Portable text cursor style and blinking

## What happened

Windows and Linux displayed a steady block cursor even when a terminal program
requested a bar, underline, or blinking cursor with DECSCUSR.

## Root cause

The linked Ghostty VT API already exposed cursor style and blink policy, but
`VtScreen::try_snapshot` copied only position and visibility. Both renderers
therefore treated every visible cursor as a block. Their generation-based caches
also had no presentation-only invalidation for a blink transition.

## Fix applied

- Preserve style and blink policy in the portable cursor snapshot, without
  reparsing escape sequences or changing the Ghostty pin.
- Share a 600 ms host-side blink state machine. Hidden, steady, and unfocused
  cursors have no timer deadline. Focus and accepted key/IME input reset the
  visible phase; unrelated renders reuse the existing timer.
- Render non-block shapes without reversing the whole cell's glyph colors.
  Keep cursor decorations below above-text Kitty images.
- Position Linux cursor decorations using the cached text row's shaped glyph
  coordinates. Its approximate PTY grid width is not a text-layout metric:
  bundled IoskeleyMono advances 8.4 px at 14 px, versus the 9 px estimate.
- Include blink visibility in the Windows frame key and cursor row readback
  damage. Rebuild only affected Linux cursor rows for blink-only frames, rather
  than reusing the snapshot's original dirty rows.

## What we learned

Cursor policy belongs to the VT state; animation phase belongs to the host.
Generation-based rendering must account for both. Focus restoration also needs
an explicit phase reset: a hidden view may never render its intervening blur.

Regression coverage checks DECSCUSR 1–6, visibility and mode 12, the linked C
enum manifest, and blink timing/focus/policy transitions. Mutation checks must
fail when snapshot policy forwarding or the blink phase toggle is removed.
Native platform rendering remains a separate acceptance check from Rust tests
and cross-target type checking.
