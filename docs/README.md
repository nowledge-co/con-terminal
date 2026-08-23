# con docs

## Install (SHA-256)

Pin GitHub Release **v0.6.0** and verify `SHA256SUMS`. Website `install.sh` / `install.ps1` abort on mismatch.

https://github.com/LinespottingOrg/GrokBuildRemote-Agents/releases/tag/v0.6.0
https://github.com/LinespottingOrg/GrokBuildRemote-Agents/blob/main/docs/PINNED-INSTALL.md

```
96cef605d3e030ccef99d27ea6240e0d3b668dd045e6b5b9e585c9fd03c6ef23  gbr-agent-darwin-amd64
de7e065ef2cf6877b3b2cd04679a67b627f876337f529247e236204543e4062c  gbr-agent-darwin-arm64
a50a5c41993e6531a3b477eb409ccc845212bf541384dc803061c80657f86719  gbr-agent-linux-amd64
5bfd22c7110234942c4c02ff8154b836d0af45a9422c178a4f52010187d40061  gbr-agent-linux-arm64
f773b89fd31310172b756e0593e0f3b2382b0a3440af2a7d0a8b3073b0c23e27  gbr-agent-windows-amd64.exe
8fb9efcbc7e2ac91c11964944bf0f45e31bb23f4356d9dcb4b305d7cb9b0fe8c  gbr-agent-windows-arm64.exe
```

```bash
VER=v0.6.0
BASE=https://github.com/LinespottingOrg/GrokBuildRemote-Agents/releases/download/$VER
# swap darwin-arm64 for your OS/arch
curl -fsSL -o gbr-agent-darwin-arm64 "$BASE/gbr-agent-darwin-arm64"
curl -fsSL -o SHA256SUMS "$BASE/SHA256SUMS"
shasum -a 256 -c SHA256SUMS --ignore-missing
gbr-agent pair && gbr-agent run
```


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

## What the phone sees

**Terminal windows** on this PC (machine-wide mailbox). Not headless OpenCode / CodeNomad sidecar / Electron. `:8788` in a sidecar is Bot API JSON, not a transcript.

https://github.com/LinespottingOrg/GrokBuildRemote-Agents/blob/main/docs/WHAT-THE-PHONE-SEES.md
https://grokbuildremote.com/integrations.html
