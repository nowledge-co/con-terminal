# Stable VT integration: ABI checks did not cover result semantics

## What happened

While integrating the official `vt-1.0.0` baseline for #548, linked ABI-manifest
checks passed but 24 of con's combined native/portable unit tests failed. Most
portable render snapshots were empty. This was caught locally before committing
or releasing the upgrade; it was not a shipped user regression.

## Root cause

The stable API changes more than record layouts and function names. Reading an
unset cell foreground/background now returns `GHOSTTY_NO_VALUE`, rather than
`GHOSTTY_INVALID_VALUE`. con's old fallback treated that result as a failed
snapshot. Default-color cells occur in almost every frame, so this also broke
Unicode, search highlights, selections, Kitty metadata and synchronized-output
checks that consume snapshots.

After restoring color fallback, the Unicode tests exposed another unchanged-key
contract: grapheme reads take a bounded `GhosttyCodepointBuffer` in 1.0, rather
than a raw scalar array. The bindings now use its pointer/capacity/length record
and check the linked manifest's key value type as well as offsets.

The new macOS portable-test path exposed two separate verification constraints:
Apple's linker could select the VT dylib with the static archive's basename,
and upstream Option-key handling differs from Windows/Linux for composed
non-ASCII text. Neither should be hidden by disabling a behavior test.

## Fix

- Migrate absent-color handling to the stable `NO_VALUE` contract while retaining
  default foreground/background and transparent-canvas semantics.
- Run existing behavior regressions against the real portable library on macOS,
  in addition to layout/enum checks. Add a styled RGB/underline extraction case.
- Stage the macOS static VT archive with a unique basename and inspect its final
  executable dependencies. No VT dylib is needed at runtime.
- Assert the actual upstream target-specific Alt/Option result; Windows/Linux
  retain their existing ESC-prefix assertion and native CI coverage.
- Keep repeated embedding-patch application from rewriting unchanged files,
  which otherwise repeatedly invalidated the native build during diagnosis.

## Lessons

A stable API's first release still requires migration from its development ABI.
A manifest can validate size/alignment/offsets and enum values, but cannot prove
fallback behavior, iterator use, allocator ownership or input semantics. Run
linked ABI checks first, then real copy/search/render/input/paste/status tests.
A macOS portable test is additional parser/FFI evidence, not Windows/Linux GUI
or input-stack acceptance. See [the integration audit](../docs/impl/libghostty-vt-1.0-upgrade.md).
