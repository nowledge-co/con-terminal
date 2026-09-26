# Terminal workflows

con should disappear until you need it. With one tab open and the panels hidden,
the app is a clean terminal surface. The extra controls are there for the
moments when they save movement: running a command across panes, asking the
agent, opening a split, or focusing a worker.

## The workspace model

con uses a few names consistently:

<p align="center">
  <img width="1080" alt="Con workspace with terminal panes, tabs, and the agent panel" src="https://github.com/user-attachments/assets/81dcf6e4-6074-497e-98b8-00c884442cc2" />
</p>

| Name | Meaning | Use it for |
| --- | --- | --- |
| Window | A top-level app window | Separate projects or monitors |
| Tab | A workspace inside a window | One project, task, or long-running shell set |
| Pane | A visible split region inside a tab | Side-by-side shells, servers, logs, editors |
| Surface | A terminal session inside one pane | Multiple agent or worker sessions without adding more visible splits |

Most terminal work only needs tabs and panes. Surfaces are advanced; they are
there when one visible pane should host several terminal sessions.

## Focus without reaching for the mouse

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| Switch focus between terminal and input | <kbd>⌘</kbd> <kbd>I</kbd> | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>I</kbd> |
| Show or hide the input bar | <kbd>⌃</kbd> <kbd>`</kbd> | <kbd>⌃</kbd> <kbd>`</kbd> |
| Show or hide the agent panel | <kbd>⌘</kbd> <kbd>L</kbd> | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>L</kbd> |
| Show or hide vertical tabs | <kbd>⌘</kbd> <kbd>B</kbd> | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>B</kbd> |
| Open Command Palette | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>P</kbd> | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>P</kbd> |

When in doubt, open Command Palette and search by the action name.

On macOS, [Quick Terminal](quick-terminal.md) can give you a separate
drop-down terminal from anywhere on the system. It is optional and off by
default.

## Tabs and windows

Use a new tab when the work belongs to the same window. Use a new window when
you want a separate space, usually for another project or display.

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| New window | <kbd>⌘</kbd> <kbd>N</kbd> | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>N</kbd> |
| New tab | <kbd>⌘</kbd> <kbd>T</kbd> | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>T</kbd> |
| Next tab | <kbd>⌃</kbd> <kbd>Tab</kbd> | <kbd>⌃</kbd> <kbd>Tab</kbd> |
| Previous tab | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>Tab</kbd> | <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>Tab</kbd> |
| Select tab 1-9 | <kbd>⌘</kbd> <kbd>1</kbd> ... <kbd>9</kbd> | <kbd>⌃</kbd> <kbd>1</kbd> ... <kbd>9</kbd> |

Vertical tabs are useful once tab titles matter more than tab position. Rename a
tab when its purpose is stable.

## Panes

Panes are visible splits. They are the right tool when you need to see more than
one terminal at once.

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| Split right | <kbd>⌘</kbd> <kbd>D</kbd> | <kbd>Alt</kbd> <kbd>D</kbd> |
| Split down | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>D</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>D</kbd> |
| Toggle pane zoom | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>Enter</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>Enter</kbd> |
| Close pane | <kbd>⌘</kbd> <kbd>⌥</kbd> <kbd>W</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>W</kbd> |

Pane zoom is for short bursts of focus. It does not stop sibling panes; it only
gives the active pane the full terminal area.

When a tab has more than one pane, each pane shows a compact title bar. Use it
to see which pane is focused, close a pane, or fullscreen that pane without
leaving the split layout. Drag the title bar to rearrange panes, or drag it into
the tab strip to move that pane into its own tab.

If you want the cleanest possible terminal surface, hide pane title bars from
Settings -> Appearance. The shortcuts and terminal context menu still expose the
same pane actions.

## The input bar

The input bar is one surface with three modes:

- **Smart** decides whether your text is a shell command or an agent request.
- **Command** sends text to the terminal.
- **Agent** sends text to the built-in agent.

This keeps con from becoming a chat app wrapped around a shell. You can hide the
bar when you want a plain terminal, bring it back when you need a command or
question, and switch modes without changing context.

## Broadcast commands

Command mode can target more than one pane. Use the pane picker to send a
command to the focused pane, every pane, or a selected set. The picker mirrors
the current pane layout, so you can choose by position instead of remembering
pane numbers.

<p align="center">
  <img width="1080" alt="Con pane broadcast picker mirroring the current split-pane layout" src="https://github.com/user-attachments/assets/21e295c9-34eb-465c-90b8-5ba3163182e5" />
</p>

## Advanced: surfaces

A surface is a terminal session inside one pane. Think of it as a pane-local
tab.

Use surfaces when you want several live terminal sessions but do not want to
keep splitting the screen smaller. This is especially useful for coding agents,
subagents, or task-specific shells that should share one visible worker area.

| Action | macOS | Windows / Linux |
| --- | --- | --- |
| New surface in focused pane | <kbd>⌘</kbd> <kbd>⌥</kbd> <kbd>T</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>T</kbd> |
| New surface pane right | <kbd>⌘</kbd> <kbd>⌥</kbd> <kbd>D</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>→</kbd> |
| New surface pane down | <kbd>⌘</kbd> <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>D</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>↓</kbd> |
| Next surface | <kbd>⌘</kbd> <kbd>⌃</kbd> <kbd>]</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>]</kbd> |
| Previous surface | <kbd>⌘</kbd> <kbd>⌃</kbd> <kbd>[</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>[</kbd> |
| Rename surface | <kbd>⌘</kbd> <kbd>⌥</kbd> <kbd>R</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>R</kbd> |
| Close surface | <kbd>⌘</kbd> <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>W</kbd> | <kbd>Alt</kbd> <kbd>⇧</kbd> <kbd>X</kbd> |

Only panes with multiple surfaces, owned orchestrator sessions, or explicit
surface names show the in-pane surface strip. Ordinary panes stay clean.

## Terminal menu

Right-click the terminal for common actions:

- copy and paste
- clear terminal
- split or close panes
- zoom the pane
- create, rename, switch, or close surfaces
- focus the input bar
- open Settings or Command Palette

The menu uses the same actions as shortcuts and Command Palette. If an action
has a shortcut, the menu shows it.

## Links, paste, and files

Use the platform modifier to open links from terminal output:

- macOS: hold <kbd>⌘</kbd>, then click a URL.
- Windows and Linux: hold <kbd>⌃</kbd>, then click a URL.

Inside a mouse-aware TUI, hold <kbd>⇧</kbd> while dragging to select and
copy terminal text rather than sending the drag to the app. On macOS,
<kbd>⌘</kbd>-drag also selects text; <kbd>⌘</kbd>-click opens a link.

Paste text normally. Dragging files into the terminal sends their paths. When a
TUI supports image/file paste protocols, con forwards compatible clipboard and
drop payloads through the terminal path.

## Project layouts

If a project has a workspace shape worth keeping, save it as a layout profile.
Profiles are project files for recreating tabs, panes, surfaces, names, and
working directories. See [Workspace profiles](workspace-layout-profiles-guide.md).

If the behavior inside that layout becomes repeatable, turn it into a
[skill](skills-and-workflows.md).
