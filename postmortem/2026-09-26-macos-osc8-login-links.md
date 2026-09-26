# macOS OSC 8 links after modifier changes

## What happened

Long authentication links printed by Codex CLI could not reliably be opened
with Command-click in Con. Copying them by selecting the displayed rows was
also unreliable. Codex emits the complete URL as an OSC 8 hyperlink on each
wrapped cell, so the URL length was not the parser's limitation.

## Root cause

Con forwarded modifier changes to embedded Ghostty by resending the same mouse
position. Embedded Ghostty drops unchanged mouse-position callbacks, and its
core caches the last link cell. It therefore did not recompute hover when
Command was pressed after the pointer had stopped. Ghostty's native macOS
frontend instead sends a modifier key event, which explicitly refreshes link
state. Separately, Con's Command-C handler only copied terminal selections; it
had no way to copy the OSC 8 target when selection was impractical.

## Fix

Forward macOS modifier press and release events to the embedded Ghostty surface
using the native virtual keycodes, including a synchronization before mouse
clicks. Use Ghostty's reported OSC 8 target as the fallback for Command-C when
there is no selection. Existing selection copy remains first priority, and
blocked OSC 8 targets are not copied through this path. The hover card advertises
the shortcut.

## What we learned

An unchanged-position mouse event is not equivalent to a modifier event in
embedded Ghostty. Link behavior must follow the upstream frontend's event
semantics. For wrapped hyperlinks, copying the protocol target is more reliable
than reconstructing an address from terminal screen cells.
