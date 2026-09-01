# Implementation: Socket API and `con-cli`

## Overview

con now ships a real local control plane:

- the app listens on a Unix domain socket
- requests use newline-delimited JSON-RPC 2.0
- `con-cli` is the first client on top of that socket

This is the first automation slice, not the final surface. The important architectural move is that the CLI does not invent new terminal semantics. It routes into the same pane runtime, tmux adapter, visible-shell execution guard, and per-tab agent session that the built-in UI already uses.

That keeps CLI automation honest: if a pane is not a proven shell in the app, `con-cli panes exec` is refused for the same reason.

## Local references that shaped the design

The CLI shape is intentionally informed by the local 3pp references in this repo:

- `3pp/agent-browser` — flat agent-friendly command ergonomics, strong `--json` output, and a CLI that is useful both for humans and other agents
- `3pp/waveterm/cmd/wsh` — app control over a local socket with a small RPC boundary instead of bolting automation directly onto UI code
- `3pp/cmux/CLI/cmux.swift` — explicit socket-path discovery and the idea that the socket client should stay thin while the app remains the authority

The result in con is a hybrid:

- user-facing CLI commands are grouped by domain: `tabs`, `panes`, `tmux`, `agent`
- the transport methods are namespaced JSON-RPC methods such as `panes.list` and `agent.ask`

## Socket path

- Release default: `/tmp/con.sock`
- Debug default: `/tmp/con-debug.sock`
- Override: `CON_SOCKET_PATH`
- CLI override: `con-cli --socket /custom/path ...`

The app removes stale socket files on startup and creates the live socket with
user-only permissions. Debug builds intentionally use a separate default
endpoint so `cargo run -p con` can run beside an installed Con without making
startup treat the dev process as another window for the production app.

## Protocol

JSON-RPC 2.0 over a Unix domain socket, one JSON request per line and one JSON response per line.

### Request

```json
{"jsonrpc":"2.0","id":"req-1","method":"panes.read","params":{"tab_index":1,"pane_id":3,"lines":80}}
```

### Response

```json
{"jsonrpc":"2.0","id":"req-1","result":{"content":"..."}}
```

### Error

```json
{"jsonrpc":"2.0","id":"req-1","error":{"code":-32602,"message":"Pane index 9 is out of range. Valid panes are ..."}}
```

## Current method surface

### System

- `system.identify`
- `system.capabilities`

### Tabs

- `tabs.list`

### Panes

- `panes.list`
- `panes.read`
- `panes.exec`
- `panes.send_keys`
- `panes.create`
- `panes.wait`
- `panes.probe_shell`

### Tree / Surfaces

- `tree.get`
- `surfaces.list`
- `surfaces.create`
- `surfaces.split`
- `surfaces.focus`
- `surfaces.rename`
- `surfaces.close`
- `surfaces.read`
- `surfaces.send_text`
- `surfaces.send_key`
- `surfaces.wait_ready`

### tmux

- `tmux.inspect`
- `tmux.list`
- `tmux.capture`
- `tmux.send_keys`
- `tmux.run`

### Agent

- `agent.ask`
- `agent.new_conversation`

## CLI surface

The first shipped client is `con-cli`.

The same binary also provides the macOS shell-integration compatibility
commands `+ssh` and `+ssh-cache`. Their protocol and fallback behavior are
documented in [`ssh-integration.md`](ssh-integration.md).

Examples:

```bash
con-cli identify
con-cli capabilities

con-cli tabs list
con-cli panes list --tab 1
con-cli panes read --tab 1 --pane-id 3 --lines 120
con-cli panes exec --tab 1 --pane-id 3 cargo test -q
con-cli panes send-keys --tab 1 --pane-id 3 $'\u0003'
con-cli panes create --tab 1 --location right --command "htop"
con-cli panes wait --tab 1 --pane-id 3 --timeout 20

con-cli tree --tab 1
con-cli surfaces list --tab 1
con-cli surfaces split --tab 1 --pane-id 3 --location right --title worker-1 --owner subagent --command "codex"
con-cli surfaces create --tab 1 --pane-id 4 --title worker-2 --owner subagent --command "codex"
con-cli surfaces wait-ready --tab 1 --surface-id 7 --timeout 10
con-cli surfaces focus --tab 1 --surface-id 7
con-cli surfaces send-text --surface-id 7 "explain this repo"
con-cli surfaces send-key --surface-id 7 enter
con-cli surfaces read --surface-id 7 --lines 120
con-cli surfaces close --surface-id 7 --close-empty-owned-pane

con-cli tmux list --tab 1 --pane-id 3
con-cli tmux capture --tab 1 --pane-id 3 --target %17 --lines 80
con-cli tmux run --tab 1 --pane-id 3 --location new-window -- cargo test

con-cli agent ask --tab 1 "Summarize what is happening in this tab"
con-cli agent ask --tab 1 --auto-approve-tools "Run the test suite and explain failures"
con-cli agent new-conversation --tab 1
```

For automation, every command also supports `--json`.

## Request routing rules

### Tabs are explicit

- `tab_index` is 1-based in the control API and CLI
- omitting `tab_index` means "use the active tab"

### Pane targets reuse con's stable pane model

- `pane_index` is the current visible position in the split layout
- `pane_id` is the stable identity for the life of that pane inside the tab
- follow-up automation should prefer `pane_id`

### Surfaces are opt-in pane-local sessions

- a pane is a visible split rectangle
- a surface is a live terminal session inside a pane
- every pane has one active surface
- `panes.*` continues to operate only on the active surface
- `surfaces.*` exposes additional pane-local tabs without changing pane
  semantics for the built-in agent or existing benchmarks
- follow-up surface automation should prefer `surface_id`

See `docs/impl/pane-surfaces.md` for the full compatibility contract.

### Pane actions reuse existing guards

- `panes.exec` goes through the same visible-shell execution path as the built-in agent
- `tmux.*` methods only work when the pane exposes native tmux capability
- `panes.probe_shell` only works when the pane exposes `probe_shell_context`

That is deliberate. The CLI is not allowed to bypass the control-plane truths that the UI and agent already depend on.

## Agent session behavior

`agent.ask` targets a tab's existing built-in agent session.

That means:

- it reuses the tab's conversation history
- it reuses the tab's focused-pane context and peer-pane summary
- the response also appears in con's own agent UI for that tab

This is what makes CLI-driven end-to-end evaluation possible. A coding agent outside the app can drive panes, inspect tmux state, call the built-in agent in a real tab session, and verify the visible result without a human acting as the bridge.

## Threading model

- socket accept/read/write runs on the harness Tokio runtime
- JSON-RPC parsing happens off the GPUI render path
- requests are bridged onto the workspace over a channel
- pane/tmux operations still execute through the workspace, because the workspace owns live panes
- long-running results are delivered back asynchronously over one-shot replies

The render loop stays authoritative, but the socket never blocks on it directly.

## Known limits of the first slice

- no subscription or streaming RPC yet; requests are request/response only
- no window-focus or pane-focus RPC yet
- no auth modes beyond local socket file permissions yet
- `agent.ask` assumes one automation request at a time per tab
- the socket surface only exposes capabilities that already exist in con's runtime and agent layers; it does not yet add a new app-native Codex/OpenCode attachment
- pane-local surfaces are live-session control primitives; session restore
  persists the active pane layout as before, not every hidden live surface

Those are acceptable limits for phase one. The important part is that the control transport now exists and is wired to real product semantics instead of placeholder CLI text.

## Live E2E status

Live app-backed smoke tests were rerun on April 10, 2026 against a real `cargo run -p con` session.

Verified end to end:

- `con-cli identify`
- `con-cli tabs list`
- `con-cli panes list`
- `con-cli panes read`
- `con-cli panes exec`
- `con-cli panes wait`
- `con-cli agent ask`

Verified behavior notes:

- `panes exec` now recovers cleanly when the visible prompt is back but Ghostty shell integration does not emit a matching command-finished signal fast enough for automation. That keeps the pane usable for follow-up `panes exec` calls in the same session.
- `agent ask` was verified against a real configured model session, not just the transport boundary.

Current known limitation from the same live run:

- `panes.create` returns promptly with `tab_index`, `pane_index`, `pane_id`, `surface_ready`, `is_alive`, and `has_shell_integration`. Its `observation_support` object distinguishes unsupported process metadata from temporarily unavailable values; on macOS, `foreground_process_group_id` and `tty_name` are supported. Use `pane_id` for follow-up targeting, then confirm the created pane exposes the exact control capabilities you need before driving it further.
