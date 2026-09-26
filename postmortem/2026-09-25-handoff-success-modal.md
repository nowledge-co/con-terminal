# Handoff clipboard success blocked the target TUI

## What happened

After a successful manual handoff to Kimi, an “Agent Handoff” modal required
an OK click before the user could paste into the target.

## Root cause

The launch observer routed clipboard success through the same native Info
prompt helper as attention states. This made a routine instruction blocking.

## Fix applied

AwaitingManualDelivery no longer opens a prompt. Its existing job card now
shows “Instruction copied — press Cmd-V in the target session.” Reopen Agent
Handoff from the source tab to see the current card. A blocked prepare exposes its lease owner inline.
The separate native panel is independent of the target's full-screen PTY.
NeedsInteraction uses Warning; Failed uses Critical. Both stop observation.
The manual notice remains acknowledged once per observer.

The existing gpui-component Notification was considered (the 3pp checkout is
absent; the pinned Cargo checkout was inspected). The workspace only renders
its notification layer on Windows/Linux, while macOS uses an embedded native
terminal NSView. The existing job card avoids expanding overlay infrastructure.

## What we learned

A successful handoff should preserve typing flow. Keep paste instructions in
durable UI outside terminal output; reserve modal prompts for attention states.
