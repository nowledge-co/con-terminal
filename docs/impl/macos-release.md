# macOS Release Pipeline

This repo now ships a first-pass macOS release pipeline for `con` that:

- builds a native `.app` bundle without depending on `cargo-bundle`
- embeds `con-cli` in the app bundle so installers and Homebrew can expose the
  control-plane CLI without a separate source build
- signs with a Developer ID Application certificate
- notarizes with `notarytool`
- staples the `.app`
- packages and signs a `.dmg`
- notarizes and staples the `.dmg`
- publishes per-architecture artifacts to GitHub Releases

## What Changed

Scripts live in [`scripts/macos`](../../scripts/macos):

- `build-app.sh` builds `con` and assembles `con.app`
- `build-app.sh` also builds `con-cli` and places it at
  `Contents/MacOS/con-cli`
- `import-certificate.sh` imports the Developer ID cert into a temporary keychain for CI
- `release.sh` runs build, sign, notarize, staple, package, and checksum generation
- `verify.sh` runs `codesign`, `spctl`, and stapler validation checks
- `verify.sh` fails the release if the bundled `con-cli` executable is missing

`con-cli` exposure is deliberately layered:

- Homebrew casks install a `con-cli` binary shim from the app bundle.
- `install.sh` links `~/.local/bin/con-cli` to the bundled CLI after copying
  the app into `/Applications`.
- Sparkle updates replace only the app bundle, so the app also performs a
  conservative launch-time self-heal: it creates or repairs
  `~/.local/bin/con-cli` only when the path is missing or already points to a
  Con `.app` bundle. It never overwrites a real user-managed binary or an
  unrelated symlink.

GitHub Actions release workflow:

- [`.github/workflows/release-macos.yml`](../../.github/workflows/release-macos.yml)

## Identity Model

The website host and the bundle identifier should not use the same order.

- website: `con-releases.nowledge.co`
- macOS bundle id base: `co.nowledge.con`

That follows reverse-DNS convention and is the long-term correct identifier layout.

Channel behavior:

- `stable`: bundle id `co.nowledge.con`, app name `con`
- `beta`: bundle id `co.nowledge.con.beta`, app name `con Beta`

That split is deliberate. If beta and stable ever need to coexist on one machine, they cannot share a bundle identifier.

## GitHub Scope Split

Use GitHub scopes this way:

- organization secrets: Apple team credentials reused by multiple apps
- repository variables: app identity and packaging defaults

This is the right long-term split because your Apple trust chain belongs to the organization, while bundle ids and app names belong to individual products.

GitHub documents organization-level secret sharing and selected-repository access here:

- https://docs.github.com/en/actions/administering-github-actions/sharing-workflows-secrets-and-runners-with-your-organization
- https://docs.github.com/en/actions/reference/security/secrets

## Required GitHub Secrets

Signing:

- `APPLE_CERTIFICATE_P12_BASE64`
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_KEYCHAIN_PASSWORD`
- `APPLE_SIGNING_IDENTITY`

Notarization, preferred:

- `APPLE_NOTARY_KEY_ID`
- `APPLE_NOTARY_ISSUER_ID`
- `APPLE_NOTARY_API_KEY_BASE64`

These are team-level credentials and can be organization secrets, restricted to the repositories that build signed apps.

Important:

- `APPLE_NOTARY_API_KEY_BASE64` must be the base64-encoded contents of the `.p8` file
- for local testing, you can skip base64 entirely and use `APPLE_NOTARY_KEY_PATH=/absolute/path/to/AuthKey_XXXXXX.p8`
- the workflow uses `APPLE_NOTARY_API_KEY_BASE64` because GitHub secrets cannot safely point to local filesystem paths

Local fallback is also supported in the scripts, but should not be your primary CI path:

- `APPLE_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`
- `APPLE_TEAM_ID`

Apple's App Store Connect API key docs are here:

- https://developer.apple.com/help/app-store-connect/get-started/app-store-connect-api
- https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api

Apple explicitly notes that individual App Store Connect API keys cannot be used with `notaryTool`; use a team key.

## Repository Variables

This repo does not need any GitHub repository variables for macOS releases.

The workflow builds correctly from the defaults baked into the scripts:

- `MACOS_APP_NAME=con`
- `MACOS_BUNDLE_ID_BASE=co.nowledge.con`
- `CON_ICON_SOURCE=assets/app-icons/con-raccoon-tail-A6.png`
- `MACOS_MINIMUM_SYSTEM_VERSION=10.15.7`

If you reuse these scripts in another app repo, override those values there instead of editing the workflow.

## Release Tags

Stable release:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Beta release:

```bash
git tag v0.2.0-beta.1
git push origin v0.2.0-beta.1
```

Dev smoke release:

```bash
git tag v0.2.0-dev.1
git push origin v0.2.0-dev.1
```

The workflow maps `*-beta.*` tags to the beta channel and `*-dev.*` tags to an
internal dev channel. Dev releases are marked as GitHub prereleases and do not
embed or update public appcasts, and do not update Homebrew casks. Beta tags
are not marked as prereleases while Con is still in the all-beta era, so fresh
installs resolve to the newest public beta after the finalizer promotes the
draft.

The release finalizer always syncs `install.sh` and `install.ps1` from the
tagged commit to `gh-pages` before it promotes the draft. This keeps internal dev
smoke tags useful for installer E2E without moving beta/stable appcasts or
Homebrew casks.

## Reusing Existing Apple Credentials

If your existing `nowledge-identities.p12` contains the same Apple team's `Developer ID Application` identity, it can be reused for `con`.

If your existing App Store Connect team API key is:

- Key ID: `KG4RA8F5A6`
- Issuer ID: `df090cec-9b81-4642-a0f0-5063ae39fb87`

then it can also be reused for notarizing `con`, provided it is a team key and still active.

What is reusable across apps:

- Developer ID Application certificate
- App Store Connect team API key
- temporary CI keychain password convention

What is app-specific and should stay per-repo:

- bundle identifier
- app name
- icon path
- updater feed URL
- release-channel defaults

## Local Dry Run

Ad-hoc signed local build:

```bash
CON_ALLOW_ADHOC_SIGNING=1 \
CON_SKIP_NOTARIZATION=1 \
./scripts/macos/release.sh
```

Signed local build with notarization:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Company (TEAMID)"
export APPLE_NOTARY_KEY_ID="XXXXXXXXXX"
export APPLE_NOTARY_ISSUER_ID="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
export APPLE_NOTARY_KEY_PATH="/absolute/path/to/AuthKey_XXXXXXXXXX.p8"

./scripts/macos/release.sh
```

Local beta updater test against the published beta appcast:

```bash
export CON_SPARKLE_PUBLIC_ED_KEY="...public key from your release setup..."

./scripts/macos/test-update-beta.sh
```

This intentionally builds a bundled beta app with a very low `CFBundleVersion`
so Sparkle should see any real published beta release as newer. `cargo run`
cannot test updates because it is not running inside a bundled app and does not
embed `Sparkle.framework`.

After the update installs, verify the result from either of these product surfaces:

- `con` → `About con`
- Settings → Updates

Both should show the updated marketing version, build number, and release channel.

## CI Output

The workflow currently builds native artifacts on:

- `macos-15` for Apple Silicon / `arm64`
- `macos-15-intel` for Intel / `x86_64`

That matches the current `con-ghostty` build behavior: it builds Ghostty for the host architecture in `build.rs`. This pipeline avoids pretending we have a universal build when we do not.

## Auto-Update Architecture

### Overview

Auto-update uses Sparkle on macOS with GitHub Releases as the artifact host and GitHub Pages as the appcast host.

```
Tag push → CI builds → signs → notarizes → uploads to GitHub Release
                                                    ↓
                                          signs artifact with Ed25519
                                          generates/updates appcast XML
                                          pushes appcast to gh-pages
                                                    ↓
                                          App (Sparkle) polls feed URL
                                          compares build number
                                          downloads DMG
                                          verifies Ed25519 signature
                                          installs + relaunches
```

### Feed URL Scheme

Each build targets one channel and one architecture.  The feed URL is derived deterministically and baked into `Info.plist` at build time:

```
https://con-releases.nowledge.co/appcast/{channel}-macos-{arch}.xml
```

Examples:

- `https://con-releases.nowledge.co/appcast/stable-macos-arm64.xml`
- `https://con-releases.nowledge.co/appcast/beta-macos-arm64.xml`
- `https://con-releases.nowledge.co/appcast/stable-macos-x86_64.xml`

This is stable across releases and extensible to Linux when needed.

### Sparkle Integration

Sparkle is loaded dynamically from `Contents/Frameworks/Sparkle.framework` at app launch.  If the framework is absent (e.g. `cargo run` dev builds), auto-update silently disables.

The Rust FFI bridge (`crates/con-app/src/updater.rs`) uses `objc` crate to:

1. Load `Sparkle.framework` from the app bundle
2. Verify `SUFeedURL` is set in Info.plist
3. Create `SPUStandardUpdaterController` (starts automatic checking)
4. Expose `check_for_updates()` for the manual “Check for Updates” menu action

### Manual Smoke Checklist

Before shipping a beta or stable build, verify these flows from the bundled app:

1. Open the app from Finder and confirm the terminal renders normally.
2. Open `About con` and confirm the app icon, version, build, and channel are visible.
3. Run `con-cli identify` from a new shell after installing by Homebrew or
   `install.sh`; it should connect to the running app and print app identity.
4. For a manual DMG/Sparkle-updated app, confirm launching the app creates or
   repairs `~/.local/bin/con-cli` when that path is missing.
5. Open Settings → Updates and confirm the same version/build information is shown there.
6. Run `Check for Updates…` against the intended channel and confirm Sparkle presents the expected UI.
7. After installation, reopen `About con` and confirm the build number changed.

### Automated Release Gates

Release safety is enforced in two layers:

1. Platform release jobs verify artifact shape before upload. macOS checks the
   app bundle, ZIP, DMG, checksums, and bundled `con-cli`; Linux checks the
   tarball layout, checksum, and `con-cli --help`; Windows expands the ZIP,
   verifies `con-app.exe` and `con-cli.exe`, runs `con-cli.exe --help`, and
   checks `SHA256SUMS-windows.txt`.
2. `release-finalize.yml` refuses to promote the draft release unless the
   expected GitHub Release assets exist, stable/beta appcasts point at the
   same tag's artifacts, and the gh-pages installer scripts expose `con-cli`
   / `con-cli.exe`.
   Dev tags skip appcast/Homebrew publication entirely and are only checked for
   artifact and installer-script shape.

This means a failed or incomplete platform build can upload nothing more than
a private draft. Fresh installs keep resolving the previous public release, and
older clients keep polling the previous valid appcast entry.

### Release Channel Runtime

`con-core/src/release_channel.rs` provides a cross-platform `ReleaseChannel` enum:

- `Dev` — local or internal smoke builds, never polls for updates
- `Beta` — pre-release, polls `beta-macos-{arch}.xml`
- `Stable` — GA builds, polls `stable-macos-{arch}.xml`

On macOS, the channel is read from `ConReleaseChannel` in the bundle's Info.plist.
On other platforms, it falls back to the `CON_RELEASE_CHANNEL` environment variable.

### Required Secrets for Auto-Update

In addition to the signing secrets, the updater needs:

- `SPARKLE_SIGNING_KEY` — Ed25519 private key (base64) for signing appcasts
- `SPARKLE_PUBLIC_ED_KEY` — Ed25519 public key (base64) baked into Info.plist

Generate a key pair:

```bash
./scripts/sparkle/keygen.sh
```

Store them:

- `SPARKLE_SIGNING_KEY` → GitHub org secret
- `SPARKLE_PUBLIC_ED_KEY` → GitHub org secret (passed as env var during build)

### Appcast Hosting

Appcasts are served from GitHub Pages:

- Branch: `gh-pages`
- Custom domain: `con-releases.nowledge.co` (CNAME record → `nowledge-co.github.io`)

Initialize the gh-pages branch:

```bash
./scripts/sparkle/init-gh-pages.sh
git push -u origin gh-pages
```

Then configure GitHub Pages in repo Settings → Pages → gh-pages branch.

### Scripts

| Script | Purpose |
|--------|---------|
| `scripts/sparkle/download.sh` | Download Sparkle framework + CLI tools |
| `scripts/sparkle/keygen.sh` | Generate Ed25519 key pair |
| `scripts/sparkle/sign-artifact.sh` | Sign a release artifact |
| `scripts/sparkle/update-appcast.sh` | Add/update entry in appcast XML |
| `scripts/sparkle/init-gh-pages.sh` | Initialize gh-pages branch |

### Build Number Policy

`CFBundleVersion` (Sparkle's version comparison key) uses `GITHUB_RUN_NUMBER` — a monotonically increasing integer scoped to the workflow.  This guarantees Sparkle always sees a strictly increasing build number regardless of marketing version or channel.

Local fallback: 0 (dev builds never poll, so the value only appears in Finder "Get Info").

### Distribution Format

**DMG is the primary distribution artifact.**  The zip file is produced for CI automation and programmatic consumption only.

macOS framework bundles (like Sparkle.framework) rely on symlinks (`Versions/Current -> B`).  Zip extraction via macOS Archive Utility dereferences these symlinks, producing a broken bundle that Gatekeeper rejects as "ambiguous (could be app or framework)."  The DMG preserves the full HFS+ structure including symlinks.

Sparkle's auto-updater downloads the DMG from GitHub Releases and handles installation — users never need to extract zip files.

### Future: Linux

The release channel enum and feed URL scheme are platform-agnostic.  On Linux, replace Sparkle with a lighter mechanism (e.g. checking the GitHub Releases API directly or a custom HTTP-based updater).  The appcast hosting infrastructure (GitHub Pages) can serve additional feed formats alongside the Sparkle XML.
