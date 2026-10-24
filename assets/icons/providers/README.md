# Provider brand marks

Provider brand assets are separate from Con-authored interface icons, which use
Phosphor. Settings and the agent panel share the exhaustive mapping in
`crates/con-app/src/provider_icons.rs`; branded providers should not silently
fall back to the generic connection icon. OpenAI Compatible intentionally uses
that generic icon because it represents a protocol, not a brand.

New assets in this change:

- `magpie.svg`: the official small-size mark, copied unchanged from
  [`yetone/magpie`, `build/icon/magpie-small.svg`](https://github.com/yetone/magpie/blob/dd1e8822d916f8aeb085db4db93fc3d79e2263ed/build/icon/magpie-small.svg).
  MIT, copyright 2026 yetone; see `LICENSE.magpie.txt`.
- `codex.svg`: the monochrome Codex brand mark distributed by
  [LobeHub Icons](https://github.com/lobehub/icons/blob/7d7d0379f18b114a4009dc5cb85a7c375284f31b/packages/static-svg/icons/codex.svg),
  following the existing provider SVG format. This is a catalog asset, not an
  asset sourced directly from OpenAI. MIT, copyright 2023 LobeHub; see
  `LICENSE.lobehub.txt`.

Keep brand geometry intact. GPUI renders these marks with an explicit semantic
text color at each call site, so the same asset works in light/dark themes and
selected, configured, and inactive states.
