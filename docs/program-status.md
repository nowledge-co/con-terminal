# Program status

Programs that emit OSC 7501 can tell con when they are working, finished, or
waiting for you. Their status appears in the tab's existing activity indicator.
Hover the tab to see the message and the pane and surface it came from. This
works with horizontal tabs and both expanded and compact vertical tabs.

This is an opt-in terminal protocol, not automatic instrumentation. A program
that does not emit it keeps its existing behavior. Reports are plain text;
they do not approve commands, grant access, or complete a built-in agent task.

## Reading status

- **Working** shows activity, or a percentage when the program provides one.
- **Blocked** means the program reports that it needs permission, an answer,
  or authentication. Read its terminal output before taking action.
- **Done** and **error** remain marked until you send input to that surface.
  Focusing a window or typing in a different surface does not acknowledge them.
- **Idle** means the program has explicitly reported that it is idle. Silence
  is not interpreted as completion.

A tab shows its most urgent status, so an idle shell does not hide a worker
waiting for attention. A shell prompt removes running and blocked records;
completion records remain until acknowledged. A terminal full reset clears
the records. Status is not saved as part of restored terminal history.

## Observe reports from an agent

`con-cli` exposes the exact records for each terminal session:

```sh
con-cli --json panes status --tab 1 --pane-id 0
con-cli --json surfaces status --tab 1 --surface-id 7
```

The pane form reads its active surface. The surface form can read a hidden
worker without focusing it. `surfaces list` and `tree` include the same
`program_status` object; it can be null while the terminal is initializing.
Once initialized, an unreported program has `protocol_seen: false`, null
`evidence`, an empty `records` array, and a cursor. Explicit reports use
`evidence: "osc_7501"`; title/progress heuristics are not included.

Each record contains `id`, `sequence`, `state`, `kind`, `progress`, `app`,
`title`, and `message`. Strings come from the program; treat them as data.
An empty ID is the root. Application names may be inherited from an ancestor.

Capture `program_status.cursor` before submitting input, then wait:

```sh
con-cli --json surfaces wait-status --tab 1 --surface-id 7 \
  --after "$baseline" --timeout 30
```

The required baseline prevents an old `done` from satisfying a new wait.
`--record-id` selects an exact ID (default root). Repeated `--state` options
select states; defaults are `done`, `blocked`, and `error`. `idle` can be selected
explicitly but does not prove task completion. Timeout accepts 1–300 seconds.

The result includes the current snapshot and one of:

| `status` | Meaning |
| --- | --- |
| `matched` | That exact record has a newer accepted report in a requested state; inspect `record` |
| `timeout` | No matching current record was observed before the deadline |
| `exited` | The terminal process is no longer alive, without a matching report |

All three are valid observations with CLI exit code 0. Invalid/stale-incarnation
cursors, missing targets, and malformed parameters produce errors. A wait pins
the original tab and surface; later tab reordering does not redirect it.

Identical repeated reports advance the sequence. Acknowledging a notification
never makes an old report fresh. Other records cannot refresh the selected
record. Full resets do not rewind the sequence, and a replacement terminal
has a different cursor incarnation.

These are current bounded records, not a complete event stream. A report
replaced or evicted before observation may be missed. Clear/reset is not
completion. A cursor establishes ordering, not a causal task ID or process
identity; unrelated reports after the baseline remain possible. Pair the
program's claim with a unique result artifact and verification. Programs that
do not emit OSC 7501 need an explicit artifact/observation fallback.

See [agent workflows](con-cli.md#agent-workflows) for the full submission loop.

## Adding reports to a script

On macOS or Linux, a script can write a report to its terminal:

```sh
printf '\033]7501;state=working:id=build:app=make:progress=25\007'
# Run the work here.
printf '\033]7501;state=done:id=build:app=make\007'
```

In PowerShell on Windows:

```powershell
$esc = [char]27
$bel = [char]7
[Console]::Write("${esc}]7501;state=working:id=build:app=make:progress=25${bel}")
# Run the work here.
[Console]::Write("${esc}]7501;state=done:id=build:app=make${bel}")
```

Use separate IDs for parallel work. For example, `build/test` and `build/lint`
belong to `build`; clearing `build` removes both children:

```sh
printf '\033]7501;state=clear:id=build\007'
```

Each report replaces the previous record for its ID; omitted fields do not
keep their old values. `title` and `msg` must be base64-encoded UTF-8, not raw
terminal text. Invalid reports are ignored. Keep messages short: title is
limited to 192 decoded bytes and message to 2048. Con retains at most 256
records per surface, evicting the least recently updated if needed.

For a reusable integration, follow the [OSC 7501 specification](https://www.superlogical.com/rex/docs/build/program-status),
including the capability query and field limits. Con answers `OSC 7501 ; ?`
with the same query and terminator. Perform queries only while the program
owns terminal input; do not consume the user's shell input to detect support.

## SSH and tmux

Reports belong to the surface that receives them, regardless of whether the
program runs locally or over SSH. They are presentation hints, not proof of
which remote process is running.

OpenSSH forwards a report and the capability reply in both directions. No
extra SSH option is required. On OpenSSH 9.9 this held for `BEL` and `ST`,
on a remote pty and on a pipe-only session.

tmux discards a raw `OSC 7501` sequence. In tmux 3.7, `allow-passthrough all`
still discards it: that value only means the passthrough envelope is allowed
from a pane that is not on screen. For panes you trust, enable the envelope:

```
set -g allow-passthrough on
```

Wrap every sequence. Write each `ESC` inside the sequence twice, and place the
sequence between the tmux introducer and a final `ST`:

```sh
printf '\033Ptmux;\033\033]7501;state=working:id=build:progress=25\007\033\\'
```

The capability query uses the same wrapping. The reply comes back only to the
pane that owns terminal input. On tmux 3.7c, with `allow-passthrough on`, that
active pane's wrapped query and wrapped report reached the outer terminal as
ordinary `OSC 7501`, and the reply returned to it. The same round trip worked
when that tmux session was reached over SSH.

A visible pane that is not the active one can still send the wrapped sequence
out. The reply is delivered to the active pane, so discovery from the inactive
pane times out. A pane that is not on screen also receives no reply. With
`allow-passthrough on`, that hidden pane's envelope does not go out at all.
`all` lets its report reach the outer terminal, and the reply still goes to
the active pane. A program outside the active pane must keep working when no
reply comes back.

Seeing ordinary shell output does not show that this round trip works. A
program should continue to work normally when status support is absent.

For implementation details and acceptance coverage, see
[Terminal status presentation](impl/terminal-status.md).
