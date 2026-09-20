# ConPTY resize and VT scrollback

## What happened
An upstream compatibility audit identified a resize policy mismatch in Con's Windows backend. This was not a locally reproduced native ConPTY failure.

## Root cause
ConPTY maintains its own active screen but cannot pull Con's VT scrollback back into it. Ghostty's default resize policy can reveal that history during row growth or column reflow, making the two active screens disagree. Upstream documents this contract in [Ghostty #14296](https://github.com/ghostty-org/ghostty/pull/14296).

## Fix applied
Pin the first merged upstream revision exposing `RESIZE_PULL_SCROLLBACK` and disable it when constructing the Windows VT. Leave Unix defaults and macOS embedding policy unchanged. The option survives RIS, so resize and reset paths need no extra work.

## What we learned
An ABI-compatible dependency can still require a host-specific policy. Check observable active rows, not just whether resize succeeds. The regression test distinguishes row growth and width reflow, before and after RIS; native ConPTY acceptance remains necessary alongside portable VT tests.
