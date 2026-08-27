---
name: release
description: Run and verify Con beta, dev, and stable releases without publishing incomplete artifacts.
---

# Con Release

Use this skill when preparing a release, pushing a release tag, checking a
release workflow, or recovering a failed publication. The release workflow is
the source of truth; do not treat a green build on one platform as a release.

## Release contract

- Release from `main` only.
- Prepare `CHANGELOG.md` before tagging. The top section must be the next
  unreleased beta and every PR-derived item must credit its GitHub author.
- Push exactly one immutable tag. Use `v0.1.0-beta.N` for beta, `v0.1.0-dev.N`
  for dev, and the project stable tag when stable is explicitly approved.
- A tag push starts the macOS, Linux, Windows, and Flatpak workflows. Platform
  jobs create or update one GitHub draft release and upload their own assets.
- A release is public only after all required platform assets and release-gate
  checks pass. Installers and updaters must never consume a draft.
- Never delete and recreate a tag to repair a release. Fix the workflow or
  rerun the affected workflow against the same commit.

## Before tagging

Run these checks from a clean checkout:

```bash
git fetch origin main --tags
git switch main
git pull --ff-only origin main
git status --short
git tag --sort=-v:refname | rg '^v[0-9]+\\.[0-9]+\\.[0-9]+-(beta|dev)\\.' | head
gh release list --limit 5
```

Confirm the next version is greater than the latest released version, the
changelog heading matches it, and `git status --short` is empty. Run the
release-relevant test suite before tagging; platform packaging tests belong in
the release workflows and must not be replaced by a local cross-build.

## Publish

Create an annotated tag at the verified `main` commit and push only that tag:

```bash
git tag -a v0.1.0-beta.N origin/main -m "Release v0.1.0-beta.N"
git push origin refs/tags/v0.1.0-beta.N
```

Track the four tag-triggered workflows:

```bash
gh run list --limit 20 --json workflowName,status,conclusion,headBranch,headSha,url \\
  --jq '.[] | select(.headBranch == "v0.1.0-beta.N")'
```

Do not publish a draft manually while a required platform is still running or
failed. The finalizer checks the newest matching run for each required
platform, then validates assets, checksums, appcasts, and public installer
scripts before promoting the draft.

## Verify the public result

After the finalizer succeeds, verify the public contract:

```bash
gh release view v0.1.0-beta.N --json isDraft,isPrerelease,publishedAt,assets,url
```

For a beta, expect `isDraft: false` and the repository's beta-channel release
semantics. Confirm the expected macOS arm64/x86_64 DMGs and ZIPs, Linux
tarball, Windows ZIP, and checksum files are present. Check the beta appcasts
and the public installer endpoints if they are part of the release:

```bash
curl -fsSL https://con-releases.nowledge.co/install.sh >/dev/null
curl -fsSL https://con-releases.nowledge.co/install.ps1 >/dev/null
```

## Recovery

- If a platform workflow is cancelled, rerun that exact workflow run; do not
  push a second tag.
- If all platform workflows succeeded but the release remains draft, inspect
  the latest `Finalize release` run. Rerun only the finalizer after confirming
  the release gate inputs are complete.
- If an asset is missing, rerun the platform that owns it and verify its
  checksum before promotion.
- If the release is already public, later finalizer runs are harmless and must
  not change its prerelease/channel semantics.
- Record a workflow or release incident in `postmortem/` when the failure
  exposes a new invariant or requires manual recovery.

## Common mistakes

- Do not tag from a stale local `main`.
- Do not use `--force` on release tags.
- Do not mark beta releases as prereleases unless the channel policy changes;
  current installers use the public latest beta semantics.
- Do not assume a draft URL or an uploaded asset means the release is public.
- Do not use a repository-wide GitHub Actions concurrency group for finalizer
  events; each source completion must remain observable.
