# Pane tab and file-menu material drift

## What happened

Light-theme file and terminal-surface tabs used heavy selected fills and slightly
different geometry. Inactive hover could look stronger than selection. The new
file-copy menu inherited the component library's ring and shadow, which looked
detached from con's quiet pane surfaces.

## Root cause

File and surface tabs duplicated their material values. Component menu behavior
and its default visual treatment were being accepted together, although con's
surface language differs from the component defaults.

## Fix

Share pane-tab height, state fills and close controls without changing the pane
header height or terminal clipping. Render the existing PopupMenu with a refined
root surface, keeping its items, keyboard handling, dismissal and focus return.
Observe menu updates so hover/keyboard selection still redraws. No dependency
source or native terminal corner path changes are needed.

## Lessons

Selection must remain stronger than hover in both themes and unfocused panes.
When upgrading the pinned component library, verify the menu render-root adapter,
cursor anchoring, viewport clamping, copying, Escape and focus restoration.
