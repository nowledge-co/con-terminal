# Input completion routing and the reveal's bottom seam

## What happened

The isolated Magpie preview initially had no slash skills: its separate HOME
and test working directory contained no skill roots. Read-only roots were
added to the preview configuration without changing the user's live settings.

With skills available, native checks exposed a separate keyboard regression:
Down still completed the first skill and Tab did not complete it. In dark mode
over a light backdrop, hiding/showing the input tray could also expose a thin
bright line below the moving card.

## Root causes

The registered Tab/Up/Down actions handled ghost text and history directly.
The raw key listener had skill-aware behavior, but those actions consumed the
keys before it. Existing Enter tests did not register the production bindings.
Late skill scans also notified only the input entity even though the workspace
owns the popup, and the bounded menu clipped long lists without scrolling.

The tray surround subtracted the full 44pt card while it slid below the viewport.
The visible card ends at the reveal clip, leaving a bottom inset outside that
clip. The full card's bounds hid this gap from the surround calculation. Separate
analytical geometry also disagreed with actual layout rounding at fractional
progress.

## Fix

Route bound actions through the same completion-priority logic as raw keys:
skills, path candidates, then history or ghost suggestions. Notify the popup
owner when the skill catalog changes. Keep the skill list scrollable and reveal
the selected row only when input or keyboard selection changes, preserving
mouse scrolling.

Collect the actual card and reveal bounds during prepaint, snap both using the
renderer's pixel grid, and subtract their intersection when painting exterior
chrome. All prepaint completes before paint. The frame-local geometry never
carries an older frame's bounds and adds no animation clock or opaque backing
beneath the card. Keep the existing glass alpha, corner coverage and motion.

## Verification

The production-binding regression reproduces the keyboard failure before the
fix, then covers Smart, Shell and Agent modes, focus retention and completion
without execution. A late-catalog test verifies overlay invalidation without
more typing and guards against render-time notification loops. Preserve the
trailing separator after completing a skill, so its menu closes and the next
Enter can submit it; both the command bar and agent composer previously trimmed
that separator away while testing for completion.

The reveal/reversal fixture samples actual card and clip layout and checks that
every exterior pixel, including the bottom gap, is covered exactly once across
fractional progress and 1x/1.5x/2x pixel grids. Native checks cover slash filtering,
keyboard/mouse selection, scrolling and dark/light tray motion.
