# Settings titlebar alignment on macOS

Issue: https://github.com/nowledge-co/con-terminal/issues/374

## What happened

The standalone Settings title and save controls appeared below the native macOS
traffic-light buttons. The reported screenshot showed roughly 8 px of vertical
misalignment.

## Root cause

Settings centered its custom header content within a 44 px row, but its transparent
native titlebar used the default traffic-light position. The 78 px leading inset
prevented horizontal overlap without aligning the native and GPUI controls
vertically. A transparent titlebar does not automatically center native buttons
inside the custom header.

## Fix applied

Share `SETTINGS_HEADER_HEIGHT` between the header layout and window options. Use
gpui-component's `TitleBar` horizontal inset and ask AppKit for a standard button's
frame height with the Settings window style. Set the vertical inset to half the
remaining header height. Fall back to the component's geometry if AppKit cannot
provide a valid button size. GPUI retains this position for resize and fullscreen
transitions. Non-macOS window options retain their existing behavior.

## What we learned

Custom window chrome and native window controls need an explicit shared geometry
contract. Measure native button frames instead of estimating their size from the
visible circle or assuming the component's default inset is exactly centered.
An AppKit probe on the development machine measured 14 pt button frames: the
component-derived inset produced centers at 21 pt, while the measured-size inset
put all three centers at the header's 22 pt centerline.

## Validation

- `cargo check --locked -p con` passed on macOS with Zig 0.16.0 after caching a
  dependency that Zig could not download directly.
- Targeted rustfmt and `git diff --check` passed.
- An AppKit geometry probe confirmed all three button centers at 22 pt before
  and after resizing to 1100 × 800 and back to 920 × 680.
- Full Con UI interaction and fullscreen transitions were not exercised.
