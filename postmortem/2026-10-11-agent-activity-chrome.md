# Agent activity chrome and collapsed approval controls

## What happened

The running agent sidebar repeated activity indicators in its header, live
summary, transcript footer, and tool rows. Full-width and inset rules created
uneven boundaries, while the small uppercase label sat too close to a tool
name and inline arguments. An isolated live preview also showed an approval
summary without visible approval actions.

## Root cause

Independent render sections each added their own spinner rather than sharing
a presentation hierarchy. The live summary squeezed labels and parameters
onto one row. Separately, the transcript footer requested full height inside
an auto-height, max-height-constrained parent; the percentage height could
collapse instead of following the approval card's intrinsic size.

## Fix

Use one animated live summary below the session controls. Approval uses a
static shield; trace rows retain their tool icons. Give tool arguments a
separate wrapping monospace line and remove the competing header/summary
rules and tiny live-status kicker. The model control consumes remaining
header width; provider identity lives in its icon and tooltip.

Let the footer measure its content and retain its 320px scrolling limit.
Do not change approval dispatch, agent state transitions, streaming events,
markdown caches, or virtualized message rows.

## Verification and lessons

Add layout regression coverage for narrow and enlarged-text live summaries
and for nonzero approval-footer height. Run existing composer, approval,
completion, and Stop tests. Native visual checks use an isolated app and a
localhost provider fixture, not the user's conversations or credentials.
Windows/Linux desktop appearance still needs native validation.

The preview exercised thinking, approval, streamed markdown, Stop, and idle
with a narrow sidebar in Flexoki Dark and Light. Its terminal command was
refused by the existing unproven-shell safety gate; this was not a successful
terminal-execution test. Windows/Linux CI runs the panel regression suite,
but does not replace desktop visual acceptance.

Percentage height belongs to a container with an externally determined
height, not an intrinsic tool/approval footer. Keep one owner for activity
animation, and verify the actual controls behind a status label rather than
stopping at an attractive summary.
