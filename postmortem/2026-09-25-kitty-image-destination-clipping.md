# Magnified Kitty images lost partially visible texels

## What happened

The GPUI API migration replaced the Linux Kitty image layer's destination mask
with the new `paint_image` crop argument. Images enlarged relative to their
source resolution could lose visible edge pixels when clipped by the terminal.

## Root cause

The new image crop path rounds source atlas coordinates to whole texels. This
is not equivalent to clipping a continuously scaled image in destination space.
For a two-pixel image stretched to 20 logical pixels, clipping the first six
logical pixels should preserve four logical pixels of the first source texel.
Source-space rounding instead starts at the second texel. Clipping the first
16 logical pixels can round the remaining source width down to zero.

## Fix applied

Restore the content mask intersected with the viewport, and pass the full image
bounds as both bounds arguments to `paint_image`. This retains the complete
atlas tile and clips only its rendered destination.

## What we learned

A new bounds argument is not necessarily a replacement for a content mask.
Check coordinate spaces and rounding, especially for low-resolution images
scaled up to terminal cells. Linux application runtime acceptance remains
separate from macOS validation of the shared GPUI painting primitives.
