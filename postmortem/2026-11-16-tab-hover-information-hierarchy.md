# Tab hover information hierarchy

## What happened

Multi-pane hover cards repeated project names and long working directories with
little distinction between pane identity and metadata. Initial visual refinements
changed spacing without helping users locate same-named terminal panes. Single-pane
cards could repeat only the tab heading because pane details were collected only
when more than one pane existed.

## Cause

The presentation lacked spatial context, and the metadata collection gate encoded
a visual assumption rather than the availability of useful pane information.

## Change

Collect details for single-pane tabs too. Multi-pane cards show a shared directory
once, retain distinct task names and paths, and pair each row with a schematic
from actual pane bounds. Focus uses a quiet row fill rather than an unexplained
badge. Single-pane cards retain directory metadata. Paths retain a distinguishing
Unicode suffix and expose the original directory to accessibility.

## Verification and lessons

35 sidebar tests and 17 terminal-status tests pass, including both themes,
long paths, shared directories, and focus/layout-only invalidation. App and CLI
builds pass on macOS. The reporter supplied actual light-theme hover screenshots;
native Windows/Linux GUI and a dark-theme desktop capture were not exercised.

Design hierarchy must add useful distinctions rather than nested containers.
Collect presentation facts independently of current density or pane count.
