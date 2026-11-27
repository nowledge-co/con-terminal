# CLI worker feedback from #575

The [#578 review](https://github.com/nowledge-co/con-terminal/pull/578)
found that a shell command could fail while `panes exec` exited zero.
It also found that the pane convenience command lacked the worker creation
options available through the surface commands.

The control request returned the terminal output and exit status, but the
CLI status reflected only delivery. The CLI now requires a reported exit
status of zero. It keeps the result on stdout, including the terminal's
exact status, and explains how to pass arguments when a space-containing
program name produces command-not-found. This preserves valid executable
paths containing spaces.

With `--background` or `--owner`, `panes create` delegates to the existing
surface split operation. Both creation paths return the readiness command.
For tab naming, an explicit surface title takes precedence over application
titles while that surface is focused, below an explicit tab name.

Owner labels match cleanup targets. Granting permission to stop an active
agent requires stronger evidence, such as a creation-scoped capability.
