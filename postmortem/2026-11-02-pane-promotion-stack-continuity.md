# Pane promotion must move the intact surface stack

## What happened

While implementing drag transfers between tabs, panes and windows, inspection
found that the existing pane-to-tab path collected surface descriptions and
rebuilt a tree with the active surface first. A pane whose second surface was
active could therefore change its surface order when promoted. Editor panes
were not transferable by that terminal-only reconstruction path.

## Root cause

Promotion reused a description of terminal surfaces instead of transferring the
owned pane node. Description-to-tree reconstruction did not carry all of the
pane's navigation state and treated focus order as storage order.

## Fix

Extract the intact leaf and promote it directly. Joining a tab into another
layout remaps tree-local IDs while retaining the ordered stack, its active
surface, terminal/editor entities and metadata. Pane-scoped runtime observations
and shell history follow the corresponding pane IDs.

## Lessons

A presentation list is not a complete ownership representation. Layout moves
must use the owned tree and never invoke close/termination. Verify active-first
ID collisions, hidden surfaces, edited files and process identity in addition to
the visible terminal. Cross-window ownership also includes event subscriptions,
native backing observers and native-session report routing, not only NSView
parentage.
