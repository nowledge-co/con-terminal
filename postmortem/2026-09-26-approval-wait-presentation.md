# Approval wait and retained panel presentation

## What happened

Auto-approved tools could appear as pending approvals in background panels.
Timed-out approval waits could leave attention visible until request completion.
Some settings/model mutations relied on unrelated parent renders to update a
cached panel. Screen-only identity detection could miss slow-starting TUIs.

## Root cause

The harness inferred approval from tool danger instead of the hook's actual
wait. The active panel consulted live global policy while the request used a
snapshot. Setters did not consistently notify their entity. Six fast screen
reads exhausted the detection budget before a delayed banner appeared.

## Fix applied

Emit approval-needed/ended at the hook wait boundary, and clear by channel plus
call ID. Clear/truncate explicitly deny pending approvals. Each panel setter
notifies only on change. Title handlers use retained event data. Six additional
exponentially spaced screen attempts cover slow startup without unbounded
polling or resetting the budget for unrelated child processes.

## What we learned

Presentation must consume lifecycle facts from the authority that owns them.
Request-time policy must not be replaced by a later UI setting. Entity caching
requires mutation-owned invalidation. Bounded retry needs both an attempt budget
and a useful time horizon; app-wide wakeups are not evidence of surface output.
