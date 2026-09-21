# Concealed terminal text was rendered on Windows and Linux

## What happened

The portable terminal renderers displayed cells marked invisible by SGR 8.
Underline and strikethrough decorations also remained visible. The macOS
embedded Ghostty renderer does not use this snapshot conversion path.

## Root cause

`read_cell` received `GhosttyStyle.invisible` from libghostty-vt but did not
preserve it in `Cell.attrs`. Both portable renderers therefore painted the
original codepoint and decorations without knowing the cell was concealed.
This was a lost rendering semantic, not an FFI layout mismatch.

## Fix applied

Preserve the invisible attribute in the existing attributes byte. At each
renderer boundary, `Cell::for_render` returns a copy with concealed glyphs,
underlines, and strikethroughs removed, matching Ghostty/xterm semantics.
Colors and inverse remain intact for background, selection, and cursor paint.
The Windows renderer reuses its blank-cell path without rasterizing the glyph.

Do not blank the stored snapshot: transcript and link extraction read its
codepoints. Conceal affects presentation, not copying or searching, and is not
a mechanism for removing secrets from terminal memory.

## What we learned

ABI compatibility alone does not ensure rendering fidelity. Attributes must
survive the full parser-to-renderer path. Regression cases cover SGR 8, SGR 28,
full reset, retained text, decorations, inverse, explicit colors, selection,
and cursor backgrounds. Keep the conversion at the paint boundary rather
than duplicating conceal rules in the two renderers or changing snapshot data.
