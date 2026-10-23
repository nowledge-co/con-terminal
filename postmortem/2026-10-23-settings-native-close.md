# Settings native close destroys the window during approval

## Report

An Intel macOS 26.7 user reported that closing Settings after saving also
closed Con. The selected font name was truncated in the Appearance section.
The application exit has not been reproduced locally; no crash report is
available. Do not treat the report as a confirmed crash or this fix as proof of
its cause. Tracking: [#514](https://github.com/nowledge-co/con-terminal/issues/514).

## Code-level defect

The Settings native should-close callback called `request_standalone_close`.
That method both checked unsaved changes and called `Window::remove_window`.
It then returned true, allowing the platform to continue closing the same
window. GPUI removes a marked window when its update ends, so native resources
could be dropped while the platform was still asking whether to close.

The pinned GPUI implementation already removes the window in its native
on-close handler. A should-close callback must only decide whether to proceed.

## Fix

Separate native approval from explicit dismissal. The native callback uses the
existing unsaved-change guard without removing the window. Keyboard dismissal
still removes the standalone window; pending edits still show the existing
inline confirmation. Neither path requests application termination.

Font rows now give the searchable selector the content width and show the
complete selected family underneath, with wrapping. Provider selectors and
fallback selection retain their existing layout.

## Regression coverage and limits

Exercise native approval with a second window present, refusal for unsaved
changes, and explicit dismissal. Assert that approval leaves both windows alive
and explicit dismissal removes only Settings. Check selected long font names at
desktop and narrow widths. These tests do not reproduce AppKit on Intel macOS
26.7; that report still needs user confirmation or a crash diagnostic.

## Lesson

Keep lifecycle decisions separate from lifecycle mutations. A callback named
should-close must not call a command that destroys the object being queried.
