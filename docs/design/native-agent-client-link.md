# Native agent client links

The durable unit is one native conversation in one terminal surface. Tabs group
panes; each pane can contain several surfaces. Linking never chooses the most
recent history or manufactures a native ID from activity, a process name or cwd.

## Current Codex compatibility

Codex 0.166 executes core hooks in the backend process. A shared daemon can
outlive its originating terminal and serve several TUI clients. Its ambient
environment cannot identify the current terminal. Herdr's native hook uses that
environment too; copying it would reproduce this failure.

The maintainer accepted terminal-local execution as the compatibility policy
on 2026-10-31, then required default-on recovery without modifying native profiles
or shell startup files. con now prepares only con-owned launchers and shell
bridges. Normal interactive `codex` receives `--no-daemon`, runtime `--enable
hooks` and a SessionFlags hook declaration. Hook discovery visits each native
configuration layer separately, so user/project hooks remain independent of the
con declaration. Native hook trust remains native. No synthetic model turn is
sent solely to obtain an ID.

Claude Code loads a con-owned hook-only plugin through its repeatable
`--plugin-dir` session option. User `--settings`, plugins, hooks, profile paths,
permissions and native history are retained. This follows its documented
[session-only plugin loading](https://code.claude.com/docs/en/cli-reference#cli-flags)
and [hook merging](https://code.claude.com/docs/en/hooks#hook-locations).

The terminal-local shell bridge chains the shipped Ghostty integration and user
startup files rather than rewriting either. Its per-surface environment restores
the effective user ZDOTDIR after startup. Con-owned resources are idempotent,
private and contain no copied user settings or credentials. Aliases/functions,
absolute native paths, unsupported shells, batch, remote and explicitly disabled
native customization modes retain their existing behavior.

Automatic recovery uses fixed allowlisted native arguments, not saved commands.
Cursor/Qwen/Copilot retain explicit hook adapters until verified session-local
injection contracts can replace their opt-in native profile installation too.
The legacy `native-session install` command remains an explicit user action;
automatic startup and launches must never call that installer.

## Upstream replacement contract

The correct long-term outlet is an opt-in **TUI client** session attachment
callback. Codex already receives the confirmed thread ID, cwd and rollout path
in `tui/src/chatwidget/session_flow.rs`; core hook construction captures backend
environment in `hooks/src/registry.rs`. These were reviewed at upstream
[`6ff1e1c`](https://github.com/openai/codex/tree/6ff1e1ca3b30fc8b914720681fcf5fe62e95f856).
No such general configurable client callback exists in that version.

A future integration must define:

- Attached/changed/detached events for new, resume, clear, fork and thread switch.
- Exact native ID, local store namespace, cwd and optional native history path.
- Client-local inherited con origin/socket, without forwarding the full client
  environment, credentials or terminal control capabilities into a shared server.
- Explicit local versus remote backend scope; remote metadata cannot create a
  locally resumable reference.
- Independent attachment and model-turn lifecycles. An idle client must be able
  to identify its conversation without making a paid request.

When an upstream release supplies and passes this contract, replace the local
execution policy behind the adapter and remove its ancestry workaround. Migrate
existing IDs in place; do not copy or fork native histories. Keep native model,
trust and permission semantics unchanged. This is a proposed upstream interface,
not a claim that con has already added a Codex feature.

## Acceptance gates

Use isolated profiles and terminals. Start a shared daemon outside con before
testing two con surfaces with the same cwd/store. Verify exact distinct IDs,
resume/fork/switch/clear, one client exiting while the other stays live, old
origin rejection after restart, native history availability, manual/automatic
recovery, profile isolation, paths with spaces and explicit remote refusal.
Keep protocol fixtures separate from actual native CLI and paid-model results.

Additional agents enter the shared allowlist only with a verified native ID,
store contract, lifecycle hook and fixed exact-ID resume command. A logo or a
Herdr command mapping alone is insufficient for a supported integration.
