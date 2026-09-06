# macOS Copy-On-Select Lost Behind the Clipboard-Write Gate

## What Happened

Since `v0.1.0-beta.93`, selecting text in a macOS terminal pane no longer
copied it to the clipboard. Cmd+C still worked, so the regression looked like
a preference change rather than a bug and went unreported. It surfaced while
auditing the libghostty bump to `492300ca`, whose upstream `copy-on-select`
redefinition (ghostty-org/ghostty#12604) prompted a check of what Con actually
did with Ghostty's selection copies.

## Root Cause

Con has relied on Ghostty's macOS default `copy-on-select = true` since the
libghostty backend landed. Ghostty implements that by calling the embedder's
`write_clipboard` callback on left-button release with a two-item payload
(`text/plain` plus `text/html`) for the standard clipboard.

PR #331 hardened the same callback so that terminal programs cannot write the
clipboard through OSC 52 or the Kitty clipboard protocol unless the user opts
in. The new guard returns early when the clipboard-write policy is disabled
(the default) or when `content_count > 1`. Ghostty uses one callback for
application writes and for user-gesture copies and gives the embedder no way
to tell them apart, so the guard also discarded every copy-on-select payload.
Ghostty had already enforced `clipboard-write = deny` for OSC 52 before ever
calling the callback, which is why the Rust-side gate only ever rejected the
user's own selections in practice.

Nothing failed loudly: the callback's early return is silent, no test covered
copy-on-select, and the callback comment was rewritten from "selection, OSC 52"
to "plain text", hiding the dropped case.

## Fix Applied

- Ghostty's own copy-on-select is now disabled explicitly
  (`copy-on-select = none` in con-ghostty's runtime config) so the clipboard
  callback receives only application-initiated writes and can keep its strict
  gate. The same change pins `middle-click-action = ignore`: the GPUI host
  view never forwards middle clicks to Ghostty, so the setting is inert today,
  and pinning it keeps a future middle-button forwarding change from pasting
  as a side effect of an upstream default.
- Copy-on-select is implemented in Con's GPUI layer: when a left-button
  release ends a selection gesture with text selected, the view reads the
  selection through `ghostty_surface_read_selection` and writes it with the
  same `cx.write_to_clipboard` path Cmd+C uses. Only left releases copy, the
  behavior Ghostty itself uses, and Ghostty clears the selection on a plain
  left press, so clicking a pane to focus it never republishes stale text.
- A unit test pins the two config lines so a future Ghostty default change or
  callback edit cannot silently reintroduce the coupling.

## What We Learned

Ghostty's embedder callbacks carry more than one trust level over one wire.
Any gate added for application-initiated traffic on `write_clipboard` also
applies to user gestures unless the runtime config stops Ghostty from routing
those gestures through the callback in the first place. Own the user-gesture
path on the GPUI side, keep the callback for application writes, and state the
split in the config that enforces it.

Behavior inherited from an upstream default is unowned behavior. It cannot be
tested without noticing it exists, and it changes when upstream changes. Every
Ghostty default Con depends on for user-visible behavior should be written into
the runtime config with a test, as `shell-integration-features` and
`link-previews` already were.
