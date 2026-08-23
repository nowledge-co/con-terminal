# con docs

con is a terminal first. If you hide the input bar and agent panel, it should
feel like a fast, elegant terminal with nothing extra in the way.

<p align="center">
  <a href="screenshots.md">
    <img width="1080" alt="Con main window with terminal panes and the agent panel" src="https://github.com/user-attachments/assets/389898d6-56bf-46aa-9279-65e59a57ed23" />
  </a>
</p>

When you ask for AI, con uses the terminal objects you already work with:
panes, SSH sessions, tmux panes, TUIs, visible output, and working directories.
When a one-off routine becomes worth repeating, skills let you keep it as a
slash command. When you build on top of con, `con-cli` and surfaces give
external agents a real terminal to drive.

Start with the page that matches what you are trying to do.

## Start

| Need | Read |
| --- | --- |
| Install con | [Install](install.md) |
| Learn the main controls | [Quick controls](quick-controls.md) |
| Open a drop-down terminal from anywhere on macOS | [Quick Terminal](quick-terminal.md) |
| Work with tabs, panes, broadcast, links, and pane zoom | [Terminal workflows](terminal-workflows.md) |
| Connect providers, tune appearance, and edit shortcuts | [Settings](settings.md) |

## Use con every day

| Need | Read |
| --- | --- |
| Use the agent panel without leaving the terminal | [Built-in agent](agent.md) |
| Turn a repeated terminal routine into a slash command | [Skills and workflows](skills-and-workflows.md) |
| Save or share a project layout | [Workspace profiles](workspace-layout-profiles-guide.md) |
| See the app | [Screenshots](screenshots.md) |
| See what changed | [Changelog](../CHANGELOG.md) |

## Build on con

| Need | Read |
| --- | --- |
| Drive con from scripts, test runners, or external agents | [con-cli and surfaces](con-cli.md) |
| Pair a spectator phone (Build Remote Agent, gbr/1) | [Build Remote Agent](gbr.md) |

## Platform status

- macOS is the primary beta platform.
- Windows is in preview.
- Linux is in preview.

Platform-specific limits are tracked in the source repository:
[Windows](https://github.com/nowledge-co/con-terminal/issues/34) and
[Linux](https://github.com/nowledge-co/con-terminal/issues/18).

## Contributor docs

These public docs are for people using con. If you want to build or change con
itself, start with the contributor quickstart in the source repository. The
implementation notes in `docs/impl/` and `docs/study/` are written for
contributors, not for the hosted docs navigation.

## Source of truth

The public docs navigation comes from [`docs/manifest.json`](manifest.json).
When a PR adds, renames, or removes a public docs page, update the manifest in
that PR. CI checks the manifest, and merges to `main` rebuild
`con.nowledge.co/docs`.
