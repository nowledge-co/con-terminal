# Windows terminal font settings did not update live panes

## What happened

Changing the terminal font family or size in Settings updated the saved/app-level renderer configuration, but text in terminal panes that were already open did not change on Windows. Newly created panes used the new values.

## Root cause

The Windows `WindowsGhosttyTerminal::update_appearance` implementation intentionally ignored its `font_family` and `font_size` arguments and only forwarded theme colors and background opacity to the live `RenderSession`. The renderer's atlas rebuild API also accepted only a size, and the glyph cache discarded access to the bundled font collection whenever its current font came from the system collection. That made a complete live family switch impossible.

## Fix applied

- Forward font family and size updates to every live Windows render session.
- Re-resolve the requested family against bundled and system collections and rebuild all DirectWrite formats, metrics, primary face data, and glyph atlas state.
- Retain the bundled collection so live switching between system fonts and IoskeleyMono works in both directions.
- Recompute the VT/ConPTY grid after cell metrics change and force a repaint.
- Keep the latest logical font size for subsequent DPI changes.

## What we learned

App-level renderer configuration is only a template for future panes. Every appearance setting advertised as a live preview must also have a per-session update path, and font updates must treat family, collection, metrics, grid size, and DPI-scaled size as one operation.
