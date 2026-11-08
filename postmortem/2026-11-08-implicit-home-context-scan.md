# AI request preparation enumerated private HOME folders

## What happened

Issue #533 reports macOS Desktop/Documents permission prompts on the first AI
question without configured API keys, with the shell cwd at HOME. The reported
beta.160 harness and the current shipped beta.172 harness have the same request
preparation code. The code path explains the reported access; no TCC permission
was granted or real user directory scanned during this investigation.

## Root cause

The harness unconditionally enriched context before constructing a provider
client. It read AGENTS.md, ran Git commands and, on Git failure, fell back to
`find . -maxdepth 3` in the observed cwd. HOME therefore became an implicit
recursive workspace. The 200-line prompt truncation happened after traversal.
`auto_context` existed as configuration but did not govern this collection.

A missing API key could stop the eventual HTTP client, but could not undo
filesystem access that had already happened. A permission prompt does not prove
an upload; equally, lack of upload does not make the unexpected access acceptable.

## Fix

Remove automatic filesystem enrichment altogether. Cwd and Git membership are
not consent. Automatic context is terminal-held state, and auto_context=false
omits it from the request and skips background pane queries. Keep prior user
messages/conversation history; do not misrepresent the switch as retroactive
redaction. Validate local provider requirements before context queries, while
preserving keyless and native OAuth transports.

Route local file reading, listing and search through visible approval by default.
Show the local workspace scope even for default-path requests. Preserve explicit
auto-approval preferences and canonical path checks. Do not suppress TCC prompts,
request Full Disk Access, or silently drop errors as a privacy fix.

## Verification

Provider/hook tests exercise context opt-out, exclusion of legacy filesystem
fields, configuration validation, keyless transports, denial and displayed scope.
A child-process request fixture isolates HOME/config/data/PATH, removes API keys,
and installs Git/find traps. HOME contains simulated Desktop/Documents and
AGENTS.md sentinels. It exercises missing/blank credentials, HOME, an ordinary
project, a remote cwd and disabled automatic context. Loopback-only HTTP captures
actual requests and verifies that filesystem sentinels are absent and terminal
context follows the switch; an automatic tmux capability must not be queried
before a missing-key failure or with context disabled. There are no paid model
calls, production configuration writes or macOS permission grants.

Final local checks: 1,226 tests passed across con-agent, con-core, the privacy
fixture and the app; one existing opt-in real-Codex test was ignored. The app
and CLI built successfully. Native Light/Dark loopback E2E displayed the effective
read scope and returned a denied tool result after clicking Deny. Fixtures used
private HOME/config/data and no production history or agent profiles. Windows
and Linux GUI validation remains with platform CI.

## Lessons

Perform readiness and context-policy decisions before collection, not after.
Local read-only access and transmission are distinct privacy boundaries.
An output cap does not constrain I/O, and a current directory is not a grant.
Future project-context automation must be explicitly scoped and bounded rather
than recreating the old scan with a shorter list of excluded directories.
