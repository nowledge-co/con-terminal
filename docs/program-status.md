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

SSH carries terminal bytes, but a multiplexer can consume escape sequences.
Inside tmux, use its supported terminal-passthrough mechanism and enable
passthrough only if you trust the programs running there. Both reports and
capability replies must reach the right program; merely seeing shell output
does not verify this round trip. A program should continue to work normally
when status support is absent.

For implementation details and acceptance coverage, see
[Terminal status presentation](impl/terminal-status.md).
