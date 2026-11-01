# Closing an agent tab did not require confirmation

## What happened

Tab close buttons and keyboard actions immediately destroyed terminal surfaces.
A tab containing Codex, Claude Code or an active built-in request could therefore
be closed accidentally, interrupting work without an opportunity to cancel.

## Root cause

The close paths implemented terminal teardown, layout updates and session
persistence, but had no shared user-intent checkpoint for live agents. Tab
orientations, bulk close actions, pane actions and surface actions entered
different paths. Saved native references also could not safely identify whether
an agent was still running.

## Fix

Use the existing live terminal presentation identity, checked against the
current process query and surface incarnation, plus active built-in activity.
Collect all affected pane-local surfaces, including hidden stacked terminals.
Saved native references alone do not trigger a warning. Hold one close request
by stable tab/pane/surface identities until approval; resolve tab positions only
when executing it. Bulk closes are held together rather than partially applied.

The confirmation uses component buttons and the component focus trap in a
borderless, shadowless con overlay. Its initial Enter action and Escape keep
sessions open. Cancellation restores the original keyboard focus. Confirmation
cancels the affected built-in request and then uses established terminal teardown.
No pane geometry, transparency or native presentation dependency changes are
required. Noninteractive tab/surface close requests return a clear error instead
of claiming success while waiting for a user decision.

## Verification and lessons

Test stable request targeting after tab reordering and surface/pane ID collisions
between tabs. Exercise actual inert agent processes in isolated windows, including
cancel/confirm, batch close, stacked terminals, light/dark themes and keyboard
focus. Never use a production agent session as a destructive test fixture.

Terminal cleanup and explicit user confirmation are separate phases. Keep
low-level teardown available for already-approved operations and terminal exit
cleanup, and keep interactive and control-plane results honest about whether
anything was actually closed.
