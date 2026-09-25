# Handoff C fallback guidance

## What happened

Fallback jobs showed long clipped errors and put Abandon ahead of the manual
completion action. Kimi stopped observing as soon as it needed user help; the
panel also cached the job, so manual paste had no visible progress feedback.

## Root cause

Fallback mixed failure diagnostics, clipboard status and user instructions in
one error string. Delivery observation and the recovery UI had no shared evidence
model. Codex/Cursor recovery omitted the copy action, and helper stdout guidance
was printed immediately before a TUI could overwrite it.

## Fix applied

Add a typed seven-kind fallback guide, a bounded read-only Kimi observer and
explicit A2 confirmation. Refresh the displayed job outside render, use three
short steps and reorder actions. Keep copy status truthful when backup fails,
allow copying for all supported targets, wrap card text and leave instructions
in the persistent panel. Keep helper diagnostics on stderr.

## What we learned

Screen detection, delivery receipts and human confirmation are separate facts.
A paste is not a submitted turn: it only enables the user's confirmation. Async
observers need both state and revision guards and a fixed observation deadline.
A persistent guide also needs a refresh path; changing the renderer alone cannot
show new evidence. Automated state/text tests do not replace desktop layout QA.
