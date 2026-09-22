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
