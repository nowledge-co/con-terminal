# Text and icon halos on transparent macOS chrome

## What happened

Inactive horizontal tab titles looked faint and outlined on a transparent dark
title bar. Active titles were legible; light themes made the problem much less
noticeable. After #437 removed extra tab-text opacity, the same dark edges
remained around file-tree icons, search controls and search-result text.

## Root cause

There were two separate contributors. The generated theme already supplies a solid `muted_foreground`, blended from
the theme's foreground and background and checked for contrast. Horizontal tabs
then applied another 0.62 alpha to that text and 0.38 to inactive icons. Vertical
tabs also applied extra alpha, though less aggressively.

This reduced glyph coverage over chrome that was itself translucent. In the
pinned GPUI Apple renderer, `monochrome_sprite_fragment` multiplies the text
color's alpha by the glyph atlas coverage. Extra text alpha therefore fades the
inside of each glyph as well as its antialiased edge, admitting the desktop
backdrop into the text. Light text on dark translucent chrome made the resulting
edge/contrast problem particularly visible. Removing that extra opacity improved
contrast, but did not explain the same artifacts on unrelated text and SVGs.

The shared renderer defect is in GPUI's Apple Metal blend state. Both
`build_pipeline_state` and `build_path_sprite_pipeline_state` use `One` for
destination alpha, although RGB already uses source-over. This produces
`src_alpha + dst_alpha` instead of
`src_alpha + dst_alpha * (1 - src_alpha)`. The destination becomes too opaque
without a corresponding RGB change. When composited over the desktop, light
glyph and icon edges therefore acquire a dark halo. Text and SVGs share the
monochrome sprite pipeline on transparent windows.

The defect was verified in Con's resolved `gpui-pre-apple` 0.3.7 source and Zed
revision `a34a062bb74159231af5d01cc01011f0276d9743`. Native GPU readback tests
reproduced it for quads, glyph/SVG atlas coverage and paths. This is not evidence
of a missing font or a terminal-cell shaping problem.

## Fix

- Use the solid theme foreground for active titles and muted foreground for
  inactive titles and neutral icons in both tab orientations.
- Preserve font family, size, weight, tab geometry, accent fills, window opacity,
  blur, and animation. This shipped in [#437](https://github.com/nowledge-co/con-terminal/pull/437)
  and is a contrast improvement, not the complete renderer fix.
- Keep the change in shared tab presentation; no platform-specific input,
  terminal, or window behavior changes.

The renderer correction changes only two destination-alpha factors to
`OneMinusSourceAlpha`. It adds no draw calls or per-frame work. The existing
path-rasterization blend state is already correct and stays unchanged. The
upstream contribution is [Zed #65295](https://github.com/zed-industries/zed/pull/65295).
It is not yet integrated into Con's published dependencies.

## Verification

- Three new native Metal readback tests fail before the correction and pass
  after it. A representative translucent case changes alpha from 166 to 140
  while retaining RGB `[49, 49, 49]`.
- All seven Apple renderer library tests pass in Longbridge's official
  test-only GPUI snapshot workspace generated from the source revision above.
- An independent native Metal probe passes 15 background/coverage combinations,
  including unchanged RGB and opaque-background results.
- Upstream workspace formatting and diff checks pass. Windows/Linux and iOS
  runtime coverage are unavailable; their renderer code was not modified.

## Remaining Work

- Obtain upstream review and a compatible published GPUI/component snapshot;
  preserve one GPUI package identity rather than introducing a local fork.
- Upgrade the exactly pinned GPUI family and paired components together.
- Validate Con's dark/light transparent and opaque chrome, text, SVGs, paths,
  menus and search results in a build that actually consumes the fix.
- Run affected-consumer tests and platform CI before shipping. Until then, do
  not list the renderer correction as a released Con fix.

## Lessons

Use color, rather than an additional alpha multiplier, to de-emphasize readable
tab text on transparent chrome. Validate dark as well as light themes: an extra
alpha that looks unobtrusive with dark ink can fail with light ink. When the
same artifact spans unrelated controls and both text and SVGs, test the shared
compositor before continuing to adjust individual styles. A style improvement,
an upstream fix and a shipped dependency update are distinct delivery states.
