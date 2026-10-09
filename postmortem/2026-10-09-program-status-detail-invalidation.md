# Program-status detail invalidation

## What happened

Review of the OSC 7501 detail layer found that a program could update its
message while its tab tooltip continued showing the previous message.
This was found before the transport feature shipped.

## Root cause

The retained tab cache compared only activity, percentage, evidence and source.
Details were read separately when rebuilding the sidebar. A message-only
change did not invalidate that sidebar, so the live getter did not help.

## Fix

Retain and compare the displayed detail alongside the activity snapshot during
the existing aggregation pass. Rebuild only when either changes. Include the
actual pane/surface location, and remove cache entries when their tab closes.
The render path performs no backend reads. A regression test covers message
changes, identical updates and clearing the detail.

## Lessons

Cache invalidation must cover every visible retained value, not just the
indicator that first motivated the cache. Protocol text also needs a distinct
presentation boundary: disarm direction overrides without stripping joiners
that valid emoji and scripts require.
