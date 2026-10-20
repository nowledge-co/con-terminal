# Pane corner contour investigation

Status: maintainer accepted the Con-only preview on 2026-10-18. This acceptance
does not certify the full display, theme, or platform matrix. PR and release
records establish delivery separately from preview acceptance.

Follow-up: the transparent light-theme report on macOS 27 exposed a limitation
of this accepted approach. A translucent cutout does not remove the rectangular
pixels beneath it. See [the glass corner coverage follow-up](2026-10-20-glass-pane-corner-coverage.md)
for the correction and its separate verification limits.

## Report

The maintainer's October 18 close-up shows dark, uneven contours around light
terminal cards, most clearly where a vertical split meets a horizontal split.
The beta.140 gutter correction does not resolve this rendering defect.

## Evidence

The current macOS card has independently rasterized components: a native
rounded clip, opaque GPUI exterior cutouts with one physical pixel of diagonal
overlap, a rounded header fill, and a translucent one-pixel contour. The
cutout's modified cubic does not describe the same circle as the native clip
and contour. That discrepancy is real, but is not by itself a demonstrated
explanation for the reported dark pixels.

The pinned GPUI 0.3.7 Apple snapshot also retains the destination-alpha `One`
blend factor documented in the October 8 transparency postmortem. That defect
was separately verified by native GPU readback tests. It does not establish
complete visual causality for this screenshot. The upstream proposal,
[Zed #65295](https://github.com/zed-industries/zed/pull/65295), is closed without
merge. A fresh official-source checkout revealed the equivalent correction in
[Zed #65571](https://github.com/zed-industries/zed/pull/65571), merged October 14.
The latest published `gpui-pre-apple` 0.3.9 archive was downloaded and verified
against its registry checksum on October 18; it does not contain the fix.
Checking only our old PR status missed this independent upstream merge.

## Bounded Local Comparison

A debug build from main `1352a3ad` used temporary diagnostic switches and an
independent three-pane fixture with the configured light theme. No live
conversation or production session was used. The native presentation sync
dependency was unchanged.

- Baseline: reproduced uneven, dark corner contour details.
- Disable only the contour: dark detailing was visibly reduced, but the
  intended pane outline disappeared. This is localization, not a solution.
- Disable only the cutouts: native gray backing appeared outside the rounded
  cards. Removing these cutouts is not a valid fix.
- Use rectangular native clipping and an exact GPUI corner arc: no clear
  overall improvement in this light-theme comparison. Do not ship that change
  based on the claim that one rounded-mask owner must solve the problem.

These are settled-window observations, not pixel-difference measurements,
dark-theme acceptance, transition-frame tests, or portable runtime results.
All temporary runtime switches were removed from the source after diagnosis.

## Earlier Borderless Comparison

At the maintainer's request, an intermediate preview kept the exact dependency pins
and removes only the pane contour overlay. Native pane clips, cutout geometry,
input/composer contours and presentation synchronization remain unchanged. It
is a deliberately quieter, borderless visual alternative, not an alpha-blend
correction or acceptance of every corner artifact. The 23 pane-tree tests and
formatting passed. This was not the final accepted solution: the maintainer
still saw dark corners, so the follow-up restored the fine contour.

The three public fork branches have different purposes:

- `fix/con-native-presentation-snapshot` at `655ce8b...` is the compatible
  package snapshot consumed by Con's immutable pin.
- `fix/native-surface-presentation-sync` at `8bd2ec8...` carries the upstream
  source proposal discussed in Zed Discussion #65433.
- `fix/gpui-transparent-source-over` at `4e84c0e...` is the source of the
  closed, unmerged Zed PR #65295.

No remote branch was changed. A future approved combined experiment should
start on a separate branch so neither upstream candidate acquires unrelated
changes. Moving any branch does not update Con's existing immutable pin.

## Accepted Fix

The maintainer still saw dark corners without the pane contour. Removing a
shadow cannot explain it: pane contours already request no contact shade.
The follow-up retains fine contours but removes competing coverage boundaries.
GPUI alone draws the circular cutout. The header supplies an opaque rectangular
fill behind it, and the native host applies a rectangular pane clip rather than
independently antialiasing the curve. The cutout uses the original circular
cubic without the compensating inward overlap. Transparent border fill retains
the border's RGB instead of interpolating toward transparent black.

The native clip now honors pane rectangles even at zero radius. Zero radius is
not equivalent to no clip: reused or resizing hosts remain confined to the pane
and workspace intersection. A linked test checks that behavior and subsequent
clean-mode mask removal. The final full run passed all 652 app tests, strict
Clippy, formatting and diff checks. An initial native test failed because an
already-unchanged setter was still expected to return true; that assertion was
corrected, and the full run passed without exclusions. The app built and its
light-theme preview was inspected. Dark-theme, 1x, full TUI and display/power
transition acceptance remain separate. Dependencies, native presentation sync
and Windows/Linux runtime paths are unchanged. The maintainer inspected this
preview and accepted its appearance on October 18. This does not mean the
independent upstream alpha defect has been corrected in Con's dependency.

## Follow-up and Diagnostic Route

- Keep the accepted contour, rectangular header/native coverage, and single
  decorative rounding owner together. The earlier partial ownership experiment
  was not sufficient; do not present it as a proven fix on its own.
- Before broader visual acceptance, check opaque/translucent light and dark
  themes, 1x/2x edges, TUI/selection clipping, nested split/zoom/close, and rapid
  chrome reversals. Windows/Linux runtime verification remains unavailable.
- Keep static corner defects separate from intermittent native/GPU presentation
  leaks. Do not hide either with opaque terminal fills or altered blur.
- Follow [Native Terminal Presentation](../docs/design/native-terminal-presentation.md#official-replacement-audit-2026-10-18)
  for the official-source/package audit and exact fork exit conditions. The
  merged alpha fix cannot replace the still-missing frame-scoped public API.
  No new alpha fork or duplicate upstream PR is needed for this Con-only fix.

The durable lesson is to inspect every owner of edge coverage, not just the
visible outline or shadow. Native masks, header fills, cutouts and contours
can each antialias the same edge independently. Preserve rectangular clipping
for reused hosts even when decorative rounding belongs to GPUI.

## Upstream Contribution Handling

The old PR was submitted before required human self-review and before/after
evidence were complete. A README marker or an AI-checked box does not establish
that review. The maintainer requested genuine human understanding and manual
testing before reopening. Their current contribution guide also asks for
human-authored communication; translated text should be quoted alongside the
original language. Do not create another PR for the already-merged correction.
Any distinct future contribution must be rebased, tested in Zed itself,
supported by before/after artifacts and personally reviewed by the author first.
