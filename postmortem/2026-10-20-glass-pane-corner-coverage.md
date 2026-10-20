# Glass pane corner coverage

Status: the initial coverage correction shipped in beta.143 with an explicit
remaining dark-rim limitation. A subsequent controlled split/header comparison
identified the extra GPUI header fill; the native-material follow-up below is
accepted by the maintainer in the updated local preview, with pane titles both
shown and hidden. Passing geometry/scene tests does not
establish correct composited edge colors. The macOS 27 comparison reached
startup/readiness checks only: SSH screen capture was denied and no desktop
session was available to inspect it. The maintainer requested a best-effort
release without screenshots. Do not describe this candidate as a complete
visual fix or claim macOS 27 visual acceptance.

## Report

After the October 18 corner change, the maintainer reported differently colored
pointed remnants outside light-theme pane corners on macOS 27. Disabling glass
removed them. No screenshot or macOS 27 machine was available during the initial
investigation. A remote host became available for the comparison below.

## Root Cause

The previous solution retained rectangular native terminal clipping and an
opaque rectangular header, relying on GPUI's inverse corner cutouts to supply
the visible rounding. Those cutouts inherit Window Chrome opacity. With glass
enabled, their exterior pixels blend with the terminal/header underneath rather
than only with the workspace background. Making the cutout curve more precise
does not remove that extra contribution.

This is a concrete coverage defect in Con. It is not proof that every pixel in
the unavailable macOS 27 reproduction has the same cause. The separate pinned
GPUI destination-alpha defect remains tracked in the earlier investigation;
this change neither patches it nor replaces the native presentation dependency.

## Fix

Forward the card's existing radius to the native pane mask instead of discarding
it. Round the uppermost header fill to the same radius, leaving internal joins
square. Keep the exact circular exterior cutouts and fine contour, unchanged
glass/blur settings, and frame-scoped native presentation synchronization.
Clean mode still uses zero radius and removes unnecessary native masks.

The native implementation already caches its shape paths. This change enables
the rounded mask where required; it does not add a timer, full-window paint or
per-frame path construction at unchanged geometry. Native masking still has a
compositing cost; no frame-rate benchmark is claimed.

## Verification

The local preview uses a separate home, configuration and session, without live
credentials or conversations. Light glass, light opaque and dark glass states
were visually inspected on macOS 15, including new splits and sidebar toggles.
The linked native regression exercises the production setter and checks all four
exterior tips, an interior point, retained path identity at unchanged geometry,
and clean-mode mask removal. A GPUI scene test checks header corner radii.

All 654 app tests passed without exclusions, including the new native and scene
regressions. The first full run had 653 passes and one new fixture failure: the
scene assertion incorrectly expected a style offset to survive layout unchanged.
The corrected assertion checks actual painted pixel alignment. The subsequent
Cargo-built test executable was stopped while blocked before test entry in
`_dyld_start`; after local ad-hoc signing, that exact executable ran all 654 tests
successfully. No system security settings were changed.

The final dev build passed and was relaunched in the isolated preview. A fresh
three-pane light/glass layout was inspected after the pixel-snapping change.
Pane closure back to clean mode was also exercised during the local comparison.

Formatting and whitespace checks passed. Strict Clippy with tests was attempted
but is not clean: unchanged `agent_panel.rs` reports `arc_with_non_send_sync`,
`workspace/render/popups.rs` reports `items_after_test_module`, and
`workspace/render/top_bar.rs` reports `useless_conversion`. Those files have no
diff against the base. No lint exemptions or unrelated fixes are included here.

macOS 27, Windows/Linux runtime behavior, 1x displays and transition-frame capture are not
certified by the local visual comparison. Runtime changes are macOS-only and no
dependencies or portable renderer branches are changed.

## Remote Comparison Setup

The maintainer provided `mac-mini`, running macOS 27.0 (26A5416b) on arm64.
It is an active CI runner. The two previews reuse native arm64 binaries built
elsewhere; no Cargo or Zig build runs on the runner. Both use separate bundle
IDs, homes, sessions, histories and sockets, with the same three-pane fixture,
Flexoki Light, blur and 0.55 terminal/chrome opacity. Each terminal reports the
same grid dimensions in both previews. The installed Con is left running.

The baseline executable comes from the checksum-verified beta.142 release.
The candidate is a native dev build with `rust-embed/debug-embed`: an earlier
ordinary dev build was stopped when remote smoke testing found missing icons
because the source checkout was absent. The replacement's startup log is clear.
The embedded-assets build also passed all four focused `assets::tests` locally.
This release-versus-dev comparison is for static appearance, not performance.

Both previews run at nice level 15. Sampled combined RSS was about 350 MiB;
idle CPU was low, but no zero-overhead or frame-rate claim is made. Screen
capture permission was not granted, and the maintainer chose manual screenshots
rather than changing permissions. Remote startup and surface readiness are
verified; composited corner appearance is still unverified.

After screen capture failed, the guarded cleanup script terminated only matching
preview executable paths, unregistered those two bundles, deleted their two
preference domains and removed the marked experiment directory. The installed
Con and CI runner were verified still running. No shared caches, global Launch
Services state or screen-recording permissions were changed.

## Release boundary

The beta.143 correction addressed rectangular source coverage, not every visible
corner artifact. Its remaining macOS 15 dark rim motivated the header-material
follow-up below, which passed local preview acceptance.
There is no evidence yet establishing the final corner appearance on macOS 27.
Do not widen this patch to a new GPUI fork, change the presentation sync pin,
make terminal glass opaque, or add an OS-version guess to hide the uncertainty.
Cross-version acceptance still needs screenshots or an authorized desktop session
on macOS 27. The follow-up's macOS 15 opaque/glass checks are recorded below.

## Lessons

- Treat coverage and material color as separate responsibilities. A translucent
  cover is not a clip, even if an opaque preview looks correct.
- A locally accepted visual simplification is not evidence across OS versions
  and opacity settings. Include translucent and opaque controls in acceptance.
- Test the production radius forwarding, not only stored geometry or the FFI
  mask implementation in isolation.

## Follow-up: Split Headers Versus a Single Pane

The maintainer supplied two screenshots on October 20: a single rounded
terminal had a quiet outline, while a split introduced dark detailing at the
upper header corners. The single and split terminal leaves already use the
same surround, contour and native clipping code. The important added layer
was the opaque rounded GPUI header fill, not another terminal renderer.

A local three-pane comparison retained the split geometry and toggled only
the header-visibility setting. Hiding the headers removed the upper dark
detailing. Restoring headers with native material instead of the GPUI fill
also removed that detailing in the local macOS 15 inspection. This identifies
the header composition as a controllable cause in this reproduction, not proof
that every corner defect or macOS version has the same explanation.

The follow-up keeps GPUI text and controls but moves their existing solid
material to a noninteractive native sibling of the terminal host. It shares
the pane's snapped bounds and upper radius, like a single pane's native
terminal material. The GPU overlay now contains the same exterior cutout and
fine contour whether or not a terminal has a title. No extra rounded header
quad competes with those curves in the overlay.

One native material spans both title and surface rows; its height comes from
the final viewport top, excluding terminal seam overdraw. It follows terminal
visibility, stays hidden after a return to a no-header layout, and is removed
on detachment. Geometry/color/scale/visibility caching prevents repeated layer
mutation or color allocation at idle. The shared canvas clip still constrains
it, and genuine updates use the existing frame-scoped presentation request.
No dependency pin or Windows/Linux renderer changes are involved.

The local comparison uses an explicitly supplied private HOME and session,
history, conversation and socket paths. Launch Services did not preserve the
private HOME on an earlier wrapper launch; no settings were saved from that
launch. Repeating the check with an explicit process environment and verifying
the header toggle took effect established the controlled comparison. A config
file's contents or hash alone is not evidence that the app loaded it.

The native-material follow-up passed all 653 linked app tests without
exclusions. The former standalone GPUI-header-quad test was removed with that
primitive; the existing pane-control scene test now rejects a full-header GPU
fill, and the native fixture exercises the real header setter, upper corners,
cache reuse, noninteractive hit testing, terminal visibility, return to a
no-header layout and removal. Two initial fixture mistakes (comparing an
Objective-C rectangle directly and using the wrong numeric corner flag) were
corrected; the latter was checked against the installed QuartzCore SDK.

The final native embedded-assets dev build and formatting/whitespace checks
passed. Local visual checks covered light/dark glass, light opaque mode, combined pane/surface titles,
nested splits, switching tabs/surfaces, zoom/restore, and closing siblings back
to a single pane. The maintainer subsequently confirmed the updated preview
looked correct with titles both shown and hidden, and approved release.
This is local visual acceptance, not a GPU pixel-difference test, a frame-rate benchmark, or macOS 27 /
Windows/Linux runtime certification. The independent upstream alpha issue and
display/power-state presentation acceptance remain separate.

The release-preparation rerun initially stalled before test entry at
`_dyld_start`. Sampling `codesign` on the same executable found CoreFoundation
enumerating the large shared `target/debug/deps` directory. The freshly built
test executable was copied to the small private preview directory and ad-hoc
signed there; no system permissions or production signing settings changed.
Its first full run passed 652 tests but hit the unrelated launch-helper
protocol fixture's four-second deadline. That fixture passed in isolation.
The same executable then passed all 653 tests with `--test-threads=1`, without
skipping tests or changing source or test deadlines.

## Concentric-corner integration review

PR #496 was authored before the native-header follow-up. Its GPUI surfaces
derived four radii from the AppKit window corner, but the existing native pane
mask and header API still accepted one uniform radius. Merging those layers
without adapting them would violate the shared-coverage rule again, especially
where a bottom corner meets a larger window corner.

The integration forwards the same resolved four radii to the native pane mask
and the upper pair to native header material. Both use cached circular paths;
the header remains noninteractive and independent of the workspace clip. Native
fixtures check asymmetric corner coverage and cache reuse, not only uniform
rounded rectangles. All layers clamp radii consistently for small panes.
The macOS-only traffic-light helper export is also gated so portable builds
do not acquire an unused import under warnings-as-errors.

The integrated candidate passed all 656 linked macOS app tests serially,
without exclusions or changed deadlines. Formatting and whitespace checks
passed. This verifies native path geometry and lifecycle on macOS 15; the
contributor's macOS 27 screenshots remain separate evidence, not a local
macOS 27 runtime certification.

The integrated dev app also built and ran in a separate private profile.
Inspection covered light/dark glass, nested panes with surface titles, the input
tray and agent panel, zoom/restore, a single-tab caption, and entering/leaving
native fullscreen. These are settled-state checks, not pixel-difference or
high-frame-rate acceptance. The maintainer noticed remaining rough dark pane
edges in this preview and requested that #496/#499 merge first, followed by a
separate rendering fix before the next release. Do not treat the geometry tests
or the initial local inspection as acceptance of that outstanding visual issue.

## Follow-up: Dark Corner Antialiasing

The maintainer localized the remaining roughness to pane corners. A separate
candidate moves only the pane contour inward by one physical pixel and reduces
its radii by the same amount. Corner centres, native clipping, exterior cutouts,
glass, blur and the presentation dependency are unchanged. The straight contour
also moves inward; this is not a corner-only geometry change. Input trays and
agent composers retain their existing contours. All changed runtime call sites
are macOS-only.

The pinned quad shader uses a half-physical-pixel coverage threshold. Separating
the contour from the exterior curve tests whether overlapping coverage ramps
contribute to the roughness; it does not establish their causal contribution to
every visible pixel. The candidate adds no layers, timers or full-pane paint.

All 657 linked macOS app tests passed serially, including a geometry regression
at 1x, 1.5x and 2x. Its first assertion used exact float equality and failed at
1.5x due to a roughly 0.00001pt rounding difference; the corrected assertion uses
a 0.0001pt tolerance. No production geometry or test deadline changed to pass it.
The embedded-assets dev build and formatting/whitespace checks also passed.

Separate baseline/candidate bundles were inspected with the same three-pane
fixture, surface headers, bottom input tray, 0.82 glass/chrome opacity and blur
on macOS 15. Light appearance remains quiet, but the dark comparison still has
visible fine edge artifacts. This experiment is not accepted as the completed
visual fix and must not be merged or released on geometry tests alone. There is
no local Windows/Linux, macOS 27, 1x-display or high-frame-rate visual acceptance.
