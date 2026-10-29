# Implementation: Build System

## Flatpak Icon

Flatpak uses a 256px derivative of the canonical Tail A6 artwork. Passing the
full-size artwork directly creates a nonstandard hicolor directory that
AppStream does not discover. After changing the artwork, regenerate the asset:

```sh
sips -z 256 256 assets/app-icons/con-raccoon-tail-A6.png --out packaging/flatpak/con-raccoon-tail-A6.png
shasum -a 256 assets/app-icons/con-raccoon-tail-A6.png
```

Update the `source-sha256` comment in `packaging/flatpak/aetherpak.yaml`, then run
`python3 scripts/release/test-flatpak-icon.py`. Native Flatpak builds still run
AppStream composition and the bundle smoke test; the local size check does not
replace those checks.

## Updater Publication

Platform workflows upload release assets to a draft and prepare signed feeds at
`release-staging/<tag>/appcast/` on this repository's `gh-pages` branch. These
staging paths are public metadata, not updater endpoints; clients continue to
read the existing `appcast/` feeds. A missing signing key or artifact fails the
platform workflow instead of silently preparing an incomplete release.

The finalizer reconciles the latest push run for each exact tag and commit.
SHA-filtered Actions results can lag; it also lists recent runs and filters
locally. Missing results and API errors receive bounded retries, then fail
visibly. An observed running platform defers to that platform's completion
event. A failed platform never passes the gate.

After validating all assets, checksum manifests and staged feeds, the finalizer
publishes the release. It checks the exact enclosure URLs anonymously, including
their lengths, before merging the staged entries into the canonical feeds.
Draft assets therefore never appear in the updater's current feed. Retry merges
preserve newer entries and unrelated Flatpak/Pages content. A failure after
promotion is recoverable: a public release still runs the feed validation and
publication steps. Homebrew updates run only after these steps succeed.

For releases made with this staged workflow, recover without rebuilding or
creating another tag:

```sh
gh workflow run release-finalize.yml --ref main -f tag=v0.1.0-beta.N
```

Older tags lack staged feeds; rerun their original finalizer only after checking
their payload and canonical feeds. Never manufacture staging data from an
unverified draft. Local tests use isolated fixtures and a local bare Git remote:

```sh
python3 scripts/release/test-publication.py
actionlint .github/workflows/release-*.yml
```

See [the beta.148 incident](../../postmortem/2026-10-23-updater-draft-download.md).

## macOS Release Handoff

Dependency setup reuses the runner's installed CMake and logs its version.
If it is absent, Homebrew must supply a bottle without auto-update or incidental
upgrade. An unavailable bottle fails rather than compiling CMake from source.
Zig remains explicitly pinned to the project version.

## CI Zig Downloads

macOS and Linux release, portable and E2E jobs share
`scripts/ci/install-zig.sh`. It tries the official distribution first, then two
independent mirrors. Each source gets one attempt with a 10-second connection
timeout, a 180-second total limit and a 30-second low-speed timeout. A 12-minute
workflow step deadline bounds the entire installation, including extraction.
The script logs each source, checks the archive against the pinned SHA-256 from
Zig's official download index, and verifies the executable's version before
adding it to the runner PATH. Partial downloads never count as success.

Update the script's version and all supported host checksums together with
`mise.toml`. Run `bash scripts/ci/test-install-zig.sh` for stalled-transfer,
corrupt-download, source exhaustion and PATH safety regressions. Windows keeps
its separate PowerShell installer and existing request timeout.

## Native Packaging

### Homebrew Casks

The release finalizer generates both casks from its inline template, after the
release and its updater downloads are publicly available.
Use `depends_on :macos` for the platform requirement: old macOS release symbols
can disappear from Homebrew even when Ruby syntax still parses correctly.
This does not change the app binary's deployment target.

Run `brew ruby scripts/macos/test-homebrew-cask.rb` to load the actual template
with Homebrew's DSL for beta/stable and ARM/Intel metadata. The macOS packaging
fixture job runs the same test. It verifies the platform requirement, version,
architecture URL and checksum without tapping, installing, downloading or
executing an app. This is a cask-loading check, not a complete install/upgrade
test. See [the incident report](../../postmortem/2026-10-19-homebrew-macos-requirement.md).

### App Bundles

The release workflow compiles ARM and Intel app bundles on their respective
native runners. Signing, notarization, stapling and DMG creation run in separate
ARM jobs. Intel binaries are not cross-compiled; Rosetta is used only to execute
the packaged Intel companion CLI's smoke check.

`scripts/macos/release.sh` normally builds locally as before. CI supplies
`CON_PREBUILT_APP_ARCHIVE`, a same-workflow native bundle archive. Preparation
checks channel, bundle ID, marketing/build version, both binary architectures,
CLI compatibility symlink and Sparkle presence before replacing the destination.
Signing and all release gates remain required. Internal archives never become
GitHub release assets. Packaging can be retried independently while its native
bundle artifact is retained.

The split follows repeated Intel-runner ticket-download failures despite
accepted notarization; it does not establish the cause of the network fault.
See [the incident report](../../postmortem/2026-10-14-macos-packaging-handoff.md).

## Overview

con is a Cargo workspace with platform terminal backends:

- macOS embeds full Ghostty with Metal rendering.
- Windows uses ConPTY plus `libghostty-vt` and a D3D11/DirectWrite renderer.
- Linux uses a Unix PTY plus `libghostty-vt` and the preview GPUI paint path.

The build no longer includes the old `vte` and `portable-pty` pipeline.

## Build commands

```bash
cargo build
cargo build --release
cargo run -p con
cargo test --workspace
```

## Workspace shape

```toml
[workspace]
members = [
    "crates/con-app",
    "crates/con-core",
    "crates/con-paths",
    "crates/con-terminal",
    "crates/con-ghostty",
    "crates/con-agent",
    "crates/con-cli",
]
```

## Key crates

| Crate | Purpose |
|-------|---------|
| `con` | GPUI app shell, tabs, splits, settings, agent panel |
| `con-core` | harness, config, session persistence |
| `con-paths` | shared platform-safe per-user app directories |
| `con-ghostty` | Rust wrapper around libghostty C API |
| `con-terminal` | terminal theme data and Ghostty palette translation helpers |
| `con-agent` | built-in AI harness and tools |

## Key dependencies

| Dependency | Purpose |
|------------|---------|
| `gpui` | native GPU UI framework (Zed's GPUI via exact-pinned `gpui-pre` crates.io snapshots) |
| `gpui-component` | reusable UI controls (Longbridge, crates.io, pinned together with `gpui-pre`) |
| `rig-core` | multi-provider agent runtime (crates.io) |
| `tokio` | async runtime for agent work |
| `crossbeam-channel` | UI and harness event routing |
| `reqwest` | live model list fetch from models.dev |

## Dependency sourcing

`con` does not build against live sources in `3pp/`.

- `3pp/` is read-only reference material only.
- Cargo dependencies resolve from crates.io or explicit git sources in the workspace manifest.
- GPUI follows gpui-component's releases: an unreleased Zed fix cannot be picked
  up by moving a git rev, and `[patch]` from Zed's repo does not apply because the
  crate there is named `gpui`, not `gpui-pre`. Wait for a `gpui-pre` snapshot with a
  matching gpui-component release, or move both back to git sources together.
- The maintainer approved a temporary exception for native macOS presentation
  on 2026-10-10. Three `gpui-pre` 0.3.7 packages resolve together from one public,
  immutable Git revision; the copied registry manifests preserve the existing
  component/type graph. This is not a general permission to fork dependencies.
  [Provenance and removal criteria](../design/native-terminal-presentation.md)
  track the upstream submission and compatible official replacement.
- Ghostty source is fetched by `con-ghostty/build.rs` when needed, unless an override source directory is provided for local development.
  The [Ghostty 1.4 integration record](ghostty-1.4-upgrade.md) documents the
  shared immutable pin, Zig 0.17 toolchain, OSC bridge, security applicability
  and native verification/rollback requirements.

## Platform boundary

The `con` UI binary builds on macOS, Windows, and Linux. Platform-specific
runtime code is `cfg`-gated:

- macOS uses the embedded libghostty surface.
- Windows uses the `con-app.exe` binary name because `CON` is a reserved DOS
  device name.
- Linux keeps the normal `con` binary name.

## Per-user app paths

Use `con-paths` for all user config, data, auth, theme, and skill storage paths.
Windows cannot safely use a bare `con` path segment, so `con-paths` maps the app
directory to `con-terminal` on Windows while preserving `con` on macOS and Linux.

## Ghostty build boundary

`con-ghostty` is intentionally thin:

- FFI bindings live in `ffi.rs`
- surface/app lifecycle lives in `terminal.rs`
- product logic stays out of the wrapper

## Ghostty resources

Ghostty's runtime is not just the static library. The child shell environment also depends on the bundled Ghostty resources payload, especially:

- `terminfo/xterm-ghostty`
- shell integration scripts
- supporting share files under `Resources/ghostty`

Con now handles this in two places:

- `cargo run -p con` debug runs: `con-ghostty` seeds `GHOSTTY_RESOURCES_DIR` from the built Ghostty `zig-out/share/ghostty` directory when that directory exists locally.
- macOS app bundles: `scripts/macos/build-app.sh` copies Ghostty's built `share/ghostty` tree into `Contents/Resources/ghostty`.

Without that payload, Ghostty falls back to `TERM=xterm-256color` and disables parts of shell integration. That changes the behavior child processes see and can invalidate product comparisons against standalone Ghostty.

If we need stronger pane observability in the future, the preferred path is to upstream or expose new libghostty C API surface area instead of growing another terminal runtime in this repo.
