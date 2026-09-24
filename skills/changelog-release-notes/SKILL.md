---
name: changelog-release-notes
description: Maintain Con's CHANGELOG.md and release notes. Use when updating changelog entries, preparing a beta/dev release, reviewing PR release-note coverage, or ensuring contributor PR credit is present.
---

# Changelog Release Notes

Use this skill whenever `CHANGELOG.md`, PR release notes, or release summaries
are touched.

## Workflow

1. Verify the latest released beta before choosing a heading:
   - `git fetch --tags origin`
   - `git tag --sort=-v:refname | rg '^v[0-9]+\\.[0-9]+\\.[0-9]+-beta\\.' | head`
   - `gh release view <tag>` when GitHub release state matters
2. Put pending work under the next unreleased beta only. If the latest shipped
   beta is `v0.1.0-beta.63`, the top unreleased heading is
   `v0.1.0-beta.64`; do not create `beta.65` until `beta.64` is tagged and
   released.
3. Gather PR and author context for every entry:
   - `gh pr view <number> --json number,title,author,url,mergedAt`
   - for merged work, `git log <latest-tag>..HEAD --merges --oneline` helps
     find the included PRs
4. Every PR-derived changelog bullet must end with PR and author credit:
   - `_(PR [#149](https://github.com/nowledge-co/con-terminal/pull/149) by [@sundy-li](https://github.com/sundy-li))_`
5. Credit contributors exactly by GitHub login. Preserve external contributor
   credit even when follow-up commits refine the same feature.
6. Keep shipped historical sections stable. Only edit old sections to correct a
   factual mistake, missing credit, or broken link.

## Writing Rules

- Write for users first: describe the visible behavior or benefit, not the
  internal implementation unless it explains a risk or platform boundary.
- Use `Added`, `Changed`, and `Fixed` sections. Group bullets under concise
  audience/platform labels such as `macOS`, `Windows, Linux`, `Panes`, or
  `Developer Experience`.
- Do not mention "workspace restore" or other baseline continuity as novelty
  unless the user-facing behavior truly changed. Default expected behavior
  should read as polish or reliability, not marketing.
- Add release dates only when a version is actually tagged/released.
- After a release, check the latest shipped headings against `gh release list
  --json tagName,publishedAt`; use the release date in Asia/Shanghai, matching
  the existing changelog convention. Do not leave shipped headings undated.
- If a bullet combines multiple PRs, include all relevant PR/author credits.

## Validation

- Confirm the heading matches the latest shipped beta plus one.
- Confirm every new PR-derived bullet has `PR [#...]` and `by [@...]`.
- Run `python3 scripts/docs/validate-manifest.py` when docs routing may be
  affected.
- Run `git diff --check` before committing.
