# Whole-sidebar animation committed native geometry before the GPUI scene

## What happened

The maintainer reported broad backdrop strips at terminal edges during Cmd+B
(left sidebar) and Cmd+L (agent panel) toggles in the latest production build.
The screenshots show both a vertical strip and a horizontal strip, rather than
only an antialiased corner. The report remains an acceptance failure of the
previous visual verification.

## Evidence and limits

The local config uses Kanagawa Wave, terminal/UI opacity approximately 0.82,
blur enabled and vertical navigation. The screenshot has the input bar hidden.
Earlier verification used almost opaque Flexoki previews with an input bar;
compact/pinned switching also differs from whole-sidebar visibility toggles.

A new isolated fixture matches those appearance values and hides the input bar.
A copy of the installed release binary was also run with isolated data (the copy
was locally signed after changing the fixture bundle metadata; the production
app was untouched). Sampled screenshots have not reproduced the reported strip.
Temporary diagnostics on 77 Cmd+B/Cmd+L layout samples found IOSurface dimensions
matching final native pane bounds after synchronous draw. This does not prove
that Core Animation presented the texture, masks and GPUI scene together.
The diagnostic-only code is not part of the patch.

## Rejected candidate

Renewing the presentation request at paint passed all 732 linked tests, but the
maintainer still observed the gap in con Chrome Sync Review. The candidate was
removed and retained only as an investigation patch under .context. This result
rules out treating a renewed request alone as sufficient acceptance.

## Recorded evidence and accepted correction

The maintainer supplied `Screen Recording 2026-10-30 at 17.17.38.mov` (60fps).
The gap appears only while moving, also cuts off terminal text, and shrinks with
per-frame travel. Sampled physical-pixel strips during left reveal:

| Time | Gap columns | Width |
| --- | --- | --- |
| 1.317s | 77–148 | 72px |
| 1.333s | 150–228 | 79px |
| 1.350s | 230–300 | 71px |
| 1.417s | 461–486 | 26px |
| 1.450s | 515–527 | 13px |

The resolved GPUI `window.rs` frame callback invokes `draw` and `present` within
one App update; App deferred effects flush after that callback returns. Native
mask setters currently commit their own transactions during prepaint, before
GPUI's platform presentation. A synchronized drawable request alone cannot
combine transactions already committed separately.

The accepted correction opens one non-animated native transaction in the canvas
prepaint before terminal children and keeps it alive through platform
presentation, closing from App deferred effects. This joins native geometry,
masks, synchronous Ghostty drawing and the GPUI drawable in one transaction.
It preserves the caller's animation context, does not wait for a later frame,
and retains the asynchronous renderer path when native geometry is unchanged.
No opacity, duration, corner raster, dependency or release timer changes.

## Verification boundary

Native tests cover inherited animation suppression/context restoration and
transaction lifetime through paint and until the App update completes.
An initial test fixture dereferenced a nil `animationKeys` result and aborted;
the fixture now handles the legitimate empty/nil result. That abort is not a
passing test run. The new candidate passed all 733 linked macOS application tests; application
and companion CLI builds, formatting and skill validation passed. The matched
preview was opened as con Atomic Frame Review. Automated interaction stopped
when the tool reported the maintainer operating that window. The maintainer
then tested Cmd+B/Cmd+L and reported that the gap no longer appeared. This
acceptance covers the matched local macOS 15.7.9/2x Kanagawa/0.82/blur fixture
with input hidden; it does not certify all hardware, OS or portable paths.
This matched preview acceptance was recorded before production integration.

## Lessons

Model mask geometry, post-draw IOSurface size and empty error logs do not prove
compositor consistency. Measure the moving boundary in the recording and test
the lifetime of the transaction that contains actual platform presentation.
Match the release/configuration/input visibility and exact Cmd+B/Cmd+L paths.
