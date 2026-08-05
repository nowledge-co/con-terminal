# Markdown preview: last blocks painted over each other

## What happened

While reviewing `README.md` in the editor's new markdown preview, the final
paragraph painted on top of the last list item. The document tail — a bullet
list followed by a normal paragraph — overlapped exactly one row's worth of
text.

## Root cause

Not the markdown parser (the block tree was correct), but the GPUI layout of
the preview container. The preview stacked blocks with
`div().w_full().flex().flex_col().gap(...)` — the same shape as the agent
panel's whole-document renderer.

Inside a flex column, taffy measures children for intrinsic height with a
non-final wrap width. `gpui::StyledText`'s measure callback computes the
text layout with whatever `available_space.width` it first sees and caches
the resulting size in its `TextLayout` state; a later measure call with
`wrap_width == None` (max-content) then returns that cached, unwrapped
height. The wrapper block ends up laid out with the unwrapped height while
the text still paints wrapped at the final width — the block is too short
and the next sibling overlaps it.

In the failing case a two-line list row measured 48px at paint time but the
list block's height was computed with one-line rows (25px), so the following
paragraph landed on top of the last row. Which block ended up mis-measured
depended on taffy's measure call order, which is why the corruption appeared
at the document tail.

The agent panel had already hit this class of bug (see
`2026-04-26-agent-panel-long-markdown-render-tree.md` — "caching an
intrinsic-height rich-text subtree at the wrong boundary can create layout
bugs") and sidestepped it by giving every block its own entity and using
`ListState` virtualization instead of one giant flex column.

## Fix applied

`render_parsed_chat_markdown_file_preview` (the preview's whole-document
renderer) now stacks blocks in a plain block-layout `div` (gpui's default
`display: block`) with `pb(block_gap)` padding between children instead of
`flex().flex_col().gap(...)`. Block containers measure children with the
definite width directly, so wrapped text reports the correct height.

The bug was chased down with a new visual layout test harness:

- `crates/con-app/Cargo.toml` dev-depends on `gpui` with the `test-support`
  feature.
- `render_block_with_width` and `render_list_children` carry
  `debug_selector(...)` markers (compile to no-ops in release builds).
- `chat_markdown::tests::list_tail_and_paragraph_do_not_overlap` renders a
  README-tail document into a test window and asserts block/row bounds via
  `VisualTestContext::debug_bounds` — it failed with the exact overlap seen
  in the app and now passes.

## What we learned

- Do not stack intrinsic-height text blocks with `flex_col + gap`; prefer
  block layout plus padding. If a flex column is unavoidable, per-block
  entities are the proven-safe boundary in this codebase.
- GPUI's `debug_bounds` + `debug_selector` harness makes layout regressions
  cheap to pin: the failing bounds immediately showed the list wrapper
  measuring 57px where 103px were painted.
- When text paints outside its layout box, suspect the measure callback's
  cached size (width-dependent) before suspecting the container tree.
