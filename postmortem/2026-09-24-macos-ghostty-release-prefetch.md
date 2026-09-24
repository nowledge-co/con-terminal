# macOS release builds retried after Zig Git package fetch failures

## What happened

The macOS Intel release jobs for beta.108, beta.109, and beta.110 succeeded
only after an initial `zig build` failure, a broad Zig package prefetch, and a
second build. The beta.109 Apple Silicon job showed the same pattern. Their
logs also carried `ranlib` warnings about an empty
`libcon_ghostty_ffi_abi.a`.

## Root cause

The original build script checked only the first Zig process's exit status.
Cargo hid its stderr when the build script eventually succeeded, so release
logs showed the fallback but not the reason for it. A targeted Intel CI build
with bounded failure diagnostics identified a nested `vaxis` dependency:

```text
git+https://github.com/zigimg/zigimg#d695acd97c02e57bb151e8f659d1280f5cd6ca70
error: unable to discover remote git server capabilities: HttpConnectionClosing
```

Zig's direct Git fetch failed in CI. The existing fallback used `git fetch`
and then local `zig fetch`, which populated the same package cache and allowed
the retry to succeed. A previous local-network incident with Zig's HTTP
fetcher is documented in `2026-05-12-con-ghostty-zig-package-prefetch.md`;
this CI incident has a separately observed Git transport failure.

The ABI C translation unit contains only compile-time assertions, not exported
symbols. Passing it to `cc::Build::compile` produced a symbol-free archive and
the unrelated `ranlib` warnings.

## Fix

- Compile the ABI assertions as intermediate object files without archiving
  them. A failed assertion still fails the build.
- Include a bounded excerpt of stderr when the first macOS Zig build fails,
  while retaining the fallback for ordinary developer builds.
- Prefetch packages before macOS app packaging, using the already validated
  curl/git and local `zig fetch` path. Normal development builds keep the
  cheaper build-first behavior and retry only when needed.
- Exercise the release-equivalent Intel Cargo build on pull requests that
  change the Ghostty build path.

## What we learned

An eventually successful build can conceal a deterministic first failure if
the build script handles it internally. Release logs need the original error,
and the release path should use a known-good dependency transport before
starting an expensive native compile rather than treating that failure as a
routine warm-up step.
