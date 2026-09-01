# macOS IME preedit retained the terminal cursor

## What happened

While composing text with a macOS IME, Con displayed the marked text inline
but left the terminal cursor visible at its leading edge. Committed text and
the candidate window otherwise behaved normally.

## Root cause

Con stored GPUI's marked text and rendered it as a separate GPUI overlay above
the embedded Ghostty surface. The overlay never set libghostty's preedit state,
so Ghostty continued to render the normal terminal cursor.

The overlay also treated `ghostty_surface_ime_point` as the marked text's
origin. That API returns a candidate-window anchor at the horizontal midpoint
of the cursor cell, which left the cursor partially uncovered.

## Fix applied

Con now forwards marked text through `ghostty_surface_preedit`, which was
already available in the pinned Ghostty revision. The native renderer owns
preedit text, underlining, cell width, clipping, and cursor suppression. Con
clears native preedit state before sending committed text and when GPUI
unmarks the composition. The redundant GPUI overlay and its color helper were
removed.

## What we learned

- Marked text and the terminal grid must have one rendering owner.
- An IME candidate-window anchor is not a text-layout origin.
- Embedded Ghostty integrations should expose existing surface APIs before
  recreating renderer behavior in the host UI.
