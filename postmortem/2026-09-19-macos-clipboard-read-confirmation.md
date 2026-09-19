# macOS clipboard reads bypassed confirmation

## What happened

A terminal program could request the macOS clipboard with OSC 52 and receive
its contents without user approval. This was identified by source inspection;
no exploitation was observed.

## Root cause

Con inherited Ghostty's `clipboard-read = ask`, but
`confirm_read_clipboard_callback` completed non-Kitty requests with
`confirmed: true`. The callback requests permission; it does not report an
existing user approval. `remember: false` prevents persisting a grant, not the
current disclosure.

## Fix applied

Con generates `clipboard-read = deny` to reject application reads before
accessing NSPasteboard. The confirmation callback denies requests using
Ghostty's consuming denial API, without retaining or dereferencing the borrowed
confirmation payload. No permission UI or new setting is introduced.

Ordinary user paste is independent of the application-read policy. Native
pastes that require Ghostty's unsafe-paste confirmation are now rejected rather
than silently approved. Con's host-side paste path remains unchanged.

## What we learned

Permission callbacks must fail closed until the host can obtain actual user
approval. Test read policy independently of clipboard-write settings; allowing
TUI copies must not grant TUI reads. Configuration prevents unnecessary clipboard
access, while callback denial handles requests that still require approval.
