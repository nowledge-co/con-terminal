# Input Composers

The bottom input bar is compact terminal chrome; the agent composer is a
multiline writing surface. They share control dimensions and submit styling,
not a second layer of decorative cards.

- Mode identity uses a 22-point cached transparent image inside a 28-point
  button. Its authored frame supplies the resting shape; only hover and press
  add a neutral control fill. All modes keep the same slot and line box.
- A shared GPUI Component button owns submit appearance in both locations:
  28-point circle, 14-point Phosphor arrow, foreground fill when actionable,
  quiet neutral fill when empty. Never tint the action with the mode color.
- The bottom bar stays flat. Its workspace owns transparency; do not add a
  second background or alter native terminal presentation during polish.
  Its 8-point leading inset matches the vertical control inset in a single-line
  row; all controls share an 8-point gap.
- The agent composer uses a single borderless, shadowless 12-point rounded
  surface with equal outer gutters. Its text column can shrink to fit narrow
  panels; the submit control remains stable at the bottom of multiline drafts.
- Preserve each input's existing font, keyboard submission, skill completion,
  history and IME behavior. Clicking send returns focus to the same input.

Dimensions scale through the existing UI density helpers. Raster assets use
the existing physical-size cache; no image decoding or resizing occurs in
paint. The generation prompts are in `assets/input-mode-icons.md`.

## Pane Targets

The scope popup belongs to command composition, not terminal output. Use UI
type for its labels and monospace only for keycaps. A compact header shows the
selected count on the same line; A and F belong beside their presets, while
numbered tiles identify individual pane toggles. The map and preset row share
one full-width frame, 3-point insets and 4-point gaps. Do not add a trailing
flex spacer to the preset row: it shrinks that row and offsets the midline.
The popup aligns with the target control and sits 10 points above the actual
input height, including multiline drafts. Clamp it inside the terminal gutters
at narrow widths. All dimensions follow UI density. The broadcast glyph denotes
all panes; labels stay neutral rather than tinting the whole control.

Pane tiles use opaque neutral selected fills with an explicit accent check mark,
not stacked translucent accent backgrounds. Number keycaps stay neutral. A target
glyph identifies the actual focused pane independently of broadcast selection;
process status remains a separate colored dot. Gaps, rather than extra separator
lines, express the live split layout. Fewer panes need less map height.

The popup stays nearly opaque in both themes so text underneath does not compete
with pane labels. This is local popup treatment, not a change to terminal glass.
Clicking presets keeps the map open; selection and routing remain unchanged.
