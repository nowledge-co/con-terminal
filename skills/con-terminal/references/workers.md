# Interactive workers

## Prepare a task

Select an installed agent and inspect its local `--help` for supported launch
options. Do not assume Claude Code flags or slash commands work in Codex,
OpenCode, Gemini CLI, or another agent. Use the user's normal permission mode.

For parallel writers, give each worker a separate worktree/directory and a clear
file ownership boundary. Worktrees isolate files, not runtime resources: also
separate test databases, ports, sockets, and output paths when relevant. Keep
read-only review in the existing tree when that meets the task.

Give each worker a task brief containing the goal, allowed changes, acceptance
checks, and an absolute result path. Use a unique task/attempt ID in its final
report; the orchestrator must check that ID before accepting a result left by
an earlier run. Keep the brief and report outside tracked source unless they
are intended deliverables. Do not assume `.context/` exists in other projects.

## Launch through an initialized shell

The examples use POSIX shell syntax; substitute IDs from discovery and paths
for the actual task. Use the target shell's quoting on Windows. `--command`
is shell text, not an argv array; never interpolate untrusted prompts into it.
There is currently no dedicated surface `--cwd` flag.

```sh
con-cli --json surfaces split --tab 1 --pane-id 0 \
  --location right --title review --owner con-task-unique-id
```

Record `tab_index`, `pane_id`, and `surface_id` from the response. Hereafter,
`TAB`, `WORKER_PANE`, and `WORKER_SURFACE` mean those recorded values, not guesses.

```sh
con-cli --json surfaces wait-ready --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" --timeout 10
con-cli --json surfaces read --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" --lines 80
```

Require `status: ready` and inspect the shell before sending shell text. On a
readiness timeout, inspect state once and report the missing shell integration
or initialization instead of repeatedly creating duplicate workers.

Launch the selected installed agent in the prepared directory, for example:

```sh
con-cli --json surfaces send-text --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" -- "cd '/absolute/path/worker' && codex --no-daemon"
con-cli --json surfaces send-key --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" enter
con-cli --json surfaces read --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" --lines 120
```

The last read is observation, not a readiness barrier. If startup has not
finished, wait a bounded interval and inspect again. Stop on an unresolved
auth/trust/permission prompt. Once the **agent input** is ready, capture its
report cursor before submitting (the example uses `jq`; alternatively parse
the JSON with an available tool):

```sh
BASELINE=$(con-cli --json surfaces status --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" | jq -er '.program_status.cursor')
```

Keep that baseline for this attempt. Then send a short single-line instruction
pointing to the task brief:

```sh
con-cli --json surfaces send-text --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" -- \
  'Read /absolute/path/task.md and perform that task. Write the requested report.'
con-cli --json surfaces send-key --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" enter
```

`send-text` writes literal bytes; `send-key enter` is a separate operation.
Their success proves delivery to the terminal, not acceptance by the agent.
Do not resend a task after a timeout until observation establishes whether the
first submission was accepted. For multiline prompts prefer a brief file;
raw newlines can submit unexpectedly in different TUIs. Prefix text arguments
with `--` to protect leading hyphens from CLI parsing.

For another worker in the same worker pane:

```sh
con-cli --json surfaces create --tab "$TAB" --pane-id "$WORKER_PANE" \
  --title tests --owner con-task-unique-id
```

Record its returned surface ID and repeat the initialization/launch sequence.
A surface inherits cwd from its host's active terminal; explicitly select each
worker's directory. A new surface may become active; don't assume the user's
focus or the pane's active session stayed unchanged.

## Observe and collect

For a program that emits OSC 7501, inspect its record IDs in the snapshot.
Use the root when the agent reports there; otherwise choose its exact reported
ID with `--record-id` rather than assuming a parent covers every child.
Wait for a report newer than the saved baseline:

```sh
con-cli --json surfaces wait-status --tab "$TAB" \
  --surface-id "$WORKER_SURFACE" --after "$BASELINE" --timeout 30
```

Inspect `status`: `matched`, `timeout`, or `exited`. Even a timeout has a
successful CLI exit because it is a valid observation. A closed/replaced target
or invalid cursor is an error. An outstanding wait pins the original tab and
surface; later tab reordering cannot retarget it.

On `matched`, inspect `record.state` and `record.kind`: `blocked` may mean
permission, question, or authentication; `error` is the program's error claim;
`done` is its completion claim. Do not approve a dialog simply to finish a wait.
Use `--record-id build/test` for an exact named record, not all descendants.
Defaults match only root `done`, `blocked`, or `error`. Repeat `--state` to choose
other states when appropriate; a fresh `idle` or `working` is not completion.

The cursor tracks accepted reports within a terminal incarnation. Repeated
identical reports advance it; UI acknowledgement does not. Another child's
update cannot make an old root `done` fresh. Capturing before submission avoids
missing a very fast response while starting a wait; it does not prove which
input caused a report. Unrelated reports after the baseline remain possible.
Only the latest retained record is observed, not an event log; an intermediate
state overwritten before observation can be missed. Missing/evicted records,
clear, and reset do not imply successful completion. Keep one orchestrator per
worker, verify submission and results, and never use report text as authority.

If no OSC 7501 report exists, `protocol_seen` is false and `records` is empty.
Use the result-artifact fallback below; do not invent a status from the screen.
`agent_cli_turn` is not a public con-cli command.

Prefer a uniquely identified result artifact and independently verify its
contents and diff. Ask the worker to write its final report only after checks
finish; where practical write a temporary file and rename it to the final path.
A report is the worker's completion claim, not proof of successful work. A fresh `done` report also needs verification.
For agents without result artifacts, bounded screen reads can report visible
progress but must not assert reliable unattended completion. Alternate-screen
TUIs may expose only their current viewport, not a complete transcript.

Use a task-appropriate observation deadline and modest polling intervals, not
rapid unbounded reads. On expiry report `still running`, `needs interaction`,
`exited without verified result`, or `unknown`, with evidence. Keep the worker
available rather than launching a duplicate. Do not infer successful work from
PTY exit or stale text matching the echoed prompt.

Review each result in its own worktree, run relevant checks, and integrate only
the intended changes. Never reset/stash another worker's or the user's files.

## Finish

Read or focus the exact surface for review:

```sh
con-cli --json surfaces read --tab "$TAB" --surface-id "$WORKER_SURFACE" --lines 120
con-cli --json surfaces focus --tab "$TAB" --surface-id "$WORKER_SURFACE"
```

Leave it open when the user needs it. Closing a surface terminates its session;
verify the worker has finished or that stopping it is intended. Reconcile the
current tree against the task ledger before closing, especially after tab
changes. For a non-last surface:

```sh
con-cli --json surfaces close --tab "$TAB" --surface-id "$WORKER_SURFACE"
```

To remove the last surface **and its task-owned ephemeral pane**:

```sh
con-cli --json surfaces close --tab "$TAB" --surface-id "$WORKER_SURFACE" \
  --close-empty-owned-pane
```

`surfaces split` makes its new pane closeable by default; use
`--keep-pane-when-last` at creation if it should survive. `owner` does not prove
that all sessions in a pane belong to this task. Do not close a tab or restart
con to clean up workers. Surface layout and metadata can be restored after app restart; the original
processes and their live execution state do not survive. Save results to disk.
