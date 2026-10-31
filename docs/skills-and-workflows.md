# Skills and workflows

A skill is a slash command backed by a `SKILL.md` file. In con, that matters
because the skill runs through an agent that understands the terminal you are
already using.

This is the workflow loop con is built for:

1. Do the work once with the agent in the loop.
2. Keep the parts that worked.
3. Ask the agent to turn the routine into a skill.
4. Review the `SKILL.md`.
5. Run it next time with `/skill-name`.

The point is not to hide the terminal. The point is to turn a messy terminal
routine into something you can rerun with context, panes, SSH, tmux, and
approvals still visible.

## Let an external coding agent use con

The **con-terminal** skill teaches Claude Code, Codex, and other coding agents
to use con's terminal workspace through `con-cli`:

```sh
npx skills add nowledge-co/con-terminal --skill con-terminal
```

Install con first, then choose the agent destinations in the installer. Add
`-g` for user-wide installation or `-a codex` / `-a claude-code` to select one.
The skill is separate from the project routines loaded by con's built-in agent.

Use it for parallel implementation in separate worktrees, independent review,
visible test runs, or supported cross-agent context handoff. Keep workers in
pane-local surfaces, give them explicit acceptance criteria, observe fresh
reported status when available, and independently verify their results. The
[agent workflow guide](con-cli.md#agent-workflows) explains the method and CLI.

## Why skills fit con

Most agents can run a prompt. con can run a prompt while seeing the terminal
shape around it:

- focused pane output
- peer panes in the tab
- current working directories
- SSH sessions
- tmux sessions, windows, and panes
- TUIs and coding-agent CLIs
- available project and global skills

That lets a skill say things like:

- reuse the existing staging SSH pane if it is healthy
- create a shell pane only if no release shell exists
- run checks in tmux instead of typing into the wrong foreground TUI
- keep the coding agent pane separate from the shell pane
- ask before tagging, uploading, deploying, or editing protected files

This is still human-in-the-loop. Dangerous tools keep the approval flow unless
you deliberately enable auto-approve for a trusted workspace.

## A release workflow example

The first time through, you might ask the agent:

```text
Prepare a beta release. Use this repo, staging over SSH, and the release notes
we already have. Show me each irreversible step before running it.
```

The agent can work with the terminal instead of starting from a blank chat:

- inspect the current repo pane
- create or reuse a test pane
- create or reuse an SSH pane for staging
- use tmux targets when the remote host is already inside tmux
- run local tests and packaging commands visibly
- check release notes and tags
- ask before pushing, signing, uploading, or deploying

After the run is good, ask:

```text
Turn this release routine into a project skill. Keep approval points for tag,
upload, deploy, and anything destructive. Save it under .con/skills/release/SKILL.md.
```

The next release can start with:

```text
/release 0.1.0-beta.60
```

You still review the plan and approvals. The difference is that the hard-won
routine now lives with the project instead of in your memory.

## Pair skills with layout profiles

Skills describe behavior. Layout profiles describe workspace shape.

Use a layout profile when the workflow benefits from the same starting layout:

- `Release` tab with local shell, tests, staging SSH, and logs
- `Incident` tab with app logs, database shell, and deploy host
- `Agents` tab with a planner pane and worker surfaces

Use a skill when the workflow has repeatable judgment:

- which checks must pass
- which panes or hosts to reuse
- what needs approval
- what should be reported at the end

Together, they give you a repeatable starting point without turning con into a
closed workflow runner.

## Where skills live

Project skills travel with the workspace:

- `skills/`
- `.agents/skills/`
- `.con/skills/`

Global skills follow you across projects:

- `~/.config/con/skills`
- `~/.agents/skills`

On Windows, the con config skills folder is:

- `~/.config/con-terminal/skills`

Project skills override global skills with the same name. That lets a repo
define its own `/release` without changing your personal `/release` elsewhere.

## Skill file shape

Each skill is a folder with a `SKILL.md` file:

```text
.con/skills/release/SKILL.md
```

Minimal example:

```md
---
name: release
description: Prepare and verify a project release.
---

# Release

You are helping with a project release from inside con.

First inspect the current panes and working directory. Reuse existing local,
SSH, and tmux workspaces when they match the task. Do not create duplicate panes
unless no reusable target exists.

Before tagging, uploading, deploying, or editing release notes, show the exact
action and wait for approval.

When finished, summarize:
- version
- checks run
- artifacts produced
- release or deploy actions completed
- anything left for the user
```

Invoke it from the input bar or agent panel:

```text
/release 0.1.0-beta.60
```

Additional text after the skill name is passed as context for that run.

## What to put in a skill

Good skills are opinionated about the workflow, not over-specific about the
machine.

Include:

- the goal
- the expected panes, hosts, or tmux sessions
- commands that are safe to run
- commands that need approval
- checks that must pass
- the final report shape

Avoid:

- secrets
- personal absolute paths
- one-off command output
- instructions to bypass approvals
- instructions that assume a pane number will never change

Use stable intent instead: "reuse the staging SSH workspace" is better than
"use pane 3" unless the layout profile owns that shape.
