# Resume native agent conversations

con can keep a supported native agent conversation linked to its exact terminal
surface, then resume that conversation after con or the machine restarts.
This first implementation supports local macOS terminals. The native CLI and
its history must remain installed on the same machine.

## Start normally

On macOS, conversation recovery is on by default. With the default zsh shell, open a con terminal and run
`claude` or `codex` normally. Their native hooks apply only to that launch:
Claude loads a con-owned local plugin with `--plugin-dir`; Codex uses its
session-flags configuration layer and terminal-independent `--no-daemon` mode.
Native hook and workspace trust prompts remain native. No setup command is
required, and con does not edit their profiles, settings, authentication or
history, or your shell startup files.

con keeps its launchers and shell bridge in its own application data directory.
The bridge runs your ordinary zsh startup files and the installed
Ghostty shell integration, then exposes the launchers only in that con terminal.
It restores the user's effective zsh configuration directory after startup.
The native CLI still resolves the actual profile, model, permissions and history
from your normal environment and arguments. No profile or credential copies are
made. Explicit remote clients, batch commands and nested native tool calls pass
through. User aliases/functions and absolute binary paths retain their behavior
and can bypass the launcher.
The automatic shell entrypoint currently covers zsh. System Bash 3.2 does not
run the required ENV startup bridge; Bash/fish integration is left unchanged
until a verified bridge preserves their native startup behavior. Existing
explicit native hooks remain usable in those shells.

For Codex 0.166, terminal-local execution is necessary because shared-daemon
hooks cannot identify the TUI client. This compatibility layer preserves native
history, arguments, signals and exit status until upstream supplies client-side
session attachment events; see [the design](design/native-agent-client-link.md).

An old conversation started before integration must be explicitly resumed once
in a new con terminal to establish its link. con never guesses one from recent
history. Other shells and explicit native safe/bare/ephemeral modes do not
acquire automatic links. Missing native history never creates a new conversation
as a substitute.

The native recovery registry also includes Cursor Agent, Qwen Code and GitHub
Copilot. These additional adapters currently require explicitly installed native
hooks; automatic profile-free launch integration covers Claude Code and Codex.
All adapters use exact native resume flags:

| Agent | Install name | Native store override | Exact resume |
|---|---|---|---|
| Claude Code | `claude` | `CLAUDE_CONFIG_DIR` | `claude --resume ID` |
| Codex | `codex` | `CODEX_HOME` | `codex resume ID` (con selects local mode) |
| Cursor Agent | `cursor` | `CURSOR_CONFIG_DIR` | `cursor-agent --resume ID` |
| Qwen Code | `qwen` | `QWEN_HOME` | `qwen --resume ID` |
| GitHub Copilot | `copilot` | `COPILOT_HOME` | `copilot --resume=ID` |

For an explicitly requested legacy/profile integration, the companion CLI still
provides `native-session install --agent NAME [--config-dir /absolute/path]` and
`uninstall`. Those explicit commands modify native settings, with private backups;
Codex installation can also edit a specified shell startup file. They are never
called by the automatic launch path. Cursor requires one local
workspace root and native transcripts enabled; its native `XDG_CONFIG_HOME/cursor`
fallback is respected when `CURSOR_CONFIG_DIR` is unset. Qwen histories must remain within
`QWEN_HOME`; a separate `QWEN_RUNTIME_DIR` outside that store is currently refused
at recovery. Unsupported or missing history never falls back to a recent session.

SessionStart links the exact native session ID, working directory, native store
and history path to that surface. It does not collect prompt text, credentials
or transcripts into con's layout file. The native agent owns its conversation
history. OSC 7501 activity reports and process names do not establish this link.

## Choose how to restore

By default, con restores the workspace and resumes valid linked conversations.
To restore only the layout without relaunching native agents, turn off
**Settings → General → Continuity → Resume Agent Sessions** and save Settings,
or set:

```ini
con.appearance.resume_agent_sessions = false
```

This preference is independent of terminal screen replay. Explicit `false`
settings are preserved. With automatic recovery disabled, linked conversations
remain available through **Resume Agent Session** in the terminal/surface menu:

```sh
con-cli --json surfaces list --tab 1
con-cli --json surfaces resume-agent --tab 1 --surface-id EXACT_SURFACE_ID
```

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
