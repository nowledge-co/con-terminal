# Terminal status presentation

Status is presentation, not control-plane authority. It must not authorize a
tool, satisfy a shell wait, or replace the harness runtime tracker.

## Ownership

- `con-process` reads bounded native process facts. Identity includes process
  lifetime and executable information; a PID or process-group ID alone is not
  an executable identity. Windows descendant evidence is not a foreground-job
  assertion.
- `con-core::terminal_status` reduces independent identity, command, title and
  progress observations. Generic title motion has a three-second lease; expiry
  means unknown, not completion. OSC 9;4 expiry remains backend-owned.
- `workspace/terminal_status.rs` owns live surface incarnations, asynchronous
  batches and tab aggregation. Closed/replaced surfaces and stale completions
  cannot update a new surface. Batches have a 300 ms minimum interval and a
  one-second backstop; screen fallback is bounded separately.
- The host PTY bridge returns sequence-correlated process metadata through one
  bounded worker. Host PIDs must never be queried in the sandbox namespace.
  Missing/oversize metadata does not end the shell. Old bridges retain terminal
  I/O compatibility without this optional capability.
- Sidebar and horizontal tabs share retained identity/name data. All surfaces
  and built-in agent activity contribute to status. Severity wins first, then
  the focused surface, then tree order; percentages retain their source and are
  never averaged across panes.

## Rendering contract

Brand icons remain still. Busy-without-percentage uses a subtle activity line;
determinate progress, error and attention use static lines. Each chrome group
has one `TabActivity` entity with a synchronized, capped 24 Hz GPUI animation.
Reduced motion disables the pulse. Hidden windows and clipped markers do not
qualify for continued animation.

Rows register marker geometry during prepaint. Keep the previous geometry until
the next prepaint: discarding it during parent rendering makes offscreen rows
look newly unmeasured and eligible for another animation. Only the registered
prefix belongs to the current frame; removed rows must not paint. A visibility
change schedules reevaluation after layout.

Animation ticks must not prepare pane/model/history data, query processes, or
read screens for summaries. Workspace notifications invalidate retained chrome
inputs; focus changes also invalidate them. Input and agent panel view caches
have externally constrained dimensions and are bypassed throughout transitions,
including the final settling frame. New mutation paths must explicitly notify
the appropriate entity rather than relying on an unrelated parent render.

## Verification

Run `cargo test --locked -p con --bin con` and
`cargo test --locked -p con-core -p con-process -p con-cli` on macOS with Xcode
selected. Cross-target `con-ghostty --tests` checks with
`CON_SKIP_GHOSTTY_VT=1` validate Rust types only, not native linking or execution.

For live checks, use the dedicated socket workflow in `con-cli-e2e.md`. Exercise
indeterminate and percentage OSC 9;4 states, attention, pane/tab removal, clipping,
panel transitions and reduced motion. Trace target `con::activity` exposes
activity-layer renders, chrome preparation, process batches, identity screen
scans and summary polls. Compare their cadence during sustained Busy activity;
frame-rate rendering must not imply frame-rate data collection.

The local implementation validation includes macOS window captures, reducer and
marker tests, a marker-retention removal experiment, and a sustained-activity
trace. It does not establish Windows/Linux GUI behavior or a 100-tab/long-chat
performance budget; those require native runtime and representative load tests.
