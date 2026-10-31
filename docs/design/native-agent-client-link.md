# Native agent client links

The durable unit is one native conversation in one terminal surface. Tabs group
panes; each pane can contain several surfaces. Linking never chooses the most
recent history or manufactures a native ID from activity, a process name or cwd.

## Current Codex compatibility

Codex 0.166 executes core hooks in the backend process. A shared daemon can
outlive its originating terminal and serve several TUI clients. Its ambient
environment cannot identify the current terminal. Herdr's native hook uses that
environment too; copying it would reproduce this failure.

The maintainer accepted terminal-local execution as the current compatibility
policy on 2026-10-31. One explicit `native-session install --agent codex` installs
the native hook and a con-only shell launcher. Users then type `codex` normally.
The launcher selects `--no-daemon` only for local interactive use, preserves the
native argv and uses exec so signals and exit status remain native. Native hooks
still confirm the exact ID. It sends no synthetic model turn to obtain an ID.
Native trust is never approved by the installer.

The shell block is scoped to the installed profile, reversible and backed up.
Aliases and functions retain priority. Explicit remote servers, noninteractive
commands, nested native tools and profiles without the integration bypass the
policy. No global daemon feature is disabled and con does not host a daemon.
Automatic recovery uses fixed allowlisted native arguments, not saved commands.

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
