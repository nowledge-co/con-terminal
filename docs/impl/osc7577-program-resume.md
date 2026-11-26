# OSC 7577 program resume

Tracked in [#573](https://github.com/nowledge-co/con-terminal/issues/573).
The wire format is [Program Resume Protocol revision 0.5](https://www.superlogical.com/rex/docs/build/program-resume).

## Authority and lifetime

Use a fresh cryptographically random token for each live PTY incarnation.
Supply it only to the initial child environment. Never log, persist, forward
or expose it through con-cli. The token authenticates the report bytes and
reduces the risk from displayed text. A child with the token can still
request a command with more privileges than it currently has.

Authenticate the complete report before decoding, enforce the timestamp
window and reject stale reports from the same owner. Verify that the reported
PID is a local process in the PTY and record its creation identity to handle
PID reuse. Only a verified owner's report may replace or clear its record.
The receiver must distinguish an ignored report from an authenticated reject.
Use one launch candidate per surface and retain it on application shutdown.
A deliberate surface, pane, tab, or window close queues removal on the
session writer. Moving a live surface keeps its record with that surface,
even when its layout ID changes. A confirmed owner exit also retires the
candidate. Terminal display resets do not clear it.

Use private session storage tied to the original local account. A generic
record is distinct from a native Codex or Claude conversation reference.
Show the escaped arguments and working directory in a resume review.
Require approval of the exact candidate before the first launch. Run the
native executable with literal argv, a validated directory and the environment
of the visible interactive shell. Do not evaluate program-provided arguments
as shell code or give a remote surface local process authority.

## Storage and consent

Session snapshots reference private records by UUID. Keep the proposed command
and directory in a separate per-account store. Require private ownership and
access modes on Unix and protect records with current-user DPAPI on Windows.
Exclude both the non-authoritative reference and the sensitive payload from
workspace profiles. A restored terminal still starts its regular interactive
shell. The user's launch action must be tied to the exact reviewed arguments,
consume the record once, and leave native agent recovery independent.

Bind approval to the exact reviewed value and the new PTY's context ID.
Pass only an opaque, single-use, short-lived approval reference to the
companion binary from the visible shell prompt. The companion
requires the same terminal context and consumes the ticket before launch.
Resolve a bare executable using that shell's environment.
On Windows use a native executable and preserve argument boundaries. On Unix
preserve argv[0]. Never interpolate path or argument text into the shell.

## Ingress and launch

Ghostty reports complete BEL and ST frames. The shared ingress queue
validates the framing and authenticates bounded payloads. It never waits
for an operating-system lookup on the parser thread. On macOS, con matches
the reported PID’s account, controlling PTY and foreground group before
accepting a proposal. Linux checks local PTY ancestry, effective account
and foreground group. Windows checks live ConPTY ancestry and the account
SID. Clear and reject affect only the matching previously accepted owner.
Elevated terminals and the Flatpak host bridge do not expose the
capability.

When a supported program saves a proposal, quit and reopen con, select the
same terminal, and open **Review Program Resume…** from the command palette.
Review the executable, working directory and argument boundaries. If con
can prove the local shell prompt is ready, **Resume** submits the approved
helper. Otherwise **Copy Command** gives you a one-time invocation to paste
at that terminal’s local shell prompt and run within one minute. Use
PowerShell or Command Prompt on Windows and a shell with `command` on Linux.
The copied text contains the helper path and ticket ID. The helper
rechecks the reviewed command and consumes the candidate on launch.
An expired or unused ticket leaves the proposal available for another review.

## Ghostty dependency

The public [pinned Ghostty commit](https://github.com/wey-gu/ghostty/commit/6e4acbf8edf9a66fe4e48680593a18ed14ba8e7f)
extends the OSC 7501 bridge with validated unknown OSC framing and an
additive embedded callback registration function. The existing runtime
configuration layout and public VT declarations are unchanged. The
parser waits for the backslash after ESC and withholds a frame if VT
normalization discarded a control byte.

[Ghostty discussion #12477](https://github.com/ghostty-org/ghostty/discussions/12477)
covers upstream embedder dispatch. [#444](https://github.com/nowledge-co/con-terminal/issues/444)
tracks returning to an official pin. Before removing the fork, verify that
the official revision forwards OSC 7501 and 7577 through full embedding and
delivers only complete, unmodified frames through both APIs. Test both
terminators, split writes, malformed frames and callback lifetime across
surface teardown on the affected platforms.
