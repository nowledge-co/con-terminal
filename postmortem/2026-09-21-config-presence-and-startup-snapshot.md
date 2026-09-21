# Configuration presence and first-run snapshots

## What happened

Review found two P2 paths: a dangling configuration symlink could be treated as
a fresh profile and replaced by a lower-priority migration, and choosing defaults
after another process created a configuration could launch with inconsistent
app-wide and terminal settings.

## Root cause

`exists` and `try_exists` follow symlinks, so a missing target looked like an
absent configuration. First-run completion also reloaded configuration after
global keybindings and network clients had already initialized.

## Fix applied

Use symlink metadata for configuration presence and propagate lookup/read errors.
Preserve links and refuse defaults or migration until their targets can be read.
Recheck profile presence for both first-run choices. A changed profile requires
reopening the app; normal import uses the validated snapshot that was committed,
and the default choice retains the initial configuration.

## What we learned

Presence is different from readability for dotfile-managed profiles. Startup
configuration must remain one snapshot across global and workspace initialization.
Removing the post-import reload also removes redundant parsing and validation.
Regression tests cover dangling primary, previous-native and TOML links, restored
targets, and first-run state links; the pre-fix versions fail these tests.
