# A temporary Codex thread replaced a pane's saved conversation

## What happened

After a restart, Con tried to resume a UUID that Codex had used earlier. The
UUID was absent from both the native session index and the history store, so
Codex refused to resume it. The logs did not identify the action that triggered
the original incident. We later reproduced the failure with `/recap`.

## Cause

Con treated a Codex `SessionStart` event without a transcript path as a
resumable conversation. Both persistent conversations with paginated history
and temporary threads can omit the path. When a temporary thread reported
its UUID in the same terminal, Con replaced the pane's saved conversation.

## Fix

When the hook omits the path, con-cli checks the writer lock for the reported
UUID in the recorded native store. A persistent local thread holds an exclusive
lock while the hook runs. A temporary thread skips history-writer initialization.
The probe opens the existing lock file read-only and never waits for a lock.
It neither scans the store nor reads the conversation. At restore time, Con
continues to pass the saved UUID to Codex.

## Verification

In an isolated macOS profile, we completed a turn in the Codex CLI 0.172.0 TUI
and ran `/recap`. A local test server supplied the model responses. Both
versions of con-cli received the same native hook events. The old helper
reported the main UUID and subsequent temporary UUIDs. The patched helper
reported only the main UUID. Running `codex resume <UUID>` reopened the
original reply for the main conversation. The recap thread returned
`No saved session found`.

Codex supplied a transcript path for the main conversation in that run. To
check compatibility with #570, we repeated the test with that field removed
from the hook input, keeping the real writer lock and history store. The
patched helper still reported the main UUID, and Codex reopened the reply.

In a separate Con GUI test, we started a persistent thread and a temporary
thread in the same pane. The layout on disk retained the persistent UUID,
and the original conversation reappeared after restart.

The regression test checks the exact UUID against a live writer, a temporary
thread in the same store, and a released lock file. It also checks resume
validation after the writer exits. Recheck the native lock behavior on Codex
upgrades because the lock layout is part of its local store implementation.
