# Handoff review round-2 fixes

Date: 2026-09-25

## What happened

A six-area parallel review of the 41 unpushed Agent Handoff commits surfaced
seven MEDIUM findings. One (stale `con-cli handoff run` corrupting a healthy
job) turned out to be a misread — the stale `ensure!` already sits outside the
`launch_error`-recorded closure — but the other six were real edge-path bugs:

1. **Control plane misclassified server faults.** The generic
   `List/Get/Respond/Cancel/Prepare` branch mapped every error to
   `invalid_params`, so a store I/O fault surfaced to con-cli clients as
   -32602 (bad request) — the exact misclassification the Start gate's
   `classify_reserve_error` exists to prevent.
2. **Cleanup treated any probe error as "no export".**
   `remove_job_checked` used `symlink_metadata(...).is_ok()`, so a permission
   or transient I/O error deleted the private record while a possibly-live
   project export was left behind — the desync the function forbids.
3. **Kimi existing-tab failures stranded the panel.** The route persisted
   `launch_error` but never called back into the panel, leaving the dialog
   busy on "Sending handoff…" with a hardcoded, often wrong message.
4. **Tab detection under-detected wrapped agents.** `refresh_agent_cli_detection`
   passed the foreground *process group* id to the single-pid argv probe
   instead of `agent_from_process_group`, missing launcher-wrapped sessions.
5. **A failed save after a successful stage bricked retries.** The staged
   export made every `begin_*` retry fail on "staging already exists".
6. **ACP replay truncated silently.** `Reader::request` returned at the
   `session/load` result, dropping `session/update` events the server is
   allowed to stream afterwards.

## Fixes applied

- Shared `classify_reserve_error` between the Start gate and the generic
  control branch (re-exported via `route`); storage faults now map to
  `ControlError::internal`, request conflicts to `invalid_params`.
- `remove_job_checked` only treats `ErrorKind::NotFound` as absent; any other
  probe error retains both directories (regression test chmods the export
  parent to 0000).
- The Kimi route now persists the launch error with the real inner message and
  calls `report_launch_failure`, clearing `busy` and surfacing the card.
- Detection calls `agent_from_process_group` (leader + member scan).
- `store::stage` accepts a byte-identical existing export as an
  already-completed stage; edited/partial/extended exports still refuse
  (regression test stages twice, then tampers).
- `Reader::request` drains `session/update` events after the result until a
  500 ms quiescence or clean stream close; read/parse errors during the drain
  still fail the export (fake-server test emits an update 100 ms after the
  result).
- Pinned the stale-launch contract with a comment at the helper's `ensure!`:
  the refusal must stay outside the `launch_error` closure.

## Verification

`cargo clippy --workspace -- -D warnings` clean; `cargo test --workspace`
passes (953 tests, including the new regression tests).

## What we learned

- Review findings about control-flow scope ("the error flows into the
  handler") must be re-verified against the actual closure boundaries before
  writing a fix — one of seven was a misread, and the right action was a
  comment, not a change.
- "Transient error becomes permanent" was the recurring pattern (three of
  six real bugs): probe errors, failed saves, and missed panel callbacks all
  converted recoverable situations into stuck state. Error paths deserve the
  same idempotency review as happy paths.
