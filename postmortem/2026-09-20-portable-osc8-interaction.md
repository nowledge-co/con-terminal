# Portable OSC 8 interaction and preview

## What happened

Windows and Linux detected visible URLs but did not resolve OSC 8 targets.
During implementation review, a stationary hover could arm a substituted URI,
and moving between cells of the same link canceled activation. Native Linux
verification also found a collapsed URI preview and invisible denial notices,
despite passing interaction tests.

## Root cause

The portable views did not query Ghostty's hyperlink API. Hit-cell geometry
was being used as link identity, while the press path did not compare its new
target with the prior preview. The absolute preview had no explicit size bound
from its pane. GPUI-component Root owns notification state but does not mount
the notification display layer automatically.

## Fix applied

Copy the hit cell's URI under the parser lock and apply the existing OSC 8
policy before opening. Retain owned decisions, never grid references. Recheck
targets on press and release; consume a substituted-target press without
opening, and compare OSC 8 activation by URI rather than cell rectangle.
Keep plain URL detection and terminal mouse reporting behavior unchanged.

Use an intrinsic-width preview bounded by the existing pane geometry and
mount the library notification layer on Windows and Linux. No polling, new
configuration, dependency upgrade, or full-grid hyperlink scan is needed.

## What we learned

Removing URI lookup makes both new VT tests fail. Removing the stale-preview
guard or replacing URI equality with rectangle equality fails the corresponding
interaction regression. Keep these checks separate from native rendering:
successful opener interception proves routing, not readable preview text or
visible rejection feedback. Native acceptance must inspect allowed and blocked
previews, denial notices, narrow panes, and long targets as well as click,
selection, mutation, and mouse-reporting behavior.
