# Control Socket Parent Permission Regression

## What Happened

Con could start normally on macOS without exposing its local control socket.
The UI remained usable, but `con-cli`, second-instance forwarding, and external
automation could not connect.

The failure appeared after Flatpak support added private permissions for its
runtime socket directory. The same preparation path also handles the macOS
default at `/tmp/con.sock`.

## Root Cause

Socket preparation unconditionally applied mode `0700` to the socket's parent.
For `/tmp/con.sock`, that attempted to change the root-owned `/tmp` directory.
A normal macOS process receives `EPERM`, so preparation returned before the
Unix socket could bind.

The security boundary was modeled at the wrong ownership level. Con owns its
socket and directories it creates, but it does not own every directory in
which a user chooses to place that socket.

## Fix Applied

- Create missing parent directories with mode `0700` through Unix
  `DirBuilderExt`, so private permissions apply at creation time.
- Leave every existing parent directory unchanged.
- Continue setting the bound socket itself to mode `0600`.
- Add regression tests for both an existing shared parent and a newly created
  private parent.
- Preserve path context in directory creation errors.

The implementation is Unix-only. Windows continues to use Named Pipes and is
not affected by filesystem socket permissions.

## What We Learned

Hardening must follow ownership. Changing permissions on a caller- or
system-owned directory is both unsafe and unreliable, even when the intended
permission is more restrictive.

Creation-time permissions are preferable to creating and then calling
`chmod`: they avoid both ownership races and a temporary overly permissive
state. Tests for security-sensitive filesystem setup must cover resources that
already exist as well as resources the application creates.
