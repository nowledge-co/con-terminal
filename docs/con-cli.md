# con-cli and surfaces

`con-cli` is the command-line handle for a running con app. It is installed with
con on macOS, Windows, and Linux.

Most users do not need it. Use it when another program, script, test runner, or
agent needs to inspect the visible terminal, create panes, drive pane-local
surfaces, or ask con's built-in agent.

External agents should get a real terminal to work with, not a headless shell
that behaves differently from what the user sees.

## Check that con is reachable

Start con first, then run:

```sh
con-cli identify
con-cli tabs list
con-cli panes list
```

For scripts and agent adapters, prefer JSON:

```sh
con-cli --json identify
con-cli --json tree
```

`con-cli` asks the running app for state instead of guessing from outside the
terminal.

## Agent skill

The repository includes a vendor-neutral [con skill](https://github.com/nowledge-co/con-terminal/tree/main/skills/con-terminal)
for agents that can run `con-cli`. It covers visible shell work, pane-local
workers, parallel worktrees, and supported Agent Handoff flows.

Install it into your coding agent with:

```sh
npx skills add nowledge-co/con-terminal --skill con-terminal
```

Choose the coding agents to install to, or add `-a codex` / `-a claude-code`.
Add `-g` for user-wide installation. con and `con-cli` must already be installed.

## Agent workflows

Use the skill when you want your coding agent to coordinate work in con:

```text
Use con to review this change in a separate worker session. Keep the current
terminal untouched, write a report, and verify the findings before summarizing.
```

```text
Use con to split this task between implementation and tests. Give parallel
writers separate worktrees, keep workers in one pane as surface tabs, and
review their diffs before integrating anything.
```

```text
Continue this Codex task in another installed agent using Agent Handoff.
Review the transferred context and coordinate the source before editing.
```

Choose the smallest arrangement that fits the work. Run a single shell check
with `panes exec`; create a worker split when side-by-side visibility matters;
add surfaces inside that pane for more sessions. Extra surfaces keep running
when hidden. Separate worktrees prevent file collisions, while separate ports,
sockets, and test databases prevent runtime collisions.

Give each worker a specific goal, file ownership, acceptance checks, and a
unique result/report path. Save the returned tab, pane, and surface identities.
Only control task-owned sessions or ones the user explicitly names. A surface's
`owner` label helps bookkeeping but is not an access-control lock.

Observe → submit once → wait for evidence → verify → integrate. A successful
input write is not proof that the agent accepted the task. A fresh program
report is not proof that the code works. Review the report, inspect the diff,
and run relevant checks. Preserve the normal permission flow and leave results
open when the user may want to inspect them.

### Observe a worker's reported state

```sh
con-cli --json panes status --tab 1 --pane-id 0
con-cli --json surfaces status --tab 1 --surface-id 7
```

Check `con-cli --json capabilities` for `surfaces.status` and
`surfaces.wait_status`; older app/CLI builds need an update to use these commands.
Both read the same explicit OSC 7501 facts. The pane form addresses its active
surface; the surface form keeps targeting that exact session even when hidden.
`tree` and `surfaces list` also include `program_status` for each surface.

For a reporting worker, capture the cursor **before** sending its next prompt
(this example uses `jq`):

```sh
baseline=$(con-cli --json surfaces status --tab 1 --surface-id 7 |
  jq -er '.program_status.cursor')
con-cli --json surfaces send-text --tab 1 --surface-id 7 -- \
  'Read /absolute/path/task.md and write the requested report.'
con-cli --json surfaces send-key --tab 1 --surface-id 7 enter
con-cli --json surfaces wait-status --tab 1 --surface-id 7 \
  --after "$baseline" --timeout 30
```

Read the result's `status` and `record`, not just its exit code. Defaults match a
fresh root `done`, `blocked`, or `error`; `--record-id build/test` selects an
exact child record, and repeated `--state` options customize the states. A
`blocked` report needs inspection, not automatic approval. Timeouts are valid
observations and do not submit or cancel anything.

Terminal initialization (`wait-ready`), readiness for agent input, reported
state, and verified task completion are separate checkpoints. A program that
never reports OSC 7501 remains unknown; use explicit result artifacts and
bounded observation rather than guessing from a quiet screen. See
[program-status semantics](program-status.md#observe-reports-from-an-agent).

## SSH terminfo cache

Con also provides the cache command used by shell integration. It is a local
metadata cache: entries identify a host where Con's `xterm-ghostty` terminfo
has already been installed. It does not store credentials, SSH keys, command
output, or remote data.

```sh
con-cli +ssh-cache --host=example.com
con-cli +ssh-cache --add=example.com
con-cli +ssh-cache
con-cli +ssh-cache --remove=example.com
con-cli +ssh-cache --clear
```

The host check exits `0` when an entry is present and `1` when it is absent,
so shell integration can use it without parsing output. Cache entries are
stored under Con's platform-specific user cache directory and are written
atomically. `--expire-days N` filters entries older than `N` days for list and
host checks. On macOS, the installed app bundle exposes this command through
the compatibility entry point expected by Ghostty's shell integration, so SSH
terminfo setup works without a separate Ghostty executable.

## Pane commands

Read terminal content:

```sh
con-cli --json panes read --tab 1 --pane-id 0 --lines 120
```

Create a visible split:

```sh
con-cli --json panes create --tab 1 --location right --command "htop"
```

Run a command visibly in a shell pane:

```sh
con-cli --json panes exec --tab 1 --pane-id 0 -- cargo test -q
```

Use `pane_id` for follow-up automation. Pane indexes can change when a user
splits, closes, or rearranges panes.

## Surface commands

A pane is a visible split region. A surface is a terminal session inside one
pane. Every pane has one active surface, and a pane can host more surfaces when
you want pane-local tabs.

This is the pattern most subagent tools want:

1. Create the first worker as a visible split.
2. Add later workers as surfaces inside that worker pane.
3. Drive each worker by `surface_id`.
4. Close each worker surface when it finishes.

Create the first worker pane:

```sh
con-cli --json surfaces split \
  --tab 1 \
  --pane-id 0 \
  --location right \
  --title worker-1 \
  --owner pi-interactive-subagents \
  --command "codex"
```

Create another worker inside the same pane:

```sh
con-cli --json surfaces create \
  --tab 1 \
  --pane-id <worker_pane_id> \
  --title worker-2 \
  --owner pi-interactive-subagents \
  --command "codex"
```

Wait before sending input:

```sh
con-cli --json surfaces wait-ready --surface-id <surface_id> --timeout 10
```

Drive the worker:

```sh
con-cli --json surfaces send-text --surface-id <surface_id> "explain this repo"
con-cli --json surfaces send-key --surface-id <surface_id> enter
con-cli --json surfaces read --surface-id <surface_id> --lines 120
```

Close it:

```sh
con-cli --json surfaces close --surface-id <surface_id>
```

If a worker pane was created only for owned surfaces, an orchestrator can ask con
to close that pane when its last surface closes:

```sh
con-cli --json surfaces close \
  --surface-id <surface_id> \
  --close-empty-owned-pane
```

## Ask the built-in agent from a script

`agent ask` uses the same in-app agent session you see in the side panel:

```sh
con-cli --json agent ask --tab 1 "Summarize what is happening in this tab"
```

That means the response appears in con, uses the tab's current context, and
follows the same provider and approval settings as the UI.

## Automation rules

- Use `--json` for anything a program will parse.
- Use `pane_id` and `surface_id` after discovery.
- Call `surfaces wait-ready` before driving a new surface.
- Treat `panes exec` as visible shell control, not a hidden subprocess runner.
- Give orchestrator-created surfaces an `--owner` so cleanup can be scoped.
- Re-read `tree` after user-visible layout changes.

The socket is local to the user session. It is not a remote API, and it does not
change shell permissions. If a script can drive `con-cli`, it can drive the
visible terminal you already opened.

## Socket path

Release builds use the default socket for the installed channel. Debug builds
use a separate debug socket so a local development build can run beside the
installed app.

Set `CON_SOCKET_PATH` only when you intentionally run con on a custom endpoint:

```sh
CON_SOCKET_PATH=/tmp/con-alt.sock con
con-cli --socket /tmp/con-alt.sock identify
```


## Resume after app or machine restart

On macOS, install the explicit native agent hooks, then use exact native
conversation links per surface. Automatic resume is off by default and can be
enabled under General → Continuity. After Codex integration setup, open a new con terminal and run `codex` normally.
See [native agent sessions](native-agent-sessions.md) for setup, failure recovery,
privacy boundaries and the `surfaces resume-agent` / `forget-agent` commands.
