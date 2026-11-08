# Portable updates

macOS keeps Sparkle. Windows and Linux share the UI-independent con-update Rust
crate and use a copy of the installed con-cli as an out-of-process replacement
helper. Application updates never download or execute install.ps1/install.sh.
Those scripts remain first-install entry points.

## Backend decision

Reviewed primary sources:

- [WinSparkle](https://github.com/vslavik/winsparkle) supports the appcast and
  Ed25519 contract, but brings its own native UI and installer handoff.
- [LinSparkle](https://github.com/medcelerate/LinSparkle) is Rust and speaks
  compatible appcasts, but targets AppImage distribution and has a Slint UI.
- [Velopack](https://github.com/velopack/velopack) offers a Rust SDK and managed
  packaging, but adopts its own package/feed layout; migration would need a
  separate installation-format project.
- [self_update](https://docs.rs/self_update/) offers executable/bundle updates,
  but does not supply our appcast, session shutdown, paired CLI, or existing
  installation-ownership contract.

Keep the signed artifacts and native macOS integration. Use established crates
for XML, SemVer, Ed25519 and archive decoding, with a narrow con-owned lifecycle.
Do not add another UI toolkit or replace all platform packaging for this feature.

## Ownership

Only canonical, nonsymlinked legacy per-user installs are adopted automatically:
Windows LOCALAPPDATA/Programs/con-terminal and Linux HOME/.local/bin. Unknown
portable layouts, symlinked launchers, system installs and package-manager
installs remain externally managed. Flatpak is detected before adoption and
points to its software manager/Flatpak. Never elevate, bypass a package manager,
or create a second installation to make an update appear successful.

## Request and preparation

The first appcast item supplies version, URL, size, signature and release notes
as one record. XML parsing respects the Sparkle namespace. Reject missing or
malformed signatures, non-HTTPS URLs, unsupported target suffixes, invalid
versions and oversized responses. A manual check can revisit a skipped version;
automatic checks respect the stored skip preference.

A unique private directory inside the installation holds staging. Download
without exiting; expose progress and cancellation. A per-installation download
lock prevents concurrent preparation. Enforce response bounds, verify the full
Ed25519 signature with a build-time public key, then extract only the application,
companion CLI and update.json identity. Reject links, traversal, duplicate
binaries and excessive expansion. Signed identity protocol 1 binds the target
version, OS and architecture to the payload, preventing a feed label from
presenting a different signed archive as a newer version.

Release workflows provide the same public key to app and CLI builds and include
update.json before signing archives. A build without a key cannot apply updates.
Old clients can still consume the archives; new updaters fail closed on legacy
archives lacking signed identity.

## Handoff and replacement

Restart is a separate explicit action after verification. Its explanation warns
that running terminals/agents close; conversation recovery follows existing
settings and does not promise process continuity.

Copy the installed CLI into staging, launch it without a console, and require
its ready acknowledgment before requesting normal application exit. The helper
re-verifies the archive and extracts fresh files; cached unpacked executables
are not trusted. A shared installation lock remains held by every running new
client. The helper waits for exclusive ownership with a deadline rather than
killing another instance. An explicit commit token authorizes replacement.

Save all current windows' tab snapshots to a con-owned pre-update recovery file
before teardown. On the next launch, restore it through the normal Session
persistence format. The current session format restores these tabs into one
window; this feature does not introduce persisted multi-window geometry.
The separate live command-history journal remains authoritative and is not
rewritten by layout restoration. Never touch native agent profiles or shell
history for updating.

Back up each binary completely and sync it before atomic replacement. Keep a
write-ahead journal and both originals until completion. If replacing the second
file fails, restore both originals. Recovery of an interrupted journal precedes
another transaction under the same exclusive lock. Only the current re-verified
plan can authorize recovery; directory-name prefixes never authorize replacing
binaries or deleting a directory. A cached prepared download
can be resumed after restarting con. A failed process launch restores originals
when exclusive ownership can be obtained; backups remain available otherwise.

The helper relaunches the app without shell interpolation. Results survive in
.con-update-result for Settings on the next start. Successful stage cleanup
renames the directory out of the recovery namespace before removing it; a
partially removed success marker must never turn a completed update into a
rollback. Never clean an active helper's staging directory.

## UI

Settings → Updates shows target-version notes using system prose typography,
semantic heading/body/credit roles, bounded scrolling and a full changelog link.
Downloaded HTML is converted to inert structured text, not executed.
Download, Cancel, Skip this version, Restart and update, and manual checks share
one state machine. No installation console or disappearing application during
download. macOS keeps Sparkle's own native UI and lifecycle.

## Verification

Run con-update tests on Windows and Linux as well as the host platform, and the
affected con-cli and con app suites. Test signature/identity mismatches, wrong
architecture, archive ambiguity, cancellation, installation ownership, shared
locks, a signed handoff, second-file rollback and interrupted recovery. Use only
temporary installations. Native macOS UI previews exercise the portable reading
surface through a debug-only fixture; they do not certify Windows/Linux runtime
behavior. No production updater endpoint or live installation is modified by
local tests.

### Native subprocess smoke

The reproducible fixture copies its own executable into a temporary installation,
signs a ZIP/tar containing a paired replacement, launches the actual copied
helper, holds a live-instance lock, authorizes exit, and checks executable
relaunch, both replacement bytes, unrelated-file retention and cleanup. It never
uses a real installation, production signing key, updater endpoint or user data.
Run it in a shell-local environment with this disposable public key:

```sh
CON_SPARKLE_PUBLIC_ED_KEY=Zs1gi5KLiOUODv6qM/rxxDzv4HKUsLh+n+CrpqPPdjM= \
CON_RELEASE_VERSION=0.1.0-beta.173 \
cargo run -p con-update --example transaction_smoke
```

On PowerShell, set the two environment variables for the command's process before
running the same cargo command. Portable CI runs this on both Windows and Linux.
The fixture validates native process/file lifecycle, not a full graphical app
update or display-server behavior.

On 2026-11-08 the same fixture source was run on Windows 11 Pro (Rust 1.97)
and Ubuntu 24.04 under WSL2, in isolated directories. Both passed the 17 updater
and 3 path tests and the real subprocess smoke, including another running
installed executable holding the lock, paired replacement and executable
relaunch. This certifies the native filesystem/process transaction on those
hosts, not the full Windows/Linux graphical Settings-to-update lifecycle.
