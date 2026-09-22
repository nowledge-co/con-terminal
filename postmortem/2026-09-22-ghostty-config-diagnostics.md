# Ghostty configuration errors lost their details

## What happened

Invalid macOS native configuration reported only a diagnostic count, leaving
startup, settings validation, and import errors without actionable parser messages.

## Root cause

`build_ghostty_config` queried `ghostty_config_diagnostics_count` but never called
`ghostty_config_get_diagnostic`, which already supplies formatted messages.

## Fix applied

Bind the upstream API with C/Rust ABI checks and append every diagnostic to the
existing error string before freeing the config. Do not free borrowed messages
individually. Regression tests cover multiple errors, source lines, Unicode include
paths, missing includes, and private-file cleanup.

## What we learned

Preserve upstream diagnostics rather than reconstructing parser errors. Root
locations describe Con's generated native input, not necessarily the original
`con.conf` line numbers; included-file locations retain their upstream provenance.

Follow-up review found that displaying randomized, already-deleted private file
paths obscured that distinction. Diagnostic location prefixes now identify the
generated layer explicitly; included paths and diagnostic bodies stay unchanged.

The Settings save-error banner sits outside the content scroll area. Multi-line
diagnostics therefore also require a bounded, scrollable banner so settings remain
reachable. A visual layout regression covers long errors at desktop and narrow
window sizes without truncating diagnostic text.
