# SSH Integration

Con embeds Ghostty's shell integration on macOS. The integration runs before
an SSH connection and invokes the bundle-local `ghostty` compatibility entry
point. In a release app this entry point resolves to `con-cli`, so Con owns the
helper protocol rather than depending on a separately installed Ghostty.

## Protocol boundary

Ghostty's current shell integration invokes:

```text
ghostty +ssh [Ghostty options] -- [ssh arguments]
```

`con-cli` accepts the same wrapper boundary:

```text
con-cli +ssh [options] -- [ssh arguments]
```

Options before `--` belong to Con. Everything after `--` is passed to the
selected SSH executable without a shell or argument rewriting. For backward
compatibility, the wrapper also follows Ghostty's convention that the first
non-`--` argument begins the SSH argument list.

Supported wrapper options are:

- `--forward-env[=bool]` controls `TERM` and terminal identification forwarding.
- `--terminfo[=bool]` enables remote `xterm-ghostty` installation.
- `--cache[=bool]` controls Con's local terminfo-install cache.
- `--ssh=<path>` selects the SSH executable instead of resolving `ssh` from `PATH`.
- `--verbose` prints setup and execution diagnostics to stderr.

## Connection flow

1. `con-cli +ssh` resolves the destination with `ssh -G`. Failure to resolve
   it does not prevent the normal SSH command from running.
2. If terminfo setup is enabled and the destination is cached, Con selects
   `TERM=xterm-ghostty` without another remote setup connection.
3. Otherwise Con asks the local `infocmp` for the bundled
   `xterm-ghostty` entry and sends it to the remote `tic` through a short-lived
   SSH control connection.
4. If any local or remote setup step fails, Con falls back to
   `TERM=xterm-256color` and continues with the user's SSH command.
5. A successful setup and successful SSH session add the destination to
   Con's cache. The cache contains host metadata only; it is not a shared
   Ghostty cache format.

Environment forwarding uses OpenSSH options rather than a shell command:

- `SetEnv=TERM=...`
- `SendEnv=COLORTERM`
- `SendEnv=TERM_PROGRAM`
- `SendEnv=TERM_PROGRAM_VERSION`

The remote SSH server must accept forwarded variables for them to arrive.
Terminfo installation does not depend on those variables.

## Compatibility and release rules

The `+ssh` helper is a compatibility boundary for shell integration, not a
terminal-rendering feature. It must remain isolated from the Windows and Linux
terminal backends. Those platforms continue to build and test `con-cli`, but
their terminal sessions do not enable the macOS bundle integration path.

When bumping Ghostty, inspect the generated shell integration before enabling
`ssh-terminfo`. A revision that invokes `+ssh` requires a helper supporting
`+ssh`; a revision that invokes only `+ssh-cache` requires the cache command.
Release verification must check both the helper entry point and the relevant
CLI action before publishing an appcast or archive.

For diagnostics, reproduce with:

```bash
con-cli +ssh --verbose -- user@example.com
```

If the wrapper is not reached, inspect the active shell function and binary:

```bash
type -a ghostty
print -r -- ${GHOSTTY_BIN_DIR:-<unset>}
functions ssh
```
