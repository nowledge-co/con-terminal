# Links inside mouse-aware TUIs

## What happened

A long Codex CLI sign-in URL could not be opened with Cmd-click in Con, and a
normal mouse drag could not select it for copying. The same URL was usable in
iTerm2.

## Root cause

Codex enables terminal mouse reporting. Ghostty deliberately sends ordinary
mouse gestures to the foreground TUI and only runs its local link hover and
selection paths when Shift escapes that capture. Con forwarded Cmd without
Shift, so Ghostty never marked the URL as a link; ordinary dragging belonged
to Codex rather than the terminal. This prevents link detection before URL
length or browser authentication can matter.

## Fix

Con queries Ghostty's actual mouse-captured state. Only while it is captured,
Cmd pointer gestures carry Ghostty's Shift escape modifier. Ghostty removes
that Shift for link matching, so Cmd still matches its normal link binding.
Outside capture, pointer modifiers are unchanged. Shift-drag remains the
standard way to select terminal text inside a TUI; Con's existing copy-on-
release and Cmd+C paths remain unchanged.

## What we learned

Host-side pointer forwarding must account for a TUI's mouse capture without
globally disabling it. A long wrapped URL is a useful test case, but the
decisive condition here is capture state rather than URL size.
