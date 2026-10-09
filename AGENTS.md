# con — Agent Guide

con is a Rust terminal emulator with a built-in AI agent harness, targeting
macOS, Windows, and Linux. It uses GPUI for UI, Ghostty for terminals, and Rig
for the agent. Use workspace manifests for current packages and versions.

## Working agreements

- For implementation requests, complete the change and relevant verification.
  Resolve routine, reversible choices from project context without pausing for
  approval at each step. Analysis and review requests stay read-only unless
  implementation is requested.
- Preserve unrelated work, including staged and untracked files. Do not stash,
  discard, or overwrite it. Keep changes within the requested scope.
- Commit when requested. Push, PR creation, merge, release, deployment, and
  destructive actions require authorization for that action; carry existing
  authorization forward without asking again.
- Report checks performed, verification gaps, and the actual delivery state:
  local, committed, pushed, merged, or released.

## Git conventions

- New branches use `<type>/<short-kebab-case>`, chosen by task purpose:
  `feat` for features, `fix` for bugs, and `refactor`, `perf`, `docs`, `test`,
  `build`, `ci`, `style`, `chore`, or `revert` for their respective changes.
  Examples: `fix/terminal-focus`, `feat/session-search`, `docs/agent-guidance`.
- Inspect the branch and worktree first. Reuse a branch for the same task;
  when starting implementation on `main`, create a task branch from the intended
  base. Do not switch an unrelated active branch or move existing work merely
  to satisfy naming. Existing branches and published history need no renaming.
- Authored commits and PR titles use Conventional Commits:
  `type(scope): imperative description`. Use the types above and English;
  scope is optional, preferably a subsystem such as `agent`, `cli`, or `ui`.
  Example: `fix(cli): terminate plain output with a newline`.
- Mark incompatible public changes with `!` and explain impact and migration
  in a `BREAKING CHANGE:` footer. Generated merge/revert messages may retain
  their tool-generated form; final squash titles follow the convention.
- Keep commits focused; separate refactoring from behavior changes when practical.
  Branch type describes the task, while commit type describes each commit.
  Inspect the staged diff and stage only intended files, not the whole worktree
  by default. Never invent issue references, author credits, or test results.

## Development and verification

Use `HACKING.md` for setup and `mise.toml` for tool versions, including the exact
Zig version. Select commands for the change; the pre-commit requirements below
are mandatory where applicable. Run through `mise exec --` when the tools are
not already on PATH.

| Command | Purpose |
|---------|---------|
| `just build` / `just run` | Build the app and companion CLI / run from source |
| `just check` | Platform-appropriate type checking |
| `just test` | The local platform's configured test set |
| `just lint` | Platform-appropriate Clippy checks |
| `cargo fmt --all -- --check` | Rust formatting check |

The UI lives in `crates/con-app/`, but its Cargo package is `con`. Windows uses
the `cargo w*` aliases in `.cargo/config.toml` to produce `con-app.exe` because
`CON` is reserved. Agent Handoff requires `con-cli` beside the app executable;
`just build` and `just run` prepare it, unlike building only the app.

### Before committing

- For Rust source changes, `cargo fmt --all -- --check` must pass.
- For code, dependency, or build changes, run tests covering the affected crates
  and behavior before committing. Start with the affected crate's tests, such as
  `cargo test -p con-core`; use the platform-appropriate aliases on Windows.
  Broaden to `just test` and affected-consumer tests for workspace-wide changes.
  Type checking or Clippy alone does not replace tests.
- Documentation-only changes may skip Rust formatting and tests; verify facts,
  paths, and any changed command examples instead.
- Results must cover the code being committed. Rerun checks affected by later
  edits, not unchanged checks for every commit. If required checks fail or cannot
  run, report the blocker and obtain an explicit waiver before committing;
  do not silently skip them or claim they passed.

### Verification scope

- Match verification to risk: documentation changes need fact/path checks, local
  logic needs focused unit tests, and shared behavior needs affected-consumer checks.
  Use `con-test` for integration that needs a running session. Tests should catch
  plausible mistakes, not mirror the implementation or cover trivial getters.
- Once relevant checks pass, broaden or repeat them only for new changes,
  failures, or unresolved concerns. Fix failures caused by the requested change.
- Inspect rendered results for visual UI changes, including affected non-default
  states. For interaction changes, exercise the changed behavior.
- A macOS check does not validate Windows/Linux `cfg` paths. For Ghostty FFI,
  run linked tests on the affected platform; `cargo check` alone misses link/ABI
  problems. Report unavailable platform coverage explicitly.
- Use isolated fixtures/sessions for tests that write data, not the user's live
  configuration or session. Read the relevant `.github/workflows/` file for actual
  CI coverage; local `just test` is not a substitute for all platform jobs.

## Project constraints

- `con-terminal` has no UI dependencies; `con-agent` has no terminal dependencies.
  Keep platform terminal details behind `con-ghostty`'s shared interfaces.
  `con-paths` owns application paths; OS observations from `con-process` are
  presentation facts, not authority for agent control.
- Pin `gpui-pre-*` and paired component packages exactly (`=`) and upgrade them
  together, including assets and the app's `gpui-base` test dependency. Keep one
  compatible GPUI package/source identity across the UI dependency graph.
- Dependencies come from crates.io or declared Git dependencies pinned to a
  revision. `3pp/` is optional, ignored, read-only reference: never modify,
  commit, or depend on it. Upstream dependency fixes instead of patching locally.
  OSC 7501 has a maintainer-authorized temporary exception (2026-10-09):
  Ghostty may use the public `wey-gu/ghostty` fork at the immutable
  revision in `con-ghostty/build.rs`. Track official replacement in #444;
  this exception does not permit local patches or other fork dependencies.
  Consult sources matching the resolved version; use upstream or installed
  package sources when a reference checkout is absent or stale.
- macOS embeds full libghostty; Windows/Linux use libghostty-vt with platform
  PTY/rendering paths. Preserve cell/grapheme boundaries through shaping;
  concatenating cells can recombine emoji incorrectly when DEC 2027 is off.
- Use the real Rig integration and shared Tokio runtime, not a runtime/thread
  per message. Commands run by Con's built-in agent must execute visibly through
  its established execution and approval flow.
- Config uses Ghostty line syntax with Con-owned dotted keys and Rust field names
  in `snake_case`; keep `con.version = 1`. If the primary file is absent, copy
  `config.ghostty` verbatim, or migrate `config.toml` only if neither native file
  exists. Preserve originals; never fall back from a malformed higher-priority file.
- Write workspace profiles as `.con/workspace.ghostty` v2; `.con/workspace.toml`
  v1 is read-only compatibility input. Session, auth, and history JSON are separate.

## UI constraints

- Use `gpui-component` controls before custom buttons, inputs, selects, or switches;
  consult the pinned API. Use IoskeleyMono for terminal/code contexts and the
  system UI font for prose/settings. Terminal backends need concrete font families,
  never GPUI aliases such as `.SystemUIFont`.
- Default to Flexoki Light and support Flexoki Dark. Con-authored surfaces stay
  borderless and shadowless, separated by opacity-based fills. Use typography for
  hierarchy and color for semantic states. Match adjacent rounding and panel widths.
- Con-authored icons use Phosphor assets in `assets/icons/phosphor/`; do not draw
  replacements manually. Existing third-party internal icon fallbacks do not permit
  new icon sets in Con's UI. Set `.text_color()` directly on every SVG.
- Preserve input-bar focus after submission. Dialogs capture and restore focus;
  `FocusInput` explicitly focuses the input bar. Keep labels compact, placeholders
  action-oriented, and redundant context out of the UI.

## Task-specific guidance

Read only guidance relevant to the task; `docs/README.md` indexes other topics.
These constraints take precedence over conflicting older examples in linked docs.

| Task | Guidance |
|------|----------|
| Architecture / boundaries | `DESIGN.md`, `docs/impl/workspace-modules.md` |
| Build / packaging | `HACKING.md`, `docs/impl/build-system.md` |
| Agent behavior / tools | `docs/impl/agent-harness.md`, `docs/impl/agent-tool-surface.md` |
| Config loading / saving / migration | `docs/impl/configuration.md` |
| Terminal rendering / ports | `docs/impl/terminal-rendering.md`, `docs/impl/windows-port.md`, `docs/impl/linux-port.md` |
| UI design / macOS focus | `docs/design/con-design-language.md`, `docs/impl/macos-focus-and-first-responder.md` |
| Socket API / live E2E / `con-test` tests | `docs/impl/socket-api.md`, `skills/con-cli-e2e/SKILL.md` |
| UI rendering performance | `skills/gpui-cache-aware/SKILL.md` |
| Changelog / release notes / release-visible PR descriptions | `skills/changelog-release-notes/SKILL.md` |

Before editing release notes, check the latest shipped beta and include PR links
and GitHub author credit for PR-derived items. For non-trivial bug fixes, create
`postmortem/YYYY-MM-DD-title.md` covering what happened, root cause, fix, and lessons.
Update guidance when a change establishes a durable constraint, rather than
accumulating incident history here.
