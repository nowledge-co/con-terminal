# File Browser navigation accumulated file tabs

## What happened

Every file selected in the File Browser appended a file tab inside the editor pane. Browsing several files left all of them open.

## Root cause

File-tree navigation used the same permanent-open path as explicit file opens. Editor tabs had no temporary/kept state. Asynchronous reads also retained superseded requests as background tabs.

## Fix applied

- File Browser navigation uses one replaceable temporary tab per editor pane, identified by an italic title.
- Double-clicking a file tab keeps it. Editing or saving also keeps the file so navigation cannot discard changes.
- Selecting an already open file activates its existing tab and preserves its kept state.
- Replacement preserves the tab position and releases the old file's LSP resources. Images and read errors share the temporary slot.
- Superseded temporary reads are ignored. Explicit opens, terminal file links, and dropped images retain their permanent-open behavior.
- Maintainer review found two additional ordering cases: completion of a
  permanent open must keep an already-loaded temporary tab for the same path,
  and reselecting the active tab must cancel a pending browser navigation.
  Both are covered by regression tests.

## What we learned

Temporary navigation needs an explicit lifetime separate from both dirty-buffer state and Markdown preview mode. Request ordering must suppress obsolete tab creation as well as obsolete activation.

## Validation

On macOS, using the installed Rust 1.97.1 toolchain and Zig 0.16.0 selected through `CON_ZIG_BIN`:

- `cargo +1.97.1 check --locked -p con` passed.
- `cargo +1.97.1 test --locked -p con --bin con editor_` passed: 144 tests, including simulated single- and double-click events on the file tab header.
- `cargo +1.97.1 fmt --all -- --check` and `git diff --check` passed.

Maintainer follow-up on the final implementation:

- Both ordering regressions above were reproduced as failing tests before
  fixing them. Saving an unedited temporary file also has a regression test.
- All 679 app tests passed, along with strict Clippy and formatting checks.
- The full live `con-test` suite passed: 27 assertions across eight files,
  using an isolated HOME and control socket.
- Native light/dark checks confirmed double-click to keep and the italic/regular
  title distinction. Light-theme checks also confirmed replacement in place
  and protection of edited files.
- User-facing workflow and implementation docs were updated. Windows/Linux
  native CI now runs the editor test group as well as compiling it. Their
  runtime behavior was not tested locally; native CI must cover the updated
  head before merge.
