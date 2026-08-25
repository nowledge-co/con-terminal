# macOS OSC desktop notifications were dropped

## What happened

Terminal applications could emit OSC 9 or OSC 777 desktop notifications, and
the embedded Ghostty runtime parsed them, but Con never displayed a macOS
notification or requested notification permission. The runtime action callback
reported the action as unhandled.

## Root cause

Con's hand-written Rust mirror of `ghostty.h` declared the
`GHOSTTY_ACTION_DESKTOP_NOTIFICATION` tag but omitted its `title` and `body`
payload from the action union. `action_callback` consequently had no matching
handler and returned `false`.

The FFI union also contained a synthetic 128-byte padding member. At the pinned
Ghostty revision the C union is 24 bytes, so Con's by-value callback argument
was 136 bytes instead of the header's 32 bytes. Reading only the leading fields
had hidden the mismatch, but it was not an ABI-safe representation.

## Fix applied

The Rust bindings now mirror the desktop-notification payload and enforce the
pinned header's payload, union, and action sizes at compile time. The action
callback forwards the borrowed, NUL-terminated UTF-8 strings synchronously to a
small Objective-C bridge, which copies them into `NSString` values before the
callback returns.

The bridge uses `UNUserNotificationCenter` to request alert and sound
authorization asynchronously. When authorization succeeds, it schedules the
same notification immediately with a unique identifier. No permission or
notification work blocks Ghostty's main-thread tick.

## What we learned

- A C union passed by value must match the upstream size exactly; oversized
  "safety" padding changes the ABI rather than making it safer.
- Borrowed callback strings must be copied before any asynchronous native API
  captures them.
- Native permission requests should preserve the triggering notification so a
  first-time user does not approve notifications only to lose the event that
  caused the prompt.
