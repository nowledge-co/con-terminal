# Terminal status presentation

Status is presentation, not control-plane authority. It must not authorize a
tool, satisfy a shell wait, or replace the harness runtime tracker.

## Ownership

- `con-process` reads bounded native process facts. Identity includes process
  lifetime and executable information; a PID or process-group ID alone is not
  an executable identity. Windows descendant evidence is not a foreground-job
  assertion.
- `con-core::terminal_status` reduces independent identity, title and
  progress observations. Generic title motion has a three-second lease; expiry
  means unknown, not completion. OSC 9;4 expiry remains backend-owned.
  PTY writes and long-lived shell commands do not establish agent activity.
- `workspace/terminal_status.rs` owns live surface incarnations, asynchronous
  batches and tab aggregation. Closed/replaced surfaces and stale completions
  cannot update a new surface. Completions check revision as well as query and
  incarnation, so an A→B→A query transition cannot accept old work.
  Batches have a 300 ms minimum interval and a
  one-second backstop; screen fallback is bounded separately.
  Title events update only their surface's retained title evidence. Unrelated
  child PID churn does not reopen the screen-scan budget. Versioned native
  Claude executables under `claude/versions/<major.minor.patch>` are recognized
  as presentation evidence, not control-plane authority.
  Screen detection allows six 300 ms attempts followed by retries spaced
  2, 4, 8, 16, 32 and 64 seconds apart for slow startup. Successful detection
  stops retries. Only a new terminal/job/title classification/input observation
  reopens the budget; no background screen polling continues indefinitely.
  Ghostty's app-wide wake generation is not a per-surface output signal and
  must not be used to renew this budget.
- The host PTY bridge returns sequence-correlated process metadata through one
  bounded worker. Host PIDs must never be queried in the sandbox namespace.
  Missing/oversize metadata does not end the shell. Old bridges retain terminal
  I/O compatibility without this optional capability.
- Sidebar and horizontal tabs share retained identity/name data. All surfaces
  and built-in agent activity contribute to status. Severity wins first, then
  the focused surface, then tree order; percentages retain their source and are
  never averaged across panes.
  Built-in approval lifetime follows its request's channel identity, not FIFO
  completion order; stopping a session denies pending approvals. Panel activity
  changes notify aggregation without waiting for terminal output or polling.
  Approval-needed/ended events originate at the hook's actual wait boundary,
  after its request-time auto-approval policy. End events match both channel
  and call ID. Clearing/truncating a conversation explicitly denies its pending
  approvals. Panel setters own conditional notifications so cached views update
  independently of their callers.

## Rendering contract

Compact rail tiles keep a stationary brand icon (at most 14pt) inside a 28pt
ring in a 32pt slot. Busy-without-percentage uses a 90-degree monochrome arc
(75% foreground, faint track), with a two-second period. Determinate progress
uses the same ring, with no underline. Attention, error and pause use static
warning/danger rings with Phosphor badges; the badge quadrant has no stroke.
Selection/tab color uses an inset left indicator and unread yields to semantic
badges. The compact tile fill is 40pt square, leaving room for the
indicator inside the fill while the 28pt ring stays centered on the 44pt rail.
Expanded/horizontal tabs replace the identity icon within the original-sized
slot: busy/progress rings or a semantic glyph, without moving the title.

Each chrome group has one `TabActivity` entity with a synchronized GPUI animation
driven by display frames. Paths use actual arcs, not polygon approximations.
Do not apply a low-frequency timer cap: coarse angular steps make continuous
motion visibly jerky. Reduced motion uses a static full ring. Inactive windows
retain a static arc; hidden windows and clipped markers do not qualify for
continued animation. Window activation explicitly invalidates chrome. Status
collection continues independently; removing a progress signal is not proof of
successful completion and must not generate a completion/unread event.

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

Focus changes reaggregate cached facts before rendering, rather than waiting
for the collector's backstop. This path never scans terminal screens or queries
processes. Identity changes invalidate direct-agent busy/idle reports only;
attention and generic title motion survive, without renewing motion's lease.

### GPUI snapshot contracts

These contracts refer to the locked `gpui-pre 0.3.6` (Zed `bcf6582`) and
`gpui-component 0.6.6`, not moving upstream main:

- [`request_animation_frame`](https://github.com/zed-industries/zed/blob/bcf6582/crates/gpui/src/window.rs#L2609-L2633)
  notifies the current view. Rendered ancestors become dirty too; this overlay
  is not a paint-only invalidation mechanism. `observe_self` observes explicit
  workspace notifications, not every ancestor redraw.
- [`AnyView::cached`](https://github.com/zed-industries/zed/blob/bcf6582/crates/gpui/src/view.rs#L421-L525)
  uses the supplied style as its layout contract. Bounds, mask and text style
  participate in reuse; parent opacity does not. Keep both dimensions constrained
  on the cache shell. Its contents are laid out as an independent root with
  those bounds as available space, not inherited styles: a horizontal-filling
  view such as `InputBar` must also declare `w_full()` on its rendered root.
  Test both cached and normal parent layout, including shrink/grow resizes,
  rather than assuming the shell stretches an auto-width flex root.
  Bypass caching through the final transition frame. Do not cache the row
  registration subtree: a cache hit skips its canvas prepaint callbacks.
- [`with_max_fps`](https://github.com/zed-industries/zed/blob/bcf6582/crates/gpui/src/elements/animation.rs#L451-L469)
  throttles animation notifications, not all renders. Synced repeating animations
  share phase; reduced motion stops their continuation scheduling.

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
