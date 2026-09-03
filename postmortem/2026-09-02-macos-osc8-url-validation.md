# macOS opened terminal-provided OSC 8 targets without validation

## What happened

The macOS terminal backend forwarded every Ghostty `OPEN_URL` action directly
to the operating system. OSC 8 output could therefore show a trusted-looking
label while opening a different target, including local files or custom URL
schemes, without Con exposing or validating the real destination.

## Root cause

Con reduced Ghostty's open-URL action to a `String` and discarded the action
kind that distinguishes untrusted OSC 8 output from ordinary detected links.
It also did not bind Ghostty's `MOUSE_OVER_LINK` action, so the app could not
show the terminal-controlled URI before a click.

## Fix applied

The macOS transport now separates OSC 8 opens from ordinary URL opens. Con asks
Ghostty to emit previews only for OSC 8 links, emits a hover event only when
the link under the pointer changes, and shows the canonical target in a
non-interactive preview.

OSC 8 clicks now scan the raw input for whitespace, control, bidirectional,
zero-width, and line-separator characters before URL parsing, since the parser
would otherwise strip or encode them. Con opens only explicit HTTP(S) URLs with
a host and no embedded credentials, plus non-empty email URLs, using the same
serialized value shown to the user. Other targets enter a persistent blocked
state rather than reaching the platform opener. The user may explicitly copy
only the escaped display value.

## What we learned

- URL safety checks must inspect raw input before WHATWG parsing because the
  parser removes tabs, newlines, and some zero-width host characters.
- Link provenance is part of the security boundary. Ordinary detected URLs
  keep their existing behavior; terminal-authored OSC 8 targets require a
  separate policy.
- Ghostty's hover payload has no link kind. Setting `link-previews = osc8`
  makes hover provenance unambiguous without changing click handling.
- Safe local-file opening requires more than executable mode bits or filename
  extensions. Denying OSC 8 `file:` targets is the conservative first policy.
