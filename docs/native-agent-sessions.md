# Resume native agent conversations

con can keep a Claude Code or Codex conversation linked to its exact terminal
surface, then resume that conversation after con or the machine restarts.
This first implementation supports local macOS terminals. The native CLI and
its history must remain installed on the same machine.

## Set up once

Install the companion `con-cli` alongside con. Explicitly install the integration
for each native agent you use:

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
codex --no-daemon
```

Codex's shared daemon can outlive its originating terminal and does not reliably
carry the current terminal context into hooks. Use `--no-daemon` for con session
linking; con's own resume adapter always uses it. Hooks without a terminal-local Codex client in their process ancestry are
ignored rather than attached to a guessed surface.

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
- Initial adapters cover Claude Code and Codex on macOS. Windows/Linux retain
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
