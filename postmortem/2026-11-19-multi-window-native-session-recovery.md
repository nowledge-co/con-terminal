# Native sessions in multiple windows

Users reported needing to resume native conversations manually after quitting
and reopening con. The reported socket or lock error has not been matched to
an exact error string. The investigation found two con-side failure modes that
affect both Claude Code and Codex when several ordinary windows are open.

Every workspace tried to listen at the same control endpoint. The workspace
that won the bind could locate its own terminals and explicitly transferred
terminals. A report from a normal secondary window could reach the listener
and find no matching origin, so the terminal never acquired a conversation link.
Separately, each window serialized its own tabs to the same session file. Even
perfectly sequential writes lost tabs from windows other than the last writer.

The process now registers all workspaces, keeps one active control listener
and routes native reports by exact terminal origin. Closing the listener
window hands the endpoint and receiver to another registered workspace. A
single worker combines window snapshots before replacing the session file.
A normal quit captures every live workspace, while closing a secondary window
removes its snapshot. The existing session format reopens the collected tabs
in one window and retains their pane, surface and native identity data.

The socket endpoint identifies the con process. The terminal origin identifies
the native session report's destination within that process. Similarly,
serializing access to a file only protects its bytes. The writer must also
know the full set of windows whose state the file represents.

In an isolated macOS preview, the earlier binary rejected a native report from
the second window with `The originating terminal no longer exists`. The fixed
preview accepted two Codex and two Claude Code fixture reports across two
ordinary windows. After quitting and reopening, the native resume helper
launched each original ID exactly once. Closing the original listener window
also left the surviving window reachable through the socket and retained only
its tabs on disk. The fixture agents made no model requests.
