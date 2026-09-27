# Cmd+C did not copy mouse-selected Codex TUI output on macOS

## What happened

Claude had become too restrictive for my workflow, so I returned to Codex. In Con Beta build 122, I found that I could select shell text and copy it with Cmd+C, but dragging over a Codex TUI response showed `ctrl+c copy` while Cmd+C left the clipboard unchanged. Pasting in another shell tab still worked.

## Root cause

Codex enables mouse reporting and owns the drag selection, so Ghostty has no terminal selection for Con to read. Con consumed macOS Cmd+C even when `has_selection()` was false, preventing Codex from receiving the Ctrl+C it uses to copy its own selection.

## Fix

Con remembers a completed left-button drag captured by the TUI. If Ghostty has no selection, the next plain Cmd+C sends one Ctrl+C to the TUI. A new left- or right-button gesture, scroll, or other key clears the pending copy. Losing focus or closing the terminal cancels the entire gesture, so switching tabs cannot leave a stale copy intent. Shell and Ghostty-owned selections retain Con's normal copy path, and the Edit menu's Copy action uses the same fallback.

After the change on `main` that copies hovered OSC 8 links, copy priority is: Ghostty text selection, a pending TUI drag selection, then a hovered OSC 8 link.

Codex also supports `tui.raw_output_mode = true` in `~/.codex/config.toml`, or `/raw` and `Alt+R` within a session. Raw mode offers scrollback that is friendlier to terminal selection, but changes the TUI interaction. Con's fix preserves Codex's default interaction and my Cmd+C habit. See the [OpenAI Docs configuration reference](https://developers.openai.com/codex/config-reference).

## What we learned and remaining limitation

A visible selection can belong to the terminal or to an interactive program. A copy shortcut cannot rely only on the terminal's selection state.

Con cannot tell whether an arbitrary mouse-reporting TUI actually selected text. After a drag with no Ghostty selection, the next Cmd+C may send Ctrl+C to a program that does not use it for copying. A new key, left- or right-button gesture, scroll, or focus loss clears this state to limit accidental forwarding.
