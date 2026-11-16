# Settings shortcut descriptions collapsed to one character per line

## Report

The Keyboard Shortcuts page could render its disabled Global Hotkey shortcut
description in a nearly zero-width column, making text run vertically down the
window. The layout remained unusable even though the overall page filled it.

## Cause and correction

The horizontal shortcut rows did not explicitly allocate remaining width to
their descriptive text. The sibling keycap area also lacked a non-shrinking
width contract. Set the descriptive column to flexible remaining width with a
zero minimum, and keep keycaps from shrinking. At mobile widths, stack the
columns and give the description full width. Apply this to Global Hotkey and
Quick Terminal together.

## Prevention

Measure the label width and height inside the actual Keys page at desktop,
compact and mobile widths, not only the outer Settings frame. Page switching
and button screenshots do not validate every section's inner flex layout.
