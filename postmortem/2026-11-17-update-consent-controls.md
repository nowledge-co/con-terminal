# Update checking and installation consent (#531)

## What happened

Settings reported “Auto-update enabled” but offered no way to disable background
checks. The label also conflated discovering a release with permission to
replace the application or interrupt running terminal/agent processes.

## Cause

The update status modeled backend availability rather than the user's policy.
macOS bundles defaulted Sparkle automatic checks on but exposed no binding to
its real persisted preference. Portable clients checked at launch without a
persisted opt-out. Sparkle's standard optional silent-update behavior was not
explicitly constrained by con's per-install consent policy.

## Change

Expose an immediate, persistent “Automatically Check for Updates” switch and
keep manual checks available while it is off. On macOS use Sparkle's public
main-thread setting APIs, without creating a second source of truth or resetting
user preferences at launch. Bundle policy explicitly disallows automatic
installation/download even if an older preference enabled it; do not erase that
stored preference. Portable clients persist their check preference atomically,
reject automatic fetches when disabled, and cancel an active automatic fetch
without canceling user-initiated checks or downloads. Downloads and restart/
replacement keep their existing explicit actions. Package-managed installs
remain owned by their package manager.

## Verification

The macOS application suite passed 757 tests. Focused updater/settings checks
cover the final source. A linked probe against the real Sparkle framework checks
preference persistence, manual-check availability while off, opt-out, and the
bundle's override of a legacy silent-update opt-in. Packaging fixtures assert
the emitted plist policy. Windows/Linux CI now includes all updater tests,
including the new preference persistence/cancellation cases. Isolated native UI
checks in light/dark verified toggling, persistence across app restart, a manual
Sparkle check while automatic checks are off (expected local-feed error), and
retaining an explicitly prepared portable update after disabling checks. Both
fixture apps were quit afterward. Native Windows/Linux GUI update lifecycle is not certified by macOS tests or a portable UI preview.

## Lessons

Represent backend availability, checking policy and installation authorization
separately. Bind native preferences to the actual framework. A preference must
survive restart, report save failures honestly, and not require a config Save
button when its owner is a separate native settings store. An opt-out from
future checks must never silently cancel an already-authorized installation.
