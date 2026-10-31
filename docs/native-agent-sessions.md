# Resume native agent conversations

con can keep a supported native agent conversation linked to its exact terminal
surface, then resume that conversation after con or the machine restarts.
This first implementation supports local macOS terminals. The native CLI and
its history must remain installed on the same machine.

## Set up once

Install the companion `con-cli` alongside con. Explicitly install the integration
for each native agent you use. Supported adapters are Claude Code, Codex,
Cursor Agent, Qwen Code and GitHub Copilot:

```sh
con-cli native-session install --agent claude
con-cli native-session install --agent codex
```

The installer preserves other hooks and settings, makes private backups before
replacing existing files, and enables Codex's native hooks feature. It does not
approve hooks on your behalf. Review Codex's native hook prompt when it appears.
Restart an already running agent after installing. Use `--config-dir /absolute/path`
if you use a separate native profile; install into each intended profile.

Start the native agent in a con terminal:

```sh
claude
codex
```

The Codex installer also adds a marked, reversible block to your current shell's
startup file (zsh, bash or fish). Open a **new con terminal** after installing;
then run `codex` normally. The block prepends con's launcher only inside con.
Existing aliases/functions keep priority and may bypass the launcher, as do
absolute native binary paths. For custom startup files use `--shell-rc /absolute/path`.
On macOS, bash uses `.bash_profile`; other platforms use `.bashrc`.

For Codex 0.166, the launcher automatically selects terminal-independent
`--no-daemon` execution for local interactive sessions. This preserves native
history, permissions, arguments, signals and exit status. Explicit remote servers,
noninteractive commands (`exec`, app-server, login, etc.), SSH and nested Codex
tool calls pass through. Profiles without con's installed hook also pass through.
Other terminals keep Codex's shared-daemon behavior. This is an explicit
compatibility layer until Codex provides client-side session-attachment events;
see [the integration design](design/native-agent-client-link.md).

If your shell has a custom alias or startup sequence, the explicit entry point is
`con-cli native-session launch --agent codex --` followed by your usual arguments.
con's automatic resume helper remains terminal-local as well.

Additional adapters use their native session-start hooks and exact resume flags:

| Agent | Install name | Native store override | Exact resume |
|---|---|---|---|
| Claude Code | `claude` | `CLAUDE_CONFIG_DIR` | `claude --resume ID` |
| Codex | `codex` | `CODEX_HOME` | `codex resume ID` (con selects local mode) |
| Cursor Agent | `cursor` | `CURSOR_CONFIG_DIR` | `cursor-agent --resume ID` |
| Qwen Code | `qwen` | `QWEN_HOME` | `qwen --resume ID` |
| GitHub Copilot | `copilot` | `COPILOT_HOME` | `copilot --resume=ID` |

Install the matching adapter with `--agent NAME`. Cursor requires one local
workspace root and native transcripts enabled; its native `XDG_CONFIG_HOME/cursor`
fallback is respected when `CURSOR_CONFIG_DIR` is unset. Qwen histories must remain within
`QWEN_HOME`; a separate `QWEN_RUNTIME_DIR` outside that store is currently refused
at recovery. Unsupported or missing history never falls back to a recent session.

SessionStart links the exact native session ID, working directory, native store
and history path to that surface. It does not collect prompt text, credentials
or transcripts into con's layout file. The native agent owns its conversation
history. OSC 7501 activity reports and process names do not establish this link.

## Choose how to restore

By default, con restores the workspace layout and keeps linked conversations
available for explicit resume. Right-click the terminal or its surface tab and
choose **Resume Agent Session** (the terminal menu names the agent). The CLI is:

```sh
con-cli --json surfaces list --tab 1
con-cli --json surfaces resume-agent --tab 1 --surface-id EXACT_SURFACE_ID
```

For automatic restart recovery, enable **Settings → General → Continuity →
Resume Agent Sessions**, or set:

```ini
con.appearance.resume_agent_sessions = true
```

This setting is independent of terminal screen replay and is off by default.
On restart, con resumes each valid linked surface once, including inactive tabs
and stacked surfaces. It waits for a proven local shell prompt and launches the
native CLI visibly in the recorded directory and native store. It selects the
exact saved conversation, never `--last`, and prevents concurrent automatic
resume of the same native identity. It sends no prompt and replays no tool call.
The agent retains its normal permission and trust flow.

A restored native terminal starts with the native conversation UI instead of
replaying an old terminal screen over it. Hidden surfaces remain hidden until
the current canvas commits their real geometry.

## If recovery needs attention

`tree` and `surfaces list` expose a `native_agent` object with the saved reference,
phase and diagnostic. `started` acknowledges native process startup; Codex may
confirm its session only on the next user turn. con sends no extra prompt to
force that event. `observed` means a native hook confirmed the link; it is
not a claim that an agent is currently working. Hover information distinguishes
saved, resuming, linked, failed and interaction-needed conversations.

If the native CLI, directory or exact history is unavailable, or the native
agent needs onboarding, authentication or hook approval, inspect the terminal.
con retains the reference and does not create a replacement conversation or
relaunch repeatedly. After resolving the problem, resume explicitly. Do not
launch again when delivery is uncertain. An active TUI or remote SSH session is
not a safe shell target.

**Forget Session Link** removes only con's saved reference, retaining native
history and the running terminal. Its CLI equivalent is:

```sh
con-cli --json surfaces forget-agent --tab 1 --surface-id EXACT_SURFACE_ID
```

To remove con's integration from a native profile:

```sh
con-cli native-session uninstall --agent claude
con-cli native-session uninstall --agent codex
```

Uninstall preserves other hooks and native history; it leaves Codex's general
hooks feature enabled because other integrations may need it.

## Boundaries

- This resumes conversations in new processes after restart. It does not keep
  processes alive, run a server, or guarantee recovery of an unfinished tool.
- Native recovery currently runs on macOS for the adapters listed above. Windows/Linux retain
  references without launching; additional native agents need explicit adapters.
- Remote hosts, ephemeral sessions, missing histories and unsupported profiles
  are not silently migrated. Profiles requiring extra CLI flags need manual
  native setup; con does not persist arbitrary launch commands or permission flags.
- Private session files contain local paths and native IDs. Shared workspace
  profiles exclude these references. Treat native history according to the
  native agent's own privacy and retention settings.
- Verified native CLI versions and the exact live validation scope are recorded
  in [the implementation study](study/con-agent-orchestration.md).

Cross-agent context transfer is a different operation: see
[Agent Handoff](con-cli.md#agent-workflows). It does not replace exact native resume.
