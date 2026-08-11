# LazyGit Shift key input was encoded as a modified key

## What happened

On macOS, pressing `Shift+P` in terminal applications such as LazyGit invoked
the lowercase `p` action instead of the uppercase `P` action. Con forwarded the
physical key, generated text, and active Shift modifier to libghostty, but said
that no modifier had been consumed while generating the text.

## Root cause

GPUI represents the AppKit event as a physical key (`p`), active modifiers
(`Shift`), and generated text (`P`). Its `Keystroke` type does not carry a
separate consumed-modifier field, so Con has to reconstruct that value for
libghostty. The macOS bridge hard-coded `consumed_mods` to zero.

Libghostty therefore treated Shift as an effective modifier instead of the
modifier already used to translate `p` into `P`. Applications using enhanced
keyboard input, including LazyGit through tcell, received a modified-key event
rather than ordinary uppercase text.

## Fix applied

The GPUI-to-Ghostty bridge now marks Shift as consumed when GPUI provides
printable translated text. Control-character, textless, and fallback-text
events keep every modifier effective so combinations such as `Shift+Enter`
retain their terminal protocol meaning. Option also remains effective because
whether it participates in text translation depends on Ghostty's
`macos-option-as-alt` setting, which this GPUI event does not expose. A unit
test locks down these boundaries, including the `Shift+P` case represented by
consumed Shift.

## What we learned

Forwarding generated text and active modifiers is not sufficient for terminal
key events. The consumed-modifier mask is part of the event's semantics and
must follow the source platform's text-translation rules. When adapting a
platform event through a higher-level UI toolkit, compare every field against
the native terminal integration rather than defaulting metadata to zero.
