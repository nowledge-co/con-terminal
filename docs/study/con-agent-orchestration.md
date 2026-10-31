# Agent orchestration through con

Status: distributable skill plus additive explicit-report observation. Existing
pane/surface semantics, terminal layout, and harness authority are preserved.

## Reference and direction

The [rex-claude skill](https://github.com/davila7/claude-code-templates/tree/main/cli-tool/components/skills/productivity/rex-claude)
provides a useful lifecycle: launch an interactive worker, send a task, wait for
a fresh program-status report, inspect blockers/results, then clean up. Its
wrapper is Claude-specific. Its strongest property is that completion belongs
to a new turn rather than matching old screen output.

con should provide one vendor-neutral `con-terminal` skill, with agent-specific
behavior discovered from installed tools rather than copied CLI flags. The
initial package is [`skills/con-terminal/`](../../skills/con-terminal/SKILL.md), with optional
worker and handoff references. It is self-contained after installation; it does
not depend on contributor docs or another locally installed skill.

The skill covers shell work, interactive external workers, the existing
built-in agent, and context handoff as distinct paths. A user can request a
review, parallel implementation in worktrees, test monitoring, or continuing a
Codex session in another supported agent without confusing those operations.
It preserves the host agent's delegation rules and the user's authorization.

## Preserve the existing object model

Keep the tab → pane → surface hierarchy. A pane is a visible rectangle; a
surface is a live session. A single worker pane with several surfaces gives
parallel agents room without turning the user's terminal into a grid of tiny
splits. Hidden surfaces still run, while users can focus a particular result.

Keep `panes.*` acting on the active surface. Changing that would silently alter
built-in tools, benchmarks, and existing integrations. External orchestration
should opt into `surfaces.*` and retain the returned surface IDs.

The `surfaces split` name is defensible: it resolves the source's host pane,
splits the layout, and returns the new pane and surface together. No rename or
second top-level worker/job abstraction is justified for the initial skill.

## Capability assessment

| Need | Current support | Consequence |
| --- | --- | --- |
| Machine-readable discovery | `identify`, `capabilities`, `tree`, list methods | Use the running app's method inventory; tolerate older builds |
| Explicit worker addressing | Tab position + stable pane/surface IDs within that tab | Good for live sessions; revalidate when tabs change |
| Layout vs session separation | `surfaces split` vs `surfaces create` | Already supports visible and hidden workers |
| Visible guarded shell execution | `panes exec`, runtime capability checks | Preserve guards; never type shell text into an unknown TUI |
| Interactive input/output | Surface read, literal text, named keys | Useful across agents, but delivery does not prove submission |
| Terminal initialization | `surfaces wait-ready` | Requires live terminal and shell integration; not TUI input readiness |
| Program-reported status | `surfaces status`, `panes status`, `surfaces list`, `tree` | Explicit evidence and exact records, including hidden workers |
| Built-in agent | One existing conversation per tab via `agent ask` | Serialize; do not treat it as a fresh independent worker |
| Context transfer | Revisioned durable `handoff` jobs, currently macOS | Separate from fresh workers and parallel editing |
| Report observation | `surfaces wait-status --after CURSOR` | Fresh exact-record matching; not a causal task receipt or success guarantee |
| Recovery | Every surface's layout/metadata and optional screen text; new terminal processes | Explicit native references support opt-in macOS resume; live processes are not preserved |

Source checked: `crates/con-cli/src/main.rs`, `crates/con-cli/src/handoff.rs`,
`crates/con-core/src/control.rs`, and app workspace `control_requests.rs`,
`control_surfaces.rs`, `control_agent_tools.rs`, `terminal_status.rs`.

## Implemented observation contract

`ProgramObservation` exposes only validated OSC 7501 records, not UI heuristic
activity. Each live terminal incarnation receives a unique UUID. Its cursor
combines that incarnation with the shared reducer's monotonic sequence. Records
carry their own accepted-report sequence, so unrelated child updates cannot
refresh a stale root completion. Repeated identical reports advance; notification
acknowledgement does not. Clear and reset never rewind the sequence.

`surfaces wait-status` requires a baseline cursor captured before input, selects
an exact record ID, and defaults to `done`, `blocked`, or `error`. Custom states
are explicit. It returns a match, timeout, or process exit; missing/replaced
targets and invalid/future cursors are errors. It captures the tab's internal
identity and surface ID once, preventing tab reordering from redirecting an
outstanding wait. New queries still use positional tab selectors.

The backend already reduces reports into bounded snapshots. Preserving its
sequence through those snapshots adds no parser, FFI callback, event queue, or
render-loop polling. App status requests drain the existing collector; waits
sample outside rendering with a bounded deadline. This queries current records,
not every historical event, so overwritten/evicted intermediate states can be
missed. No claim links a report causally to one submitted prompt. Submission
readiness, permission approval, task receipts, and successful work remain
separate concerns.

## Deferred ergonomics

### Routing and launch ergonomics when supported by usage

Tab selectors currently use a mutable 1-based index. A stable tab/window target
or explicit origin context would avoid retargeting after user tab changes.
Inspect multi-window dispatch and compatibility before choosing this contract.
Do not claim that stable pane IDs alone solve tab routing.

A surface `cwd` plus structured argv could avoid shell-string quoting and make
agent launches more portable. This should be additive and must preserve visible
execution, normal permissions, and the existing startup lifecycle. Do not put
agent-specific launch flags into con's generic surface abstraction.

The `owner` label and closeable-pane flag support cleanup, not isolation or a
lease. Only introduce stronger ownership if concurrent orchestrators show a
real conflict; don't advertise a label as an enforcement mechanism.

## Distribution and maintenance

The directory and skill identifier are `con-terminal`, because `con` is a
reserved Windows device name. The product remains styled `con`.

The standard `skills/con-terminal/SKILL.md` layout is discoverable by
[the skills CLI](https://github.com/vercel-labs/skills/blob/main/docs/creating-skills.md).
After these files reach the public default branch, users can install just this
skill with:

```sh
npx skills add nowledge-co/con-terminal --skill con-terminal
```

The installer selects supported agents; optional `-a codex` or `-a claude-code`
limits destinations, and `-g` chooses user scope. No separate npm package or
Claude-only plugin is necessary. Publishing repository files does not itself
prove a skills.sh listing or ranking; verify installation from the public
revision before advertising availability.

Before publishing, validate frontmatter, install from a local path in an
isolated project for at least Codex and Claude Code, verify relative reference
files survive, and exercise the worker lifecycle against an isolated con
session. Keep a record of binary/revision/platform and distinguish CLI parsing,
live terminal transport, and actual agent inference coverage. Don't launch paid
agents merely to validate packaging.

Future CLI changes should recheck this skill's commands and capability claims.
Do not copy the entire CLI manual into the package. Keep the contributor E2E
skill separate: it tests con itself; this skill lets users work through con.

## Initial validation (2026-10-31)

- Skill frontmatter/scaffold validator passed; all local reference links resolve.
- `npx skills add ./skills/con-terminal --list` discovered exactly the intended skill
  skill. An isolated local-path copy installation for `codex` and `claude-code`
  preserved all three files byte-for-byte; no user/global skill was installed.
- All 21 command paths used by the skill were checked against the existing
  local `target/debug/con-cli` help. Capability and lifecycle claims were
  checked against source; this is documentation/packaging verification.
- No live worker or paid agent was launched. The previous isolated review
  endpoint was absent, so no live transport/agent-turn E2E result is claimed.
  Windows/Linux runtime behavior and public-repository installation remain
  unverified in that initial packaging pass. See the PR validation record for
  the subsequent explicit-report implementation and live coverage.

## Explicit-report implementation validation

- macOS linked app tests: 733 passed.
- Affected crate tests: CLI 26 plus one config integration test; core 224;
  Ghostty 35 plus six build-script tests; terminal 16. All passed.
- App plus companion CLI built with the pinned Zig 0.17.0; Rust formatting,
  docs manifest, and skill validation passed.
- A local reporter emitted real OSC 7501 through a native terminal in an
  isolated app bundle. The live CLI verified stale completion rejection,
  identical-report freshness, exact-record matching, hidden-surface isolation,
  blocked permission kind, fast completion before wait start, tab index shift
  during a wait, wrong-incarnation rejection, clear, and reset. No agent
  inference was launched. The fixture required a launched/rendering macOS app;
  a background standalone process had not initialized its terminal views.
- The con-terminal skill was installed locally for Codex and Claude Code.
  Its package names avoid Windows reserved device names. Windows/Linux runtime
  and public-main installation are not yet verified locally.

## Native conversation continuity after restart

The first implementation deliberately separates native conversation recovery
from keeping live agent processes running. Herdr's architecture distinguishes
these too: its background server owns detach/reattach, while native recovery
persists identity and uses an allowlisted resume adapter. References reviewed
at [`ee5bfb5`](https://github.com/herdrdev/herdr/tree/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde):
[`agent_resume`](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/agent_resume.rs),
[app resume](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/app/agent_resume.rs),
[restore](https://github.com/herdrdev/herdr/blob/ee5bfb5041bb3e4a49d591c72308aae3c81ecdde/src/persist/restore.rs),
and its native SessionStart integration assets. con implements its own adapters;
no Herdr source was copied.

The durable reference belongs to an individual terminal surface. A tab groups
panes; a pane can contain multiple surfaces with different native conversations.
The reference records exact agent/session/store identity, cwd, native history
path and a confirmed viewport size. Shared workspace profiles never export it.
Native history remains in the native agent's store, separately from con's
optional terminal screen replay. No screen parser, OSC activity record or process
observation can manufacture a native identity.

Each newly created terminal receives a fresh origin UUID and the exact app
socket in its environment before native shell creation. SessionStart hooks use
these inherited values and a monotonic report sequence; an old terminal cannot
silently attach to a different restarted app. Hooks ignore subagents and operate
as optional fail-open metadata reporting, without granting harness control.
Native hook trust and permissions remain native decisions.

Restored references remain available with automatic recovery off. When enabled,
recovery reserves each structural identity before launch, waits for a proven
local shell, and submits one visible allowlisted helper. Missing paths/history,
unknown adapters, remote terminals, busy TUIs and uncertain input delivery do not
fall back to a new conversation. The helper checks the exact native history and
runs the native CLI with the recorded store and cwd. It does not save or replay
arbitrary executable arguments, model prompts or permission bypass flags.

Current Codex has two relevant differences from Claude Code. The managed shared
daemon loses terminal-local hook context, so con resumes with `--no-daemon` and
rejects shared-daemon hook ancestry. Hook subprocesses can themselves detach
from the TTY, so opening `/dev/tty` is not a reliable client-context check. Also SessionStart can be
lazy until the next turn. Native process startup is therefore an independent
acknowledgement (`started`), not fake hook confirmation (`observed`). con never
sends an extra model request just to obtain that confirmation. See the
[official hook contract](https://learn.chatgpt.com/docs/hooks) for native event
and trust semantics; installed CLI help and live testing determine adapter
behavior for the versions below.

Inactive restored surfaces may initialize while hidden from a previously
confirmed physical viewport size. They discard those bootstrap bounds as current
layout and remain hidden until the ordinary canvas/frame transaction commits
real geometry. This exception is documented in the native-surface-coverage skill;
ordinary split/zoom control initialization still cannot use estimated geometry.

The UI keeps the terminal-first structure: native continuity appears in existing
hover information and exact-surface context actions, plus one opt-in General
setting. A saved reference does not replace live activity icons or claim that a
worker is still running. Background process hosting and remote recovery remain
separate future work.

Initial local verification uses macOS ARM64 with Codex CLI **0.166.0** and Claude
Code **2.1.314**, private native stores and genuine CLI-created conversations.
A localhost Responses/Messages fixture supplies model replies; no user native
profile, credentials or paid model backend is used. This does not certify
Windows/Linux, Intel hardware, other CLI versions or remote stores.

The live restart fixture verified an inactive Codex tab and hidden Claude Code
surface, exact native IDs after recovery, duplicate-identity suppression, rejection
of old origins and foreign helper callbacks, default-off/manual recovery, missing
exact history without replacement, and forgetting a link without deleting native
history. Codex confirmed the resumed ID through its real hook on the next test
turn. A hook outside terminal-local Codex ancestry could not reuse inherited
context. Light/dark settings and stacked-surface activation were inspected.
Application restart was exercised; the host machine was not rebooted.
