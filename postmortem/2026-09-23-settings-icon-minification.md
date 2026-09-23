# Settings icon minification (#386)

## What happened

A Windows report showed jagged edges on the A1 app-icon picker preview. The
same report showed wrapped Clipboard Writes text touching its card's bottom;
that separate layout issue is covered by the Settings readability repair.

## Root cause

The picker passed 512px PNGs to a 48 logical-pixel image element. At the pinned
GPUI revision, Windows uses linear sampling of a one-mip-level image atlas, so
large reductions do not integrate all source pixels. This is an aliasing-prone
path, not a Ghostty configuration parsing problem. Windows driver/DPI-specific
confirmation of the reported appearance remains necessary.

## Fix applied

App-icon picker previews are downsampled with Lanczos3 before GPU upload, to
their display-scale-adjusted physical size. Filtering uses premultiplied alpha
to keep invisible RGB out of curved edges, then converts to GPUI's straight-alpha
BGRA contract. GPUI's existing asset cache owns the result, keyed by asset path
and physical size. Original PNGs and native app-icon installation are unchanged.
The existing image dependency is shared across platforms with PNG decoding;
no dependency version or third-party source changes are required.

Follow-up review exposed a lifecycle gap: this GPUI revision's `use_asset`
notifies only the first requesting view. A new view joining a pending load
(for example after reopening Settings) can remain blank until an unrelated
redraw. The preview now uses the shared cached task directly and retains one
completion waiter per displayed preview using GPUI element state. Disappearing
views drop their waiters without discarding the shared pixel cache.

## What we learned

Linear GPU sampling is not sufficient antialiasing for large image reductions.
Cache processed pixels rather than repeating decoding during UI rendering, and
include physical size in the key for fractional and HiDPI display scales.

## Verification

Tests check transparent colored stripes: the output must retain the opaque
color in BGRA and average coverage instead of selecting one source sample.
Replacing Lanczos3 with nearest sampling made the coverage assertion fail
(alpha 255 rather than approximately 128); restoring it passes. Another test
checks the real A1 asset at 48/60/72/96/144 physical pixels, including transparent,
opaque, and partially covered pixels. Windows native rendering is not verified
on this macOS development machine.

A GPUI regression starts an in-flight load before creating the requesting view,
draws while the image is still pending, and then checks that completion alone
redraws the image. It failed with `joining view was not redrawn after the shared
load completed` on `use_asset`, and passes with the per-view completion waiter.
