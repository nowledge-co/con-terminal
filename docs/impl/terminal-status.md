# Terminal status presentation

Status is presentation, not control-plane authority. It must not authorize a
tool, satisfy a shell wait, or replace the harness runtime tracker.

## OSC 7501 delivery status

The bounded model and retained details landed in #449 and #450. #451 adds
native delivery on all three backends, using the maintainer-authorized public
Ghostty pin on macOS. That bridge includes queued-report cleanup and stable
surface-incarnation validation; Con uses bounded synchronous IO ingress.
The bridge is not yet in official Ghostty; #444 tracks its replacement.
#451 is merged and the beta.122 notes include the feature. A linked build or
parser test alone is not live protocol acceptance.

SSH and tmux forwarding was exercised on 2026-10-09 on this macOS workstation
(OpenSSH 9.9, tmux 3.7c). A local pty master answered `OSC 7501 ; ?` and
recorded the bytes. OpenSSH carried the query, the report, and the reply with
`BEL` and `ST`, with and without a remote pty. tmux delivered a wrapped
sequence from a visible pane while `allow-passthrough` was `on`, including
when tmux ran on the far side of SSH. The capability reply returned only to
the pane that owned terminal input. With two panes on screen together, the
inactive pane's wrapped query reached the outer terminal and the reply arrived
in the active pane; the inactive pane timed out. A hidden pane with `on`
delivered nothing. A hidden pane with `all` delivered the query and the
report, and the reply did not return to that pane. Raw sequences were
discarded in every tmux configuration tried, including `all`. The user-facing
steps are in `docs/program-status.md`.

On this Mac the shared reducer covered working, blocked, working again,
done, acknowledgement, and a later error that remains after process exit
and an alternate-screen change. A 400-character Unicode message with an
emoji joiner and a direction override is shown as at most 180 characters,
keeps the joiner, and drops the override. Tests construct compact and
expanded status icons at a 24px slot and check that only working states
register a ring. They do not inspect painted glyphs. Theme helper tests
check label and icon sizes with configured UI fonts of 12px, 16px, and
24px; they do not verify the rendered hover label.

Eight thousand progress-changing reports on one id retain one record,
expose each latest progress value, and signal each changed presentation.
Repeating the final report does not signal another presentation change.
This is a deterministic reducer test, not a performance benchmark.

Windows and Linux execute the same sequence from terminal bytes in
`program_status_sequence_survives_malformed_input_and_alternate_screen`.
That test is headless. A physical Windows or Linux window was not
observed. #447 still tracks live typing, scrolling, and painting performance
against a baseline, plus the rendered visual matrix and platform acceptance.
#444 stays open until official Ghostty replaces the fork, and #443 stays
open with it.

## Ownership

- `con-process` reads bounded native process facts. Identity includes process
  lifetime and executable information; a PID or process-group ID alone is not
  an executable identity. Windows descendant evidence is not a foreground-job
  assertion.
- `con-core::terminal_status` reduces independent identity, title and
  progress observations. Generic title motion has a three-second lease; expiry
  means unknown, not completion. OSC 9;4 expiry remains backend-owned.
  PTY writes and long-lived shell commands do not establish agent activity.
- `con-core::program_status` stores OSC 7501 records for one surface.
  Ghostty parses and validates; Con does not keep a second parser. macOS
  receives the report, prompt start, and full reset as embedder actions from
  the pinned Ghostty fork. Windows and Linux receive the same facts from
  libghostty-vt callbacks. The support query is answered only on those paths.
  The macOS bridge is an unmerged upstream proposal. On 2026-10-09 the
  maintainer explicitly authorized a temporary, immutable pin of the public
  `wey-gu/ghostty` fork for three-platform support. This is a scoped exception
  to the official-source policy, not permission for local dependency patches.
  #444 remains open until a compatible official revision replaces the fork.
  Con uses the optional IO-thread ingress, not per-report UI mailbox actions:
  it copies bounded borrowed fields into retained Rust state and schedules a
  coalesced wake on the first pending update. The callback must not call
  Ghostty or UI APIs. Surface teardown joins IO before freeing its userdata.
  Callback ingress reduces reports and lifecycle events in order into at most
  256 records, rather than evicting old events from a FIFO. The UI takes one
  dirty snapshot per collection pass. Clear, prompt, reset and acknowledgement
  cannot be lost during a report burst. Ordinary input with no accepted OSC
  7501 reports creates no pending snapshot. Raw tool writes, failed writes,
  empty input and key releases do not acknowledge a completion. Committed user
  text does, including IME input. Each
  report replaces its record completely. `app` is read from the nearest
  ancestor id, and `build` does not cover `builder`. At most 256 records
  are kept, evicting the least recently updated. Nothing expires on a timer.
  A shell prompt or attached-process exit removes `working` and `blocked`
  records and keeps `idle`, `done`, and `error`. RIS clears the set and
  allows OSC 9;4 to contribute again. DECSTR and alternate-screen changes
  do not. `done` and `error` stay unseen until `acknowledge()` for that
  surface, which is a key delivered to it. Window focus and another
  surface becoming active are not acknowledgement. An identical completion
  does not become unseen again. Any accepted report suppresses OSC 9;4
  until RIS. An explicit record outranks title and progress heuristics for
  the scope it describes, and an idle root does not hide a blocked child.
  The activity snapshot uses the most severe record; its percentage is the
  only one shown. Activity and detail text are retained separately: a changed
  message invalidates the tooltip even when severity and percentage stay the
  same; an identical detail does not invalidate it. The rail hover card and
  the tab's tooltip identify the source pane and surface, and show the
  winning record as plain text: inherited app, state, blocked kind, percentage, title,
  and message. The words are the protocol values. The message is not
  read as approval or failure. Bidirectional and other hidden formatting
  characters are removed so they cannot reorder the label outside the
  grid. Joiners needed for emoji and script shaping are preserved. No separate
  notification is posted for a report. These facts never authorize the harness or complete a
  control-plane wait. The workspace drops the set when the terminal entity
  is closed or replaced.
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

Compact rail tiles keep a stationary brand icon (at most 14pt) inside a 24pt
ring in a 32pt slot. Busy-without-percentage uses a 90-degree monochrome arc
(solid foreground, faint track), with a two-second period. Determinate progress
uses the same ring, with no underline. Attention, error, pause and completion
replace the identity icon with one centered 18pt Phosphor glyph. They have no
ring, corner badge, or animation: only running work gets a progress treatment.
Selection/tab color uses an inset left indicator and unread yields to semantic
badges. The compact tile fill is 40pt square, leaving room for the
indicator inside the fill while the 24pt ring stays centered on the 44pt rail.
Expanded/horizontal tabs replace the identity icon within the original-sized
slot: busy/progress rings or a semantic glyph, without moving the title.
An unacknowledged program-status completion uses that same static glyph
slot with a check. It does not schedule an animation.

Status hover text groups program identity, the readable protocol state and
progress, and literal title/message on separate lines. Pane/surface provenance
comes last. Build this bounded presentation only when retained facts change;
never parse message text or collect terminal state on animation frames.

Each chrome group has one `TabActivity` entity with a synchronized GPUI animation
driven by display frames. Paths use actual arcs, not polygon approximations.
Do not apply a low-frequency timer cap: coarse angular steps make continuous
motion visibly jerky. Reduced motion uses a static full ring. Visible windows
continue animating even when inactive; hidden windows and clipped markers do not
qualify for continued animation. On macOS, GPUI visibility includes full window
occlusion, minimization and app hiding. Each activity layer observes visibility
changes so uncovering restarts animation even if a hidden render retired its
last frame callback, without activation or terminal output. Window activation
still invalidates chrome for focus changes. Status collection continues
independently; removing a progress signal is not proof of successful completion
and must not generate a completion/unread event.

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

These contracts refer to the locked `gpui-pre 0.3.7` (Zed `1a28cff`) and
`gpui-component 0.7.0`, not moving upstream main:

- [`request_animation_frame`](https://github.com/zed-industries/zed/blob/1a28cff/crates/gpui/src/window.rs#L2605-L2629)
  notifies the current view. Rendered ancestors become dirty too; this overlay
  is not a paint-only invalidation mechanism. `observe_self` observes explicit
  workspace notifications, not every ancestor redraw.
- [`AnyView::cached`](https://github.com/zed-industries/zed/blob/1a28cff/crates/gpui/src/view.rs#L421-L525)
  uses the supplied style as its layout contract. Bounds, mask and text style
  participate in reuse; parent opacity does not. Keep both dimensions constrained
  on the cache shell. Its contents are laid out as an independent root with
  those bounds as available space, not inherited styles: a horizontal-filling
  view such as `InputBar` must also declare `w_full()` on its rendered root.
  Test both cached and normal parent layout, including shrink/grow resizes,
  rather than assuming the shell stretches an auto-width flex root.
  Bypass caching through the final transition frame. Do not cache the row
  registration subtree: a cache hit skips its canvas prepaint callbacks.
- [`with_max_fps`](https://github.com/zed-industries/zed/blob/1a28cff/crates/gpui/src/elements/animation.rs#L451-L469)
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
