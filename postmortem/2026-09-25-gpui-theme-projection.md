# Theme overrides stopped reaching base components after the upgrade

## What happened

After migrating to gpui-component 0.6.6, Con still overrode the styled theme's
font and scrollbar fields, but base primitives and cached TextView defaults
could retain the values from before those overrides.

## Root cause

The component library now projects its styled theme into a separate
`gpui_base::Theme`. `Theme::change` publishes that projection, but Con modified
the font and scrollbar fields afterward without publishing them again.

## Fix applied

Call `Theme::sync_base` once after both groups of overrides, on initialization
and theme changes. A regression test switches light/dark/light and verifies
that the base scrollbar uses Hover and its tokens match the overridden theme.
The test failed before the fix with Scrolling instead of Hover.

## What we learned

Public mutable theme fields do not imply that downstream cached projections
update automatically. Review the theme publication API during library upgrades.
