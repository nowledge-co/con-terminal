# Pane Runtime Observer

This document keeps the original name, but the current implementation direction in con is a reducer-backed pane runtime tracker.

## Why this exists

A terminal pane is not a single process.

For serious workflows, the visible runtime is usually a stack:

1. local login shell
2. ssh client
3. remote shell
4. tmux or zellij
5. another shell
6. an agent CLI, vim, htop, less, or a long-running program

The current agent context model is still mostly a snapshot:

- title
- cwd
- recent output
- last command
- PTY input generation
- last completed command boundary
- a small amount of pane-mode inference

That is enough to avoid some bad mistakes, but it is not enough to answer the real question:

`what is this pane actually running right now, and how certain are we?`

This document defines the long-term architecture for that answer.

## Design goals

- Model the visible runtime of a pane, not just shell metadata.
- Separate backend facts from product inferences.
- Represent nested scopes explicitly.
- Preserve confidence and freshness with every claim.
- Work from Ghostty facts without rebuilding another terminal engine.
- Support external agent CLIs without hijacking them.
- Degrade honestly when the platform cannot provide strong evidence.

## Non-goals

- Perfect remote introspection without cooperation from the remote machine.
- Regex-based certainty about every TUI.
- Ghostty-specific hacks that pretend the C API exposes more than it really does.
- Hiding uncertainty from the user or the agent.

## Core rule

The system must never present shell-derived metadata as if it were the visible foreground runtime unless the evidence says that is still true.

## Architecture

The design is a three-layer model.

### Layer 1: Backend facts

These are raw observations with minimal interpretation.

Examples:

- terminal title
- OSC 7 pwd
- shell integration presence
- alternate-screen state
- command-finished events
- visible screen text
- local PTY foreground process-group ID
- local PTY slave name

This layer answers:

`what did the backend actually observe?`

It does not answer:

`what app is definitely running?`

### Layer 2: Pane runtime tracker

This is a stateful reducer that consumes facts and actions over time and produces a runtime model.

It is responsible for:

- evidence aggregation
- action-history aggregation
- freshness tracking
- scope detection
- conflict resolution
- confidence scoring
- invalidation when the foreground runtime changes

This layer answers:

`given all recent facts, what runtime stack is most defensible?`

### Layer 3: Consumers

Consumers include:

- the built-in agent prompt
- `list_panes`
- tab and sidebar labels
- notifications
- approval UI
- future session restore and resume surfaces

Consumers must receive structured runtime state, not re-run their own heuristics on raw text.

## Data model

### `PaneObservationFrame`

An immutable observation snapshot emitted by a backend adapter.

Current implementation in con:

- `title`
- `cwd`
- `foreground_process_group_id`
- `tty_name`
- `recent_output`
- `last_command`
- `last_exit_code`
- `last_command_duration_secs`
- `support`
- `has_shell_integration`
- `is_alt_screen`
- `is_busy`
- `input_generation`
- `last_command_finished_input_generation`

Suggested fields:

- `pane_id`
- `observed_at`
- `backend`
- `pty_child_pid`
- `title`
- `pwd`
- `shell_integration`
- `command_finished`
- `alt_screen`
- `screen_excerpt`
- `screen_hash`
- `size`

### `Evidence`

Every non-trivial claim must carry evidence.

Suggested fields:

- `source`
- `observed_at`
- `strength`
- `freshness`
- `value`
- `note`

Suggested sources:

- `pty_foreground`
- `pty_child`
- `shell_integration`
- `command_line`
- `surface_state`
- `ghostty_action`
- `cwd_artifact`
- `user_label`
- `manual_override`

### `PaneRuntimeState`

The durable observer output for one pane.

Current implementation in con:

- `front_state`
- `mode`
- `shell_metadata_fresh`
- `remote_host`
- `agent_cli`
- `tmux_session`
- `last_verified_scope_stack`
- `last_verified_tmux_session`
- `shell_context`
- `shell_context_fresh`
- `active_scope`
- `evidence`
- `scope_stack`
- `recent_actions`
- `warnings`

### `ScopeStack`

A pane should expose nested scopes instead of a single label.

In the shipped reducer, there are now two scope stacks with different meaning:

- `scope_stack`: current verified foreground stack only
- `last_verified_scope_stack`: last shell frame con verified through a typed shell probe

This is intentional.

`last_verified_scope_stack` is historical orientation, not live foreground truth.

## Shipped tmux attachment

con now uses that distinction to unlock the first native tmux control path safely.

When all of these are true:

- the current front state is a proven shell prompt
- the shell context is fresh
- the shell probe confirms tmux in that shell frame

con can treat tmux as a native protocol attachment instead of a raw visible TUI.

That allows:

- listing tmux panes and windows
- capturing a chosen tmux pane by target id
- sending tmux-native keys to a chosen tmux pane

This is materially different from outer-pane `send_keys`.

Suggested scope kinds:

- `LocalShell`
- `SshConnection`
- `RemoteShell`
- `Multiplexer`
- `Shell`
- `InteractiveApp`
- `AgentCli`

Suggested app kinds:

- `Tmux`
- `Zellij`
- `Vim`
- `Neovim`
- `Less`
- `Htop`
- `Top`
- `Unknown`

Suggested agent CLI kinds:

- `KnownAgent`
- `Unknown`

Example:

```text
[
  LocalShell(zsh),
  SshConnection(host=prod-2),
  RemoteShell(zsh),
  Multiplexer(kind=tmux, session=deploy),
  Shell(bash),
  AgentCli(kind=KnownAgent)
]
```

That is the abstraction the product actually needs.

## Strong signals vs raw observations

### Strong signals

These can justify high-confidence claims.

#### Ghostty action and screen facts

The public libghostty C API already gives us strong pane facts:

- title
- working directory
- command-finished events
- process-exited state
- foreground process-group ID
- PTY slave name
- visible text
- scrollback text

These are strong facts about terminal state.
The process-group ID is not direct foreground-process identity.

#### Foreground runtime facts

Ghostty exposes the local PTY foreground process group through `ghostty_surface_foreground_pid` and the PTY slave name through `ghostty_surface_tty_name`.
Despite the upstream function name, the former is a job-control process-group ID, not necessarily the PID of the exact process drawing the visible UI.

Con records these values as backend facts without promoting them to executable, shell, TUI, SSH, or tmux identity.

#### Shell integration events

OSC 133 provides prompt and command lifecycle semantics.

Useful facts:

- whether the shell is active
- when a command started
- when it finished
- whether command metadata belongs to the visible runtime

This is strong for shell state, but it is not sufficient to identify the full nested scope stack by itself.

#### Alternate screen

Alternate-screen entry is a strong signal that the visible runtime is no longer an ordinary shell prompt.

It should invalidate shell-metadata freshness for visible-app claims.

Current embedded Ghostty note:

the runtime model is ready for this signal, but the public embedded surface API does not export it yet. con therefore treats alternate-screen support as an explicit backend capability, and it is currently false on Ghostty panes.

### Raw observations

These remain valuable to the model, but con does not promote them into typed runtime state.

#### Screen text and structure

Examples:

- tmux status bars
- boxed layouts
- agent CLI banners
- vim-like rulers

These stay in `read_pane` / prompt output as raw observations. They do not create `PaneRuntimeState` facts.

#### Title

Titles are useful for human inspection, but they are not authoritative foreground-runtime identity.

#### Filesystem artifacts

Examples:

- agent-specific local metadata
- agent-specific workspace metadata
- `AGENTS.md`

These can explain why a tool might be in use, but they do not prove the pane is running it.

## Backend adapters

## Ghostty backend

Ghostty currently gives us:

- title via action callback
- pwd via action callback
- command-finished via action callback
- foreground process-group ID via `ghostty_surface_foreground_pid`
- PTY slave name via `ghostty_surface_tty_name`
- visible and scrollback text via `ghostty_surface_read_text`
- selection access
- inspector handle

Important limit:

the embedded C API does not currently expose the same rich semantic prompt and runtime internals that Ghostty uses internally.

More concretely, the current embedded path does not export:

- authoritative foreground command text
- authoritative alternate-screen state
- authoritative remote-host identity
- exact foreground executable or argument identity

Important consequence:

we should not design the pane-runtime system around assumptions that Ghostty will tell us the exact foreground app or nested scope stack today.

Ghostty should feed Layer 1 facts. Layer 2 should remain a con-owned observer that merges those facts into a defensible runtime model.

## Ghostty-specific observations

Upstream Ghostty clearly maintains richer prompt semantics internally:

- semantic prompt state in `Screen.zig`
- prompt/output selection boundaries
- prompt-click movement
- command lifecycle handling in `stream_handler.zig`

But those semantics are not fully exported through the embedded C API today.

Also, Ghostty's OSC 7 handling validates the reported hostname against the local system before surfacing it as `PWD` state. That means `PWD` is not a durable source of remote host identity for embedded consumers.

This matters because a product design that depends on remote hostname coming from Ghostty `PWD` is structurally unsound.

Current con behavior reflects that limit:

- remote host identity is left `unknown` unless con has a stronger backend fact
- tmux and agent-CLI identity only enter typed runtime state through authoritative command-line or surface-state evidence
- pane titles and screen structure remain raw observations for the model, not runtime facts
- prompt and `list_panes` expose backend-support flags so the model can see when Ghostty cannot prove command text, alternate-screen state, or remote-host identity
- each tab now owns per-pane runtime observers that persist defensible facts across sparse frames and feed every consumer from the same state

## Probe design

The observer should run probes independently and merge their evidence.

### `GhosttyObservationProbe`

Purpose:

- build `PaneObservationFrame` from the embedded Ghostty surface
- keep title, cwd, command-finished, and screen excerpts synchronized
- expose only facts that libghostty actually exports today

### `ShellIntegrationProbe`

Purpose:

- track prompt-oriented metadata that Ghostty exposes indirectly today
- mark shell metadata freshness
- detect transitions back to the shell when strong shell evidence returns

### `TerminalSemanticProbe`

Purpose:

- consume backend-native signals such as command-finished, title updates, and future Ghostty semantic exports

### `ScreenStructureProbe`

Purpose:

- expose screen text and layout as raw observations for the model and the user

Constraint:

this probe does not create typed runtime facts.

### `RemoteContextProbe`

Purpose:

- accept only explicit remote-runtime contracts when they exist

Likely evidence:

- persistent SSH target
- user-confirmed labels
- future explicit integration markers

### `ManualLabelProbe`

Purpose:

- allow the user to name or confirm a scope when the platform cannot prove it

Examples:

- "prod deploy tmux"
- "agent task on staging"
- "logs tail pane"

Manual labels should never overwrite facts. They should layer on top of them.

### `GhosttyObservabilityContract`

Purpose:

- define the next upstream libghostty exports con actually needs
- avoid rebuilding a parallel PTY/process introspection stack beside Ghostty

High-value future exports:

- foreground process identity
- alternate-screen state
- richer semantic prompt lifecycle
- explicit remote/runtime markers for embedded hosts

## Freshness and invalidation

This is the part cheap designs usually miss.

The observer must invalidate stale metadata aggressively.

Examples:

- When a pane enters alternate screen, shell cwd and last command stop being trusted for visible-app claims.
- When the foreground process group changes from `zsh` to `tmux`, shell prompt assumptions must be downgraded immediately.
- When the foreground process group returns to a shell and OSC 133 prompt markers resume, shell metadata can become fresh again.
- When the pane is inside `ssh`, remote runtime identity must stay `unknown` unless supported by stronger evidence.

## Agent-facing contract

The built-in agent should not reason directly from raw title, cwd, and output.

It should receive a structured summary such as:

- `scope_stack`
- `active_scope_kind`
- `remote_host`
- `multiplexer_session`
- `agent_cli_kind`
- `shell_metadata_fresh`
- `screen_mode`
- `confidence`
- `warnings`

Example warning:

`Visible pane appears to be inside tmux. cwd and last_command may describe the underlying shell, not the visible program. Inspect the pane before making claims.`

## Product implications

This architecture is not only for prompts.

It also enables better product surfaces:

- clearer pane badges
- better sidebar names
- safer approval copy for remote operations
- accurate notifications from external agent CLIs
- reliable resume state when returning to a workspace

## Implementation plan

### Phase 1

Shipped in con:

- `PaneObservationFrame`, `PaneEvidence`, `PaneRuntimeState`, and `PaneRuntimeObserver`
- per-tab observer maps keyed by `PaneId`
- shared observer output consumed by agent context, `list_panes`, sidebar naming, and smart-input remote classification
- Ghostty command-boundary tracking (`input_generation` vs `last_command_finished_input_generation`) for shell freshness
- explicit observation-support flags surfaced to the prompt and `list_panes`
- stateful retention for authoritative tmux and external agent CLI identity, with explicit invalidation when a fresh shell returns
- no title- or screen-pattern heuristics promoted into typed runtime state

Current Ghostty embedded status:

- `foreground_process_group_id` and `tty_name` are available as macOS backend facts
- `support.foreground_process_group_id = true` on macOS and `false` on the portable backends
- `support.tty_name = true` on macOS and `false` on the portable backends
- `support.foreground_command = false`
- `support.alternate_screen = false`
- `support.remote_host_identity = false`

### Phase 2

- record executable identity only from explicit backend contracts
- avoid shelling out in the hot path for pane identity

### Phase 3

- integrate backend adapters:
  - Ghostty surface adapter
- unify freshness rules across all Ghostty fact streams

### Phase 4

- add external-agent CLI classifiers based only on explicit backend evidence
- expose runtime summaries in `list_panes` and agent context

### Phase 5

- add user-visible scope badges and manual labels
- use runtime scopes in approvals and notifications

## Testing strategy

- unit tests for evidence merge rules
- unit tests for freshness invalidation
- fixture-based tests for common scope stacks
- integration tests for:
  - local shell -> tmux
  - local shell -> ssh
  - local shell -> ssh -> tmux
  - local shell -> agent CLI
  - local shell -> ssh -> tmux -> agent CLI

## What this avoids

This design avoids three long-term failures:

1. believing process-wide environment variables describe the focused pane
2. over-trusting shell metadata when a TUI has taken over the screen
3. scattering app-specific heuristics across prompts, tools, and UI labels

That is the standard required if con wants real credibility in SSH, tmux, and external-agent workflows.

The next paired layer is the control plane: how con safely acts on those observed runtimes without confusing con panes, tmux panes, shell execution, and TUI input. con now ships the first typed `PaneControlState` layer on top of the observer, and the longer-term design remains in `docs/impl/agent-runtime-control-plane.md`.
