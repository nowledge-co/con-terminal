# Windows tmux Ctrl-Punctuation Prefix

## What Happened

Issue #148 reported that this tmux config worked in other terminals but not in
Con on Windows:

```tmux
unbind C-b
set -g prefix C-]
set -g prefix2 C-h
```

`C-h` worked, but `C-]` did not.

## Root Cause

Con's Windows and Linux preview terminal views translate GPUI key events into
terminal bytes themselves. That translation handled `Ctrl+A` through `Ctrl+Z`,
but it did not handle the defined ASCII control punctuation chords:

- `Ctrl+@` / `Ctrl+Space` -> NUL (`0x00`)
- `Ctrl+[` -> ESC (`0x1b`)
- `Ctrl+\` -> FS (`0x1c`)
- `Ctrl+]` -> GS (`0x1d`)
- `Ctrl+^` -> RS (`0x1e`)
- `Ctrl+_` -> US (`0x1f`)
- `Ctrl+~` -> RS (`0x1e`)
- `Ctrl+?` -> DEL (`0x7f`)

The surface control API also accepts the legacy `ctrl-2..8` aliases because an
orchestrator has no conflict with app navigation. Interactive Windows/Linux
keyboard input intentionally keeps unshifted `Ctrl+1..9` reserved for tab
selection; users can still send NUL through `Ctrl+Space` or shifted
punctuation such as `Ctrl+Shift+2` (`Ctrl+@`).

tmux treats `C-]` as `0x1d`, so Con's letter-only mapper meant the prefix never
reached tmux.

macOS was not affected because it sends keys through Ghostty's native key
pipeline instead of Con's portable VT key mapper.

## Fix Applied

Con now has one shared ASCII-control helper used by:

- Windows terminal key handling
- Linux terminal key handling
- the surface control API's `keys.send` parser

That keeps physical user input and orchestrator-driven surface input aligned.
The helper intentionally does not map shifted bracket variants like `Ctrl+}` or
`Ctrl+{`, so Windows/Linux app shortcuts such as `Ctrl+Shift+]` for tab
switching stay app-level.

One deliberate product boundary remains: the keyboard path does not map
unshifted `Ctrl+2..8` because `Ctrl+1..9` is Con's Windows/Linux tab-selection
gesture. The surface control API supports those aliases because automation can
target terminal bytes directly without stealing a human navigation shortcut.

This is the complete legacy C0 control-byte layer, not the complete modern
keyboard-protocol layer. Full parity with Ghostty on Windows and Linux should
eventually route portable key events through libghostty-vt's key encoder
(`ghostty_key_encoder_*`) so terminal state such as Kitty keyboard protocol,
modifyOtherKeys, keypad application mode, and fixterms stays owned by Ghostty
instead of being hand-maintained in Con.

## Copy-On-Select Note

The same issue asked about copy-on-select under tmux. This fix does not claim to
solve that larger workflow. Local Con selection can be copied through the normal
terminal copy action, and tmux mouse mode still requires terminal-level handling
or tmux/OSC52 clipboard integration. That should be tracked separately so we do
not hide a clipboard protocol gap behind the key-prefix fix.

## What We Learned

Terminal control-key support is not just letters. If Con owns key translation on
a platform, it must implement the complete legacy C0 control-byte layer and keep
those semantics shared with the control-plane surface API. For anything beyond
that layer, the long-term answer is not a larger Rust table; it is reusing
Ghostty's VT key encoder.

## Follow-Up (2026-09-06)

The long-term answer above landed in `07783391` (2026-08-26): Windows and
Linux key presses now go through libghostty-vt's `ghostty_key_encoder_*`
(`crates/con-ghostty/src/vt.rs`, `VtScreen::send_key`), so the Rust C0 table
described here no longer sits on the keyboard path. `ctrl_key_to_c0` survives
only for the control plane's `keys.send`, which writes raw bytes to the PTY.

One consequence worth knowing: the Ghostty bump to `492300ca` made the
encoder emit xterm `CSI 27 ; mods ; key ~` for Ctrl chords while an
application has enabled modifyOtherKeys=2 (`CSI > 4;2 m`). tmux only does
that with `extended-keys on`, so the default `C-]` prefix flow from #148 is
unchanged; the `key_encoder_tracks_modify_other_keys_and_extended_function_keys`
test in `vt.rs` pins both behaviors.
