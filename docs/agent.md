# Built-in agent

The agent in con is not the product. The terminal is.

The agent panel is there when terminal context matters and a model can help. It
reads the pane you are using, acts in view, and asks before doing work that
should not happen silently.

<p align="center">
  <img width="1080" alt="Con agent panel next to a terminal workspace" src="https://github.com/user-attachments/assets/e1972fac-9df3-443b-be08-c0eec4697cf3" />
</p>

## When to use it

Use the agent when the answer depends on terminal state:

- explain an error on screen
- plan a fix before changing files
- run a command after checking the current pane
- work inside an SSH or tmux session
- reason about a TUI or coding-agent CLI already running in a pane
- summarize what happened in a long terminal flow

Use the shell directly when you already know the command.

Provider and model choices live in [Settings](settings.md). Each tab keeps its
own active provider/model once you choose it. Global settings define the default
for new sessions and the models available in the picker.

## What terminal-native means

The agent starts from the focused pane. From there it can reason about terminal
objects instead of loose screenshots:

- visible pane output
- pane names and working directories
- SSH session context
- tmux sessions, windows, and panes
- shell state and command history
- TUIs and coding-agent CLIs running inside the terminal

That context is useful, but it is not a license to act silently. Destructive or
high-impact actions still require approval.

When the agent streams markdown, con renders code, tables, math, and diagrams in
the panel instead of treating the answer as a separate web page. Long responses
stay scrollable so the terminal remains usable.

## Context and file access

Asking a question does not recursively scan your current directory. Automatic
context comes from the terminal snapshot already held by con; it does not read
`AGENTS.md`, Git diffs or a project file listing in the background. This applies
in HOME, ordinary projects and SSH sessions alike.

Set `con.agent.auto_context = false` to omit automatic terminal context and its
background pane queries from new requests. Your question and conversation
history are still sent to the selected provider; turning this off does not
remove information from earlier messages.

If the agent needs local files, it uses visible `file_read`, `list_files` or
`search` tools. These require approval by default, with the local workspace
root shown alongside the arguments. Existing automatic-tool-approval settings
remain an explicit opt-in. Local path checks still constrain file tools to
their workspace scope; remote work uses terminal-native tools instead.

Provider configuration is checked before automatic context queries. Local and
subscription providers retain their existing keyless or OAuth flows. macOS
folder access is an OS permission, not permission to silently include files in
a model request.

## SSH, tmux, and TUIs

con is built for terminal-native workflows. The agent can help while you are
inside SSH, tmux, shells, editors, and coding-agent CLIs.

Keep the same rule in mind: the terminal remains the source of truth. If the
agent needs to know what is happening, it should inspect the pane before making
claims or taking action.

## Stay in control

- Keep the pane you care about focused before asking.
- Ask for a plan first when the task is broad.
- Review commands before approving them.
- Use the terminal directly for simple commands.
- Hide the agent panel when you want a plain terminal.

con should feel like a serious terminal with help available, not a chat app
wrapped around a shell.

## External agents

If you are building an orchestrator or subagent workflow, use
[con-cli and surfaces](con-cli.md). Surfaces let another agent create worker
terminal sessions inside a pane without taking over the main terminal layout.

The built-in agent harness and benchmark loop are open in the repository. They
exist so terminal-native behavior can be tested and improved, not hidden behind
a product claim.

If you want to repeat a workflow through the built-in agent itself, write it as
a [skill](skills-and-workflows.md). Skills are the user-facing way to keep a
good terminal routine without turning it into a separate app.
