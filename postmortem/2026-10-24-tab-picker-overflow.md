# Tab picker long-label overflow

## What happened

Long working-directory paths in the horizontal titlebar’s All tabs menu could
escape the item surface and collide with its selection mark.

## Root cause

The menu constrained its outer items container, but custom rows occupied an
intrinsic-width flex wrapper in the pinned gpui-component menu. A percentage
row width and a truncating child did not constrain that intervening wrapper.

## Fix

Give the picker a stable width bounded by the current window, and give its
custom row a definite width excluding the pinned menu’s outer/item padding.
Text receives the remaining space between a common identity well and the reserved selection slot. Middle
ellipsis retains the beginning and end of a path; the accessible name and
bounded hover tooltip preserve the full title. Keep the component’s menu
selection, scrolling, dismissal and keyboard handling.

## Lessons

Exercise the actual component wrapper when testing truncation. An isolated
row inside an explicitly sized div cannot catch intrinsic-width propagation
through a popup menu. Cover unbroken strings, multilingual names, multi-app
identities and narrow windows together.

## Verification

The real PopupMenu fixture passes at 600pt and 280pt window widths with 14pt
and 20pt UI fonts, with the production Root’s rem sizing, including POSIX/Windows paths, Chinese names, unbroken long
strings and mixed identity counts. All 17 focused top-bar tests pass. Native
Light/Dark previews confirm the final geometry; pointer selection, Down/Enter
activation and Escape dismissal were exercised against the isolated app.
Native Windows/Linux rendering was not available locally.
