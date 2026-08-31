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
host checks.

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
