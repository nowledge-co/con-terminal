# Cursor handoff recommended an interrupted session with no exportable history

## What happened

Cursor session `ad6cf081-19e8-4f9c-88e8-d22ab65d711f` appeared as the recent
source but preparing a handoff failed with `Cursor CLI transcript contains no text`.
Its transcript contained one user message followed by `turn_ended: aborted`.

## Root cause

Local discovery trusted metadata and recent binding sorted by update time without
checking completed history. Export correctly discarded the aborted turn under
`docs/design/agent-session-handoff-contract.md`, but reported a generic empty-text
error. The file adapter's error also bypassed ACP entirely. Ordinary CLI chats can
be absent from ACP listing, so requiring list membership before load would still
prevent fallback for a locally verified identity.

## Fix applied

- Discovery performs one bounded transcript read and a lightweight scan. It keeps
  unavailable sessions visible with an optional, backwards-compatible
  `export_warning`; recent suggestions exclude them. UI selection shows the reason
  and blocks them, including explicit live bindings and implicit single-session
  selection. Local status takes precedence over a duplicate ACP list entry.
- File export reads the selected transcript once. JSONL/normalization failures
  carry the verified source identity into restricted ACP `session/load`, without
  requiring ACP list membership. A successful file export skips ACP. Failed ACP
  replay preserves both errors in the user-visible error string.
- Interrupted-only, unfinished, and truly empty histories have distinct errors.
  Completed-turn rules are unchanged. Known unfinished turns and storage/identity
  safety failures remain fatal rather than being bypassed by ACP.
- Unit coverage includes interrupted-only and empty history, unavailable recent
  candidates, invalid/unfinished preflight, parser-failure fallback, ACP success,
  empty replay, failure/timeout context, and UI selection. A shell ACP fixture
  accepts load but not list, exercising the real restricted reader. Existing
  failed-middle/final-turn regression tests remain intact.

## Live ACP evidence

On 2026-09-25, Cursor CLI `2026.09.23-86fc751` was started as `cursor-agent acp`.
The probe sent only `initialize` and `session/load` with the exact session ID,
repository cwd, empty `mcpServers`, and disabled filesystem/terminal capabilities.
Incoming tool/file/terminal requests were rejected and permission requests cancelled;
no prompt or authentication request was sent.

Sandboxed load returned `-32000 Authentication required`. Repeating outside the
sandbox with the existing native login returned `-32602 Invalid params`, with
`Session "ad6cf081-19e8-4f9c-88e8-d22ab65d711f" not found`. It produced zero
session updates and zero text characters. This session cannot be recovered through
the tested ACP path; that result does not establish behavior for all aborted sessions.

## What we learned

Discovery is not proof of exportable history, and a recent timestamp is not proof
of a completed turn. ACP can have different session visibility from CLI storage.
Keep export boundaries strict, present unavailable histories explicitly, and retain
both native-file and replay diagnostics when fallback cannot recover a session.

## Verification

All commands used `mise exec --`: `cargo build --workspace`,
`cargo clippy --workspace --all-targets -- -D warnings`, `just lint`, and
`cargo test --workspace` passed. Tests: **903 passed / 0 failed**, up from
894 by nine new tests. macOS compilation required running outside the sandbox
because clang writes its ModuleCache under the user's cache directory.

The original `con-cli handoff prepare --source-session ad6cf081-…` command now
fails with the interrupted-session reason followed by the ACP `session/load`
`-32602` failure. Discovery still lists that ID with
`export_warning: "Interrupted — no completed turns"`. No handoff bundle is created.
UI selection rules have unit coverage; visual interaction remains a manual check.
