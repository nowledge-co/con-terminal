# Release workflows cancelled before building

## What happened

The first beta.85 tag triggered Linux, macOS, Windows, and Flatpak workflows,
but only the first workflow acquired the repository's `con-gh-pages-publish`
concurrency group. The remaining workflows were cancelled before GitHub created
their jobs because a concurrency group allows only one running and one pending
run; later pending runs replace earlier ones.

## Root cause

The concurrency group was declared at workflow scope in all three platform
release workflows. That serialized platform builds, even though only the
appcast update jobs write the shared `gh-pages` branch.

## Fix applied

The platform workflow-level locks were removed. The lock now applies only to
each platform's `update-appcast` job. Platform builds and GitHub Release asset
publishes can run concurrently; appcast writers remain serialized to protect
the shared branch.

## What we learned

Concurrency should protect the smallest shared mutation, not the whole build
pipeline. Release preflight must verify that every tag-triggered platform
workflow has created jobs before treating a tag as publishable.
