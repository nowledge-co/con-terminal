# macOS pointer feedback was ignored

## What happened

The embedded macOS terminal ignored Ghostty's mouse-shape and mouse-visibility
actions. OSC 22 cursor shapes and hyperlink pointer feedback never reached the
host. This concerns the system pointer, not the terminal's text cursor.

## Root cause

The action tags were ABI-checked, but their payloads were not decoded. GPUI owns
cursor shape selection over the terminal hitbox, while AppKit's transient cursor
visibility is application-wide; invoking native setters in a callback would not
respect that ownership or the UI lifecycle.

## Fix applied

- Coalesce each surface's latest shape and visibility without allocating events.
- Check all mouse enum values and payload sizes against the pinned C header.
- Apply shape through GPUI and transient visibility on the main thread using
  `setHiddenUntilMouseMoves:`, matching Ghostty's native macOS implementation.
- Only the actual focused, visible terminal under the pointer may hide it.
  A single main-thread owner prevents an old pane from undoing a newer hide.
  Blur, view hiding, detach, and rendering after exit release that ownership.
- Keep existing configuration defaults: `mouse-hide-while-typing` remains off.

## What we learned

Transient AppKit visibility can change on mouse movement without updating local
state. Reapply each hide request even when show/hide actions coalesce. A per-pane
boolean cannot safely own application-wide visibility across pane transitions.
Use the existing unique native-view owner ID rather than another ID allocator.

Regression tests cover action decoding/coalescing, distinct resize axes, unknown
shape fallback, repeated hides, and cross-pane ownership. Removing action handling
makes the regression test fail; removing owner matching makes the cross-pane test
fail. Native checks exercised crosshair/pointing-hand shapes and, with the existing
Ghostty option temporarily enabled in a test build, typing hide, motion restore,
Find focus, window deactivate/reactivate, surface switching, and surface close.
The temporary configuration change was removed after verification.
