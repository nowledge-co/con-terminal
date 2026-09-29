# Command Palette results were clipped

## What happened

The palette opened with a full-window backdrop and visible search field, but
only the top of the first command appeared. The remaining results were hidden,
making the palette unusable. The report is tracked in issue #420.

## Root cause

The results wrapper had a maximum height but no definite height. Its scrollable
child requested `size_full()`, so the parent's height depended on a percentage
of its own unresolved height. GPUI could collapse the results viewport while
still painting the search field. The screenshot matches that layout: the card
ends just below the results' top padding rather than at the window boundary.

## Fix applied

Give the results viewport a definite height based on the number of commands,
capped at 360 points and by the space below the search field. Keep the existing
scroll handle, scrollbar, selection, and keyboard behavior. The palette also
respects narrow windows instead of always requesting 560 points of width.

## What we learned

Scrollable content cannot establish its own height with `h_full` when its parent
has only `max_h`. The owner of the viewport must provide a definite size or a
definite flex constraint. A pure sizing test covers empty, short, long, and
small-window result sets; the running dev build still needs visual acceptance.
