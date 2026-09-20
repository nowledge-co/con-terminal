# Workspace layout profiles

When a workspace shape is worth keeping for a project, save it as a layout
profile. A profile is for starting a project in the right shape again, or for
sharing that shape with a team.

## What a layout profile is

A layout profile is a con-generated `.con/workspace.ghostty` file.

It describes workspace shape:

- tab names
- pane split structure
- pane and surface names
- relative working directories
- optional agent provider/model defaults

It does not contain private runtime state:

- no commands to run
- no conversations
- no command history
- no terminal text history
- no credentials
- no trust decisions

Think of it as a profile for recreating a tuned workspace, not as a terminal
session backup. It is safe to review because it describes shape, not activity.

## The basic flow

1. Tune the workspace visually until it feels right.
2. Name the tabs, panes, and surfaces so the intent is obvious.
3. Save the layout profile.
4. Review the generated `.con/workspace.ghostty`.
5. Reopen the project later with `con ~/dev/app`, or commit the file so a
   teammate can get the same starting shape.

The profile captures the workspace you designed. It does not capture what you
typed, what the agent said, or what processes were running.

Private terminal text settings live in **Settings -> General**. Layout profiles
never include terminal text, regardless of that setting.

## Save a profile

1. Open a project in con.
2. Arrange tabs, panes, and surfaces visually.
3. Rename tabs, panes, and surfaces so the layout is understandable.
4. Choose **Save Layout Profile** from Command Palette or the Workspace menu.
5. Save as `.con/workspace.ghostty` under the project root.
6. Review the generated file.
7. Commit it only if the layout is useful to the project.

Example:

```text
Project: ~/dev/app
  Tab: Dev
    Pane: Shell      cwd .
    Pane: Server     cwd crates/server
    Pane: Tests      cwd crates/server
  Tab: Agents
    Surface: Planner cwd .
    Surface: Worker  cwd crates/ui
```

The exported file should recreate that shape, not the processes that happened
to be running inside it.

Path values are written as repo-relative slash paths, even on Windows:

```ini
pane.cwd = "crates/server"
```

That keeps the file stable in git diffs and usable across machines.

## Open a project profile

Open a project profile explicitly:

```sh
con ~/dev/app
```

con opens `~/dev/app/.con/workspace.ghostty` when it exists. Otherwise it can
read a legacy `.con/workspace.toml` profile without rewriting it. If neither
exists, con opens a fresh shell rooted at `~/dev/app`.

Open a profile file directly:

```sh
con ~/dev/app/.con/workspace.ghostty
```

If the requested profile is malformed, con opens a fresh shell and shows the
profile error in the terminal. It does not silently open an unrelated workspace.

Inside the app, use:

- **Add Tabs from Layout Profile** to choose a project folder or profile file
  and add its tabs to the current window. If the folder has no profile, con adds
  one fresh tab rooted there.
- **Open Layout Profile in New Window** to choose a project folder or profile
  file and open it separately.

To open a project profile, pass the project path or the profile file path
explicitly.

## Share a profile

Commit `.con/workspace.ghostty` when the layout is useful to other people on the
project.

Good shared profile content:

- "Dev", "Server", "Tests", "Release" tab names
- repo-relative cwd values
- panes and surfaces with clear names
- no machine-specific absolute paths unless unavoidable

Bad shared profile content:

- personal history
- output transcripts
- secrets
- commands that run automatically
- private agent conversations

## New tab, new window, and defaults

Use these rules:

- **New Tab** stays scratch by default. It should not surprise you by expanding
  into a multi-pane project layout.
- **Add Tabs from Layout Profile** is the explicit "new tab(s) from this profile"
  flow. If the profile contains one tab, it behaves like a new tab. If it
  contains several tabs, con adds all of them.
- **Open Layout Profile in New Window** is the explicit "new window from this
  profile" flow.
- **New Window** stays scratch by default. A global "default new-window layout"
  setting is intentionally deferred because it can fight with project memory.

## Entry points

| Gesture | Result |
| --- | --- |
| Cmd+N / New Window | Open one clean scratch shell with shared history. |
| `con ~/dev/app` | Open the project's profile if present; otherwise one shell rooted there. |
| `con ~/dev/app/.con/workspace.ghostty` | Open that profile directly. |
| Add Tabs from Layout Profile | Add the selected project/profile into the current window. |
| Open Layout Profile in New Window | Open the selected project/profile separately. |

This keeps every entry point legible: scratch is fresh, and project layout is
explicit.

## Process continuity

Layout profiles do not resume running processes. Use tmux or zellij when you
want processes to survive app restarts:

```sh
tmux attach -t app || tmux new -s app
```

con recreates the layout and directory. tmux restores the running session.

## Skills

If a layout is the workspace shape, a skill is the routine that runs inside it.
A release layout might open local tests, staging SSH, and logs. A `/release`
skill can then decide which targets to reuse, which checks to run, and where it
must stop for approval.

See [Skills and workflows](skills-and-workflows.md).
