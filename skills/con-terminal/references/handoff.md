# Continue context across agents

Agent Handoff is currently macOS-only. Check installed capabilities and local
help; do not assume every agent supports both history export and native input.

```sh
con-cli --json handoff agents
con-cli --json handoff sources --agent codex --cwd /absolute/path/project
```

Use an exact source session from discovery, not a title or the latest file in
an agent's history directory. Preparing a handoff exports filtered context and
a worktree snapshot; it does not start target inference:

```sh
con-cli --json handoff prepare --source-agent codex --target-agent cursor \
  --cwd /absolute/path/project --source-session EXACT_SESSION_ID \
  --goal 'Continue the remaining verification and explain any gaps.'
```

Inspect the returned job with `handoff get JOB_ID`. Review its preview and
warnings before starting; filtered export is not a guarantee that every secret
has been removed. `handoff start` opens a visible startup helper and may cause
inference. Start only when the user's task includes that handoff, using the
**current returned revision**:

```sh
con-cli --json handoff start JOB_ID --revision CURRENT_REVISION \
  --tab 1 --pane-id 0
```

Re-read a job after state changes; revisions protect against stale transitions.
An uncertain delivery is not a reason to launch again. Inspect the visible
helper and job state, and use the supported manual interaction when required.
Do not call `handoff respond ... received/sent/stopped` merely to advance the
state machine: those are assertions about actual delivery or stopped work.

Handoff preserves the current worktree. It does not stop the source agent or
isolate simultaneous writers. Coordinate the source before continuing edits in
the target. A completed handoff means context transfer, not task completion.


## Resume the same native conversation after restart

This is separate from cross-agent handoff. On macOS, con can link Claude Code
and Codex native sessions using explicitly installed native hooks. Installation
changes the selected agent's settings: do it only when session integration is
within the user's requested scope. Preserve native approval and hook trust.
Start Codex with `--no-daemon`; shared daemon hooks cannot reliably identify the
originating terminal.

Inspect `tree` or `surfaces list` for `native_agent`. A saved reference is not a
live worker and an observed link is not task success. Use exact surface IDs with
`surfaces resume-agent`; do not type `--last`, infer IDs from a screen, or launch
again after uncertain delivery. Automatic recovery is opt-in through
`con.appearance.resume_agent_sessions = true`. Missing native history or an
interaction-needed phase calls for visible inspection, not a fresh conversation.
`surfaces forget-agent` drops the link only; do not delete native history.
