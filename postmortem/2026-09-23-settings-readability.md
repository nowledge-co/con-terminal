# Settings readability regression (#385)

## What happened

After the Ghostty-style configuration change (#373), Flexoki Dark input
placeholders looked like populated fields. The same report called out small
Settings text that ignored UI Size, uneven field alignment, and Appearance
controls ordered ahead of terminal themes and fonts.

## Root cause

The configuration change also added UI contrast correction. A preferred color
below 4.5:1 contrast was replaced outright with black or white. Flexoki Dark's
muted text changed from `#787772` to white, brighter than its `#CECDC3` body text.
GPUI inputs use that muted color directly for placeholders. Existing contrast
tests checked a lower bound but not the distinction between text roles.

The other symptoms predated #373: Settings mixed fixed-pixel text with scaled
component text, field rows used natural label widths and fixed heights, and
Appearance placed icon/avatar choices before themes.

## Fix applied

Contrast correction now moves the preferred color toward the better black/white
endpoint only until it reaches the existing contrast target. Already-legible
colors remain unchanged; all supplied backgrounds are still checked.

## What we learned

Minimum contrast alone does not preserve visual hierarchy. Test both readability
and relative prominence, and inspect empty inputs as well as populated inputs.
Configuration migrations can contain unrelated presentation changes: attribute
each reported symptom from its own code path and history.

## Verification

The new Flexoki dark/light hierarchy test failed on the old implementation with
`flexoki-dark: muted text must remain less prominent than body text`, then passed
after correction. A separate test checks preservation of valid colors and
correction against multiple backgrounds. The existing low-contrast, warning,
and theme-mode tests continue to pass.
