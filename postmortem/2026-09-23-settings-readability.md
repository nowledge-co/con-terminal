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

Settings prose now uses the existing UI scale or rem-based component styles,
including auxiliary text on the provider, keybinding, and configuration pages.
Field labels share a scaled column; rows grow with their contents instead of
clipping larger text. Wrapped row descriptions use definite preferred widths so
their measured heights include every line. Narrow headers omit the redundant
save-status label to keep the window title and Save button visible.

Appearance now starts with terminal themes and fonts; app icons and agent avatars
follow terminal controls and theme import. The theme card cannot shrink vertically,
so wrapping its grid at larger UI sizes does not clip themes before the Fonts group.
Theme helper text uses the muted role without additional opacity attenuation.

Follow-up review found the same opacity attenuation on provider labels/status,
font fallback instructions, theme import, keybinding help, and update information.
Those readable labels now use the muted role directly. Decorative status dots,
background fills, and disabled-state opacity remain unchanged.

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

The Settings layout test covers 16px/24px UI text at 375/600/840/920px window widths,
proxy alignment, heading scaling, title fit, wrapped row descriptions, and Appearance order.
Restoring the old 10px heading makes that test fail. The wrapped-description
assertion also failed before using a definite column width. Removing two trial
`flex_shrink_0` rules kept it green, so those redundant rules were discarded.
The order assertion failed with the old section order and exposed theme-card
compression at large UI sizes. An explicit inner width did not fix compression
and was removed; preventing the outer theme card from shrinking did.

Follow-up layout assertions check horizontal containment and separation of
select/slider controls, plus the unsaved-close action area at all eight sizes.
The description columns can shrink: adding their preferred widths does not prove
overflow. These assertions passed without changing row geometry. Native before/
after screenshots check the auxiliary-text color correction; the layout tests
do not assert rendered text colors.
