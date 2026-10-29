# Embedded Ghostty security fixes and host paste integration

## What happened

Ghostty 1.4.0 disclosed several terminal and application security fixes after
con's previous development-source pin. The previous pin lacked the fix for
cursor style/hyperlink references when clearing history prunes the cursor page.
This was found during an upgrade audit, not from a reported con production
exploit.

The audit also found that con's macOS GPUI clipboard/drop entry point used the
same text API as IME commits. Ghostty's new protected paste API would not take
effect at that call site through a dependency bump alone.

## Root cause

Cursor style and hyperlink IDs belong to their allocating terminal page. The
old clear path could move the cursor pin to another page after freeing the old
page, then reload stale page-local IDs. Increasing history changes the trigger
conditions but does not establish a safe minimum.

con embeds Ghostty's core rather than its Swift application. Upstream app-level
clipboard or URL policy changes cannot be assumed to change con's GPUI host.
The macOS text API deliberately bypasses paste protection for composed input.

## Fix and evidence

Use the official 1.4.0 baseline, retaining only the existing scoped public OSC
7501 bridge and its Zig 0.17 port. Two upstream cursor-page tests crash on the
old pin (style and hyperlink reference integrity); the candidate passes the
81-test focused clear suite. Add a portable con feed/snapshot regression across
recycled pages and extend paste tests to CR and CRLF, not only LF.

Route macOS user clipboard/drop text through `ghostty_surface_paste_text`.
Keep IME commits and explicit control-plane writes on their existing paths.
The existing clipboard confirmation callback continues to fail closed; a denied
paste does not clear the completed-program notification.

The [integration record](../docs/impl/ghostty-1.4-upgrade.md) separates core,
packaged shell integration, and host-app advisory applicability. It records the
source/toolchain contract and the platform validation requirements.

## Lessons

Audit changes against con's actual development pin, not the previous numbered
Ghostty application release. Keep library and host security boundaries separate.
Dependency migrations must preserve the OSC bridge's IO-thread lifetime contract
and test the real ABI and native backends. A source-only bump, larger history
limit, or successful type check is not sufficient evidence.
