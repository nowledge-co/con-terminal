## What happened

After the dynamic input bar height change, pressing Shift+Enter in the command input grew the bar but made typed shell text invisible.

## Root cause

Shell input renders a colored single-line command overlay and hides the native `Input` text underneath to avoid double text. The auto-grow change allowed multiline shell input, but `command_overlay_runs` intentionally returns `None` for values containing `\n`. The native input text still became transparent for any non-empty shell input, so multiline values had no visible text renderer.

## Fix applied

Native shell text is now hidden only when the command overlay is actually present. Multiline shell input falls back to the native `Input` text, while single-line command overlay rendering keeps its previous behavior.

## What we learned

When a visual overlay replaces native text, the visibility condition must be tied to overlay availability, not just to input non-emptiness.
