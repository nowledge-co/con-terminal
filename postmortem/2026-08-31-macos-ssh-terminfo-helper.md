# macOS SSH terminfo helper mismatch

## What happened

After upgrading Con, users could see an SSH startup error similar to:

```text
no such file or directory: /Applications/con Beta.app/Contents/MacOS/ghostty
```

The terminal connection itself could still start, but the error was visible
and the automatic terminfo setup path was not reliable.

## Root cause

Con embeds Ghostty's shell-integration scripts and enabled the
`ssh-terminfo` feature. Those scripts use Ghostty's `+ssh-cache` command by
invoking `$GHOSTTY_BIN_DIR/ghostty`. Con's app bundle intentionally ships
`con` and `con-cli`, not a Ghostty executable, so the helper path did not
exist.

The important boundary is that `ssh-terminfo` has two responsibilities:
installing the terminal definition on the remote host and remembering which
hosts succeeded. Con had the former through the embedded integration, but not
the latter through a compatible local command.

## Fix

The fix was delivered in three independently reviewable steps:

1. PR #321 stopped advertising `ssh-terminfo` until a compatible helper
   existed, removing the user-visible error without changing Windows or Linux.
2. PR #322 added `con-cli +ssh-cache`, with host checks, add/update, remove,
   clear, expiry filtering, atomic writes, lock recovery, and malformed-entry
   tolerance. The cache stores host metadata and timestamps only.
3. PR #323 added a macOS bundle-local `ghostty -> con-cli` symlink. This keeps
   the upstream shell scripts unchanged while routing their existing
   `+ssh-cache` calls to Con's implementation. The packaging change is
   macOS-only; Windows and Linux do not run `scripts/macos/build-app.sh`.

## What we learned

- Enabling an upstream feature is not sufficient when the feature invokes a
  companion executable that the host application does not ship.
- Compatibility entry points are safer than copying or rewriting third-party
  shell scripts, provided the adapter is explicit, local to the platform
  bundle, and covered by packaging checks.
- A broken optional optimization should fail closed at the feature boundary,
  not fail the user's SSH startup or the application's build.

## Follow-up

- Keep the `con-cli +ssh-cache` output and exit status stable because shell
  integration uses the host check as a predicate.
- Add a signed macOS release-artifact assertion that the compatibility entry
  point exists and resolves to `con-cli` before publishing a beta.
- Revisit the adapter if upstream Ghostty exposes a configurable cache helper;
  until then, do not re-enable `ssh-terminfo` through configuration alone.
