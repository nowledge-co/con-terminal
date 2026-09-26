# Status review: evidence, request lifetime and focus boundaries

## What happened

A branch-wide review found four presentation bugs before publication:

- A newly discovered process identity erased an unchanged attention title.
- A pending process result could survive an A→B→A query transition.
- A Windows terminal launched directly into an agent omitted that root process
  from identity candidates.
- Equal-severity progress from different panes could retain the old focused
  source until the one-second collector backstop.

These affect presentation, not tool approval or shell-control authority.

## Root cause

Identity changes invalidated all title evidence rather than only reports that
depended on identity. Async acceptance compared query values without comparing
their revisions. Windows traversal assumed the root was a shell. Focus-dependent
aggregation shared the external-observation collector's scheduling boundary.

## Fix applied

Preserve independent attention/motion observations and their original timestamps;
check request revision at completion; include a validated Windows root within
the existing candidate budget. Separate cached aggregation from evidence
collection and reaggregate on UI data/focus invalidation before rendering.

## What we learned

Data dependency determines invalidation, not the proximity of two fields.
Value equality alone is insufficient for async lifetime validation. GPUI marks
rendered ancestors dirty on child animation notifications, but does not notify
their self-observers. Cached preparation and fresh focus aggregation therefore
need explicit, separate contracts.

Regression tests cover attention across identity changes, unchanged motion
leases, stale query revisions and Windows root/bound cases. The attention test
failed before its fix. Windows tests were type-checked cross-target, not executed
on macOS. A live two-pane check with 21% and 83% progress confirmed the selected
tab line follows focus; screenshots were captured after each focus change.
