# Inactive tab text on transparent chrome

## What happened

Inactive horizontal tab titles looked faint and outlined on a transparent dark
title bar. Active titles were legible; light themes made the problem much less
noticeable.

## Root cause

The generated theme already supplies a solid `muted_foreground`, blended from
the theme's foreground and background and checked for contrast. Horizontal tabs
then applied another 0.62 alpha to that text and 0.38 to inactive icons. Vertical
tabs also applied extra alpha, though less aggressively.

This reduced glyph coverage over chrome that was itself translucent. In the
pinned GPUI Apple renderer, `monochrome_sprite_fragment` multiplies the text
color's alpha by the glyph atlas coverage. Extra text alpha therefore fades the
inside of each glyph as well as its antialiased edge, admitting the desktop
backdrop into the text. Light text on dark translucent chrome made the resulting
edge/contrast problem particularly visible. This was a color-composition issue,
not a missing font, a new font rasterizer, or terminal-cell shaping.

## Fix

- Use the solid theme foreground for active titles and muted foreground for
  inactive titles and neutral icons in both tab orientations.
- Preserve font family, size, weight, tab geometry, accent fills, window opacity,
  blur, and animation. No renderer or terminal changes are required.
- Keep the change in shared tab presentation; no platform-specific input,
  terminal, or window behavior changes.

## Lessons

Use color, rather than an additional alpha multiplier, to de-emphasize readable
tab text on transparent chrome. Validate dark as well as light themes: an extra
alpha that looks unobtrusive with dark ink can fail with light ink.
