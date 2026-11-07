# Documentation and release bookkeeping triggered excessive CI

## What happened

During beta.171 publication, main repeated native checks already passed on the
identical PR tree and started a two-architecture unstable Flatpak build ahead of
the beta tag. These three redundant main runs were cancelled after confirming
tree equality and keeping the tag release runs intact.

## Root cause

Flatpak had no main-push path filter. Every main change could start a full
package build, including changelog bookkeeping. Portable CI also included all
`docs/**` changes just to run a small manifest validator. Read-only workflows
lacked supersession concurrency, so new PR commits did not stop older work.
The existing E2E filter already excluded standalone changelog changes; it was
not accurate to attribute every observed run to the date-only commit.

## Fix and lessons

- Filter Flatpak branch pushes by actual build inputs; preserve tag, scheduled
  and manual publication and the existing non-cancelling publication lock.
- Move documentation/CI policy checks into a lightweight Ubuntu workflow.
- Cancel superseded read-only checks by workflow and PR/ref, without cancelling
  release builds or finalizers.
- Keep source, dependency, asset and toolchain coverage, and test mixed changes
  rather than trusting commit messages. A PR diff is cumulative.
- Use path filters instead of `[skip ci]` so useful public docs updates remain.

Policy tests load the actual workflow YAML and cover docs-only exclusions,
mixed source/docs, real packaging inputs, release tags, manual/scheduled builds
and concurrency isolation. actionlint validates the workflow schema. Neither
local tool alone is a hosted event-dispatch test.
