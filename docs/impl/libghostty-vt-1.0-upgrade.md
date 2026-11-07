# libghostty-vt 1.0 integration

Tracker: [#548](https://github.com/nowledge-co/con-terminal/issues/548).
Integration: [PR #549](https://github.com/nowledge-co/con-terminal/pull/549).

## Source and scope

The official release is **libghostty-vt 1.0.0**, tag `vt-1.0.0`, peeled commit
`0fa3d577a5614150ff8e2c152de5fb9d5c6d2dec`. Its version cycle is separate from
the Ghostty application. The stable VT C API does not stabilize the full
macOS embedding API in `ghostty.h`.

con consumes the same immutable source on every platform: public
`wey-gu/ghostty@9a97075f932b6c0d524618ff09dc4d93a41aba2d`, the official tag
plus the existing opt-in OSC 7501 bridge and a comment distinguishing its
size-prefixed report from the stable VT report. The full embedding action
`SET_WINDOW_THEME` remains at upstream value 70; bridge actions follow it.
There are no new terminal-core changes in the carry. The previous official
1.4.0 security baseline is an ancestor of this tag (266 upstream commits later).

Official `src/termio/stream_handler.zig` still ignores `.program_status` at the
tag. Removing the public bridge would regress native program-status reports,
prompt/reset cleanup and synchronous ingress. [#444](https://github.com/nowledge-co/con-terminal/issues/444)
remains open until an official consumable source includes that complete
contract. This upgrade does not expand the fork exception, change GPUI, or
claim to complete portable animation/Unicode-placeholder trackers #534–#536.

Zig stays **0.17.0 exactly**. Git-source builds use upstream
`-Demit-lib-vt=true`; the source-distribution build instructions are not an
interchangeable Git build recipe. Existing initial-output restoration remains
an embedding-only hook applied to con's build copy, with the new nullable C
header annotations preserved. User shell input/history is not used to restore
screen output.

## Stable ABI migration

The previous development VT ABI cannot be linked to the new Rust bindings:

- Option enums gain an invalid-zero sentinel. Selection gesture, system and
  mouse-encoder keys also change; consumed values are checked against the
  **linked** library's `ghostty_type_json()` manifest.
- Selection/grid references, notification, progress, semantic-prompt,
  program-status and clipboard request records become fixed layouts without
  a leading `size`. Other extensible records retain their size field.
- Absent render foreground/background values now return `GHOSTTY_NO_VALUE`,
  preserving default-color fallbacks and transparent terminal backgrounds.
- Grapheme reads use `GhosttyCodepointBuffer` with pointer/capacity/length,
  rather than passing a bare scalar array through `void*`. Validate enum key
  **value types** as well as their numbers; the function prototype alone cannot
  express this change.
- `GhosttyStyle` becomes a 16-byte record containing 4-byte colors and a
  16-bit flag word. Underline uses bits 8–10. Size, alignment, offsets and
  styled render extraction are covered.
- Key/mouse setup functions are renamed and return `GhosttyResult`; UTF-8
  input takes `GhosttyString`. Failed setup never encodes a reused stale
  event. Output buffers use `uint8_t*`; scrolling checks its new result.
- Copy selection uses a temporary terminal formatter, preserving plain-text,
  trim and soft-line unwrapping. Its handle and allocation are released with
  the upstream allocator, including error paths.
- Search uses shifted data keys and dedicated next/previous selection functions;
  the former action-like set options are removed.
- Kitty pixels are read as a borrowed `GhosttyString`, copied while the
  terminal lock is held. Existing decoder/storage/placement limits remain.
- Full macOS embedding keeps its separate 88-byte bridge report and action
  layout assertions. Do not reuse the stable VT report for that callback.

The Windows stub mirrors the new signatures but deliberately cannot satisfy
ABI/runtime acceptance. Never use `CON_SKIP_GHOSTTY_VT`, a stub, or only
`cargo check` as evidence for an upgrade.

## Verification

`cargo test -p con-ghostty --features vt-tests` adds the real portable VT archive
to macOS tests alongside full embedding. Normal macOS application builds keep
the native backend. The macOS Intel workflow runs this feature; Windows/Linux
continue linked native VT tests in `ci-portable.yml`.

Local Apple Silicon linked checks pass 129 con-ghostty unit tests and 9 build
integration tests on the normal pinned-source path, including both ABI manifests
and grapheme output-key contracts. The filtered upstream C API suite passes
579 tests; the embedded program-status suite also passes. The full upstream
suite's debug stress run is separate evidence and must not be counted as passed
without completion. Keep its result, application checks and native CI status in
the PR rather than inferring them from the C API subset.

Record exact checks and gaps in the PR. Local Apple Silicon VT tests exercise
portable parser/FFI behavior, not Windows D3D, ConPTY, Linux compositor or Intel
desktop presentation.

## Future upgrades and rollback

Use the official VT versioned headers and packaging guide matching the exact
tag. Audit consumed function prototypes as well as manifest layouts; not every
ABI break is represented by a record size. Check the full embedding API
separately and retain the bridge only within its authorized scope.

Revert the source pin and all bindings/build adaptations together if rollback
is required. Mixed old/new archives or a platform-specific source pin are not
supported. Keep the 1.4 security fixes described in
[the prior audit](ghostty-1.4-upgrade.md).
