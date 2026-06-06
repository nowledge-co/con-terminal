# Quick Terminal Doubao IME composition was lost

## What happened

In Quick Terminal mode, typing into the input bar with the Doubao input method did not show committed or composing text. The same input bar worked in normal windows, and other input methods worked in Quick Terminal.

## Root cause

Quick Terminal reuses the normal `ConWorkspace` and input bar, but wraps the GPUI window with extra AppKit behavior: it converts the window to a borderless transient window, orders it out, shows it again with `makeKeyAndOrderFront:`, and auto-hides on `NSWindowDidResignKeyNotification`.

That made the native text-input target less stable than in a normal GPUI window. IME text is delivered through the window's first responder, which must implement `NSTextInputClient`. After the Quick Terminal show/configure path, the repair code initially tried to restore focus by selecting the first content subview, but that subview was a plain `NSView`, not GPUI's `NSTextInputClient` view. That changed the first responder from `GPUIView` to `NSView`, so printable text and IME commits had no text-input target.

The resign-key observer also treated all key-window loss as an external click, even though some IME candidate/composition flows temporarily move key focus before marked text is established.

Doubao appears to rely on that stricter AppKit text-input lifecycle, while other input methods tolerate the transient focus path.

## Fix applied

- Restore only the GPUI view that actually implements the `NSTextInputClient` selectors as first responder after Quick Terminal configuration and every slide-in.
- Detect active marked text on the current first responder.
- Delay Quick Terminal auto-hide while IME marked text is active, then re-check after composition settles.
- Treat `NSWindowDidResignKeyNotification` as an auto-hide signal only when Con has actually deactivated or another Con window has become key. If Con is still active and no other app window is key, restore the GPUI view as first responder so IME candidate-window focus handoff does not interrupt composition.

## What we learned

Quick Terminal's native shell around GPUI must preserve AppKit text-input invariants, not only GPUI focus state. For IME compatibility, a borderless transient hotkey window still needs a stable `NSTextInputClient` first responder, and focus-loss auto-hide must distinguish real user deactivation from IME composition handoff.
