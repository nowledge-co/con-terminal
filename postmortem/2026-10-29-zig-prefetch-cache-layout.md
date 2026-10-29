# Zig 0.17 prefetch cache layout

## What happened

The Ghostty 1.4 upgrade passed local Apple Silicon builds and Linux native
linking, but Intel macOS CI rejected four dependency content hashes before
linking. The job enabled con's proactive curl/git package prefetch fallback.

## Root cause

Zig 0.17 stores recompressed package archives in the global cache and unpacked
packages in a project-local directory. Standalone `zig fetch <archive>` without
local package storage computes the correct hash but recompresses the temporary
directory containing the archive's outer directory. A later build unpacks that
extra directory and computes a different hash.

The issue is not Intel-specific. An isolated Apple Silicon fixture reproduced
the identical pixels-package hash mismatch seen on Intel CI. Existing local
package trees and correctly populated caches hid the faulty prefetch path.

## Fix

Use `--save-exact` in a disposable manifest and pass `--pkg-dir` using the same
project-local directory as the build, honoring `ZIG_LOCAL_PKG_DIR`. Zig 0.17
only activates local storage in save mode; specifying the directory alone is
insufficient. Zig then moves the verified package root before recompression.
Traverse transitive manifests in that local directory. Retain the existing
bounded curl/git fallback and all upstream manifests and content hashes.

Wire the build-script tests into Cargo's integration tests. The new offline
regression creates an archive with an outer directory, prefetches it through
the real helper, then builds with a separate empty package directory. That
forces verification of the cached archive rather than reuse of an unpacked
tree. Its dependency URL is deliberately unavailable, so an unexpected network
fallback cannot conceal a broken cache round trip.

## Lessons

Test both cold-cache provisioning and cached-archive reuse when upgrading a
build tool. A successful fetch and its printed hash do not establish that the
produced cache is consumable. Reproduce cache layout defects independently of
runner architecture before treating them as CPU or ABI failures.
