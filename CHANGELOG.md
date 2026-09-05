# Changelog

All notable changes to con are documented here.

con is still pre-release, so entries may group related beta work while the product shape is stabilizing.

## `v0.1.0-beta.95` - unreleased

### Fixed

**macOS**

- Terminal links now reject unsafe OSC 8 targets before opening a browser,
  while preserving supported web, mail, file, and SSH links. _(PR
  [#340](https://github.com/nowledge-co/con-terminal/pull/340) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_
- Terminal surfaces now pause before system sleep and resume only after the
  display topology has settled, avoiding the CoreVideo display-link hang
  observed when waking a Mac onto an external display. _(PR
  [#342](https://github.com/nowledge-co/con-terminal/pull/342) by
  [@wey-gu](https://github.com/wey-gu))_

---

## `v0.1.0-beta.94` - 2026-09-02

### Changed

**Windows and Linux**

- Terminal selection now uses libghostty-vt's canonical selection model,
  keeping drag, word, line, and scrollback selection behavior consistent with
  terminal state. _(PR
  [#338](https://github.com/nowledge-co/con-terminal/pull/338) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

### Fixed

**Windows**

- Mouse reporting now uses Ghostty's protocol encoder, improving wheel,
  button, modifier, coordinate, and alternate-scroll handling in terminal
  applications. _(PR
  [#339](https://github.com/nowledge-co/con-terminal/pull/339) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

---

## `v0.1.0-beta.93` - 2026-09-01

### Added

**macOS**

- SSH shell integration now supports Ghostty's `+ssh` wrapper through Con's
  bundle-local `con-cli`, including terminal environment forwarding and
  best-effort `xterm-ghostty` terminfo setup. If setup is unavailable, SSH
  continues with the standard `xterm-256color` terminal type. _(PR
  [#335](https://github.com/nowledge-co/con-terminal/pull/335) by
  [@wey-gu](https://github.com/wey-gu))_

**Windows and Linux**

- Terminal updates now consume incremental dirty-row changes, reducing
  unnecessary parser and renderer work during continuous output and window
  resizing. _(PR
  [#336](https://github.com/nowledge-co/con-terminal/pull/336) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

**Developer tools**

- Terminal execution now recognizes semantic shell prompt state on Windows and
  Linux when shell integration is available, improving completion detection
  for visible agent-run commands while retaining the existing fallback. _(PR
  [#328](https://github.com/nowledge-co/con-terminal/pull/328) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_
- Agent-run commands now report an unconfirmed result instead of claiming
  success when shell prompt integration disappears before completion can be
  verified. _(PR
  [#330](https://github.com/nowledge-co/con-terminal/pull/330) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_
- Portable terminal sessions now surface title, working-directory, bell,
  progress, notification, and clipboard effects consistently on Windows and
  Linux, with bounded payloads and conservative clipboard defaults. _(PR
  [#331](https://github.com/nowledge-co/con-terminal/pull/331) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_
- The control plane can now identify the foreground process group and PTY for
  macOS panes when the terminal backend supports those details. _(PR
  [#332](https://github.com/nowledge-co/con-terminal/pull/332) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_
- macOS release validation now checks the bundle-local SSH compatibility
  entry point in the app bundle, ZIP, and DMG before artifacts can be
  published. _(PR [#334](https://github.com/nowledge-co/con-terminal/pull/334)
  and PR [#337](https://github.com/nowledge-co/con-terminal/pull/337) by
  [@wey-gu](https://github.com/wey-gu))_

---

## `v0.1.0-beta.92` - 2026-08-31

### Fixed

**macOS**

- Improved IME composition rendering by keeping marked text, cursor
  suppression, underlining, and cell layout in the terminal renderer. _(PR
  [#326](https://github.com/nowledge-co/con-terminal/pull/326) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

---

## `v0.1.0-beta.91` - 2026-08-31

### Fixed

**macOS**

- Removed an unsupported Ghostty SSH terminfo helper invocation that could
  print a missing `Contents/MacOS/ghostty` error before SSH connections. SSH
  environment integration remains enabled. _(PR [#321](https://github.com/nowledge-co/con-terminal/pull/321) by [@wey-gu](https://github.com/wey-gu))_
- Restored automatic SSH terminfo setup by routing Ghostty's existing shell
  integration helper through the native `con-cli` implementation in the app
  bundle. _(PR [#323](https://github.com/nowledge-co/con-terminal/pull/323) by [@wey-gu](https://github.com/wey-gu))_

### Added

**Developer tools**

- Added the native `con-cli +ssh-cache` command for tracking remote hosts that
  already have Con's terminal definition installed. The cache stores host
  metadata only and provides stable exit codes for shell integration. _(PR [#322](https://github.com/nowledge-co/con-terminal/pull/322) by [@wey-gu](https://github.com/wey-gu))_

---

## `v0.1.0-beta.90` - 2026-08-30

### Added

**Linux**

- Con now works as an `xdg-terminal-exec` provider, so desktop launchers can
  open a requested directory or start a TUI with its arguments, title, and app
  identity preserved. The official installer, tarball, and Flatpak metadata
  advertise the same capabilities. _(PR
  [#317](https://github.com/nowledge-co/con-terminal/pull/317) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Linux**

- Matched the installed launcher icon name to Con's desktop application id, so
  launchers can resolve the correct icon and group windows consistently. Older
  release archives remain compatible with the installer. _(PR
  [#318](https://github.com/nowledge-co/con-terminal/pull/318) by
  [@wey-gu](https://github.com/wey-gu))_

---

## `v0.1.0-beta.89` - 2026-08-30

### Added

**Settings**

- Appearance now includes a selectable raccoon app-icon grid. On macOS the
  choice updates the Dock and Cmd-Tab image for the running app; Finder still
  uses the bundled icon. _(PR
  [#311](https://github.com/nowledge-co/con-terminal/pull/311) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**macOS**

- Refined trackpad scrolling in shell scrollback by preserving precise input
  deltas instead of applying an extra host-side speed multiplier. This keeps
  slow, fine-grained scrolling from feeling unexpectedly fast while leaving
  terminal applications' own scroll handling unchanged. _(PR
  [#308](https://github.com/nowledge-co/con-terminal/pull/308) by
  [@wey-gu](https://github.com/wey-gu))_

### Changed

**macOS**

- Added a subtle overlay scrollbar for terminal scrollback. It appears only
  when a pane has history beyond the visible screen, without changing the
  terminal's layout or transparency behavior. _(PR
  [#309](https://github.com/nowledge-co/con-terminal/pull/309) by
  [@wey-gu](https://github.com/wey-gu))_
- Clicking a scrollback scrollbar now also focuses its pane, so the next
  keyboard input goes to the pane the user is working with. _(PR
  [#310](https://github.com/nowledge-co/con-terminal/pull/310) by
  [@wey-gu](https://github.com/wey-gu))_

**Branding**

- Updated the company name shown in Con's About window, license, and Flatpak
  metadata to **Nowledge Labs, Inc.** Apple signing and notarization settings
  are unchanged. _(PR
  [#306](https://github.com/nowledge-co/con-terminal/pull/306) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Input Bar**

- Restored the UI typeface for natural-language Agent input and aligned its
  baseline with Smart and Shell modes. The mode control now also shows the
  configured shortcut for switching modes with the same keycaps used elsewhere
  in Con. _(PR
  [#307](https://github.com/nowledge-co/con-terminal/pull/307) by
  [@wey-gu](https://github.com/wey-gu))_

---

## `v0.1.0-beta.88` - 2026-08-28

### Added

**Settings**

- Added per-shortcut and global reset actions in Keyboard Shortcuts, so custom
  bindings can be restored without rebuilding the entire configuration. _(PR
  [#305](https://github.com/nowledge-co/con-terminal/pull/305) by
  [@zhangmo8](https://github.com/zhangmo8))_

### Fixed

**Updates and Releases**

- Improved release and update reliability when macOS, Linux, and Windows
  artifacts finish concurrently. Appcast publication now retries safely, and
  incomplete releases remain unpublished instead of being exposed to users.
  _(PR [#304](https://github.com/nowledge-co/con-terminal/pull/304) by
  [@wey-gu](https://github.com/wey-gu))_

**Settings**

- Fixed Shift disappearing from shortcut keycaps when macOS records the shifted
  character directly. _(PR
  [#305](https://github.com/nowledge-co/con-terminal/pull/305) by
  [@zhangmo8](https://github.com/zhangmo8))_

---

## `v0.1.0-beta.87` - 2026-08-27

### Added

**macOS**

- Added **Find in Terminal** with Cmd+F, match counts, next/previous
  navigation, and selection highlights backed by Ghostty's native terminal
  search. It is also available from the Edit menu, command palette, and
  configurable shortcuts. _(PR
  [#303](https://github.com/nowledge-co/con-terminal/pull/303) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

### Fixed

**Settings**

- Fixed the unsaved-changes confirmation flow for the Settings panel. Escape,
  Cmd+W, and clicking outside the panel now close it consistently when it is
  safe to do so, while preserving the option to keep editing, discard, or
  save changes. _(PR
  [#301](https://github.com/nowledge-co/con-terminal/pull/301) by
  [@zhangmo8](https://github.com/zhangmo8))_

**Terminal**

- Fixed terminal mouse interactions that begin inside a pane and end outside
  it. Con now completes the matching mouse release exactly once, including
  during focus changes and pane teardown, so selections and mouse-aware TUIs
  do not remain stuck. _(PR
  [#302](https://github.com/nowledge-co/con-terminal/pull/302) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

---

## `v0.1.0-beta.86` - 2026-08-27

### Added

**Windows, Linux**

- Added portable Kitty keyboard encoding, mediated clipboard paste, and
  bounded static image rendering while retaining each platform's native PTY
  and renderer. _(PR
  [#297](https://github.com/nowledge-co/con-terminal/pull/297) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

**Window Management**

- Added an option to keep a window open after its last tab is closed. When
  enabled, Con opens a fresh tab in that window instead of closing it. _(PR
  [#295](https://github.com/nowledge-co/con-terminal/pull/295) by
  [@zhangmo8](https://github.com/zhangmo8))_

---

## `v0.1.0-beta.85` - 2026-08-25

### Added

**macOS**

- Added native desktop notifications for terminal applications that emit OSC 9
  or OSC 777 sequences, including the first notification shown while Con asks
  for notification permission. _(PR
  [#290](https://github.com/nowledge-co/con-terminal/pull/290) by
  [@yyhhyyyyyy](https://github.com/yyhhyyyyyy))_

**Packaging**

- Added Flatpak packaging and automated AetherPak repository deployment to GitHub Pages (`https://con-releases.nowledge.co/flatpak`), supporting both `x86_64` and `aarch64` architectures. _(by [@abn](https://github.com/abn))_
- Added automatic container detection and host shell forwarding in the Linux PTY backend using `flatpak-spawn --host`. _(by [@abn](https://github.com/abn))_

### Fixed

**Linux**

- Fixed top bar window decorations to support drag-to-move across Linux window managers. _(by [@abn](https://github.com/abn))_

---

## `v0.1.0-beta.84` - 2026-08-13

### Added

**Editor**

- Added a read-only image viewer for local image files opened from the file
  tree, sidebar search, or drag-and-drop. Images fit the editor pane without
  entering text-edit mode, and oversized files show a safe placeholder instead
  of being decoded at full resolution; the guard checks both file size and
  declared image dimensions before previewing. _(PR
  [#270](https://github.com/nowledge-co/con-terminal/pull/270) by
  [@sunny0826](https://github.com/sunny0826))_

### Fixed

**Sidebar**

- Fixed sidebar rail icons and file-search result icons stretching when the
  window is compressed. The session rail now keeps controls fixed and scrolls
  the session list instead of shrinking icon boxes. _(PR
  [#272](https://github.com/nowledge-co/con-terminal/pull/272) by
  [@sunny0826](https://github.com/sunny0826))_

---

## `v0.1.0-beta.83` - 2026-08-11

### Fixed

**Terminal**

- Forward right-clicks to terminal apps that enabled mouse reporting, so TUI
  menus work as expected while Con's context menu still appears for ordinary
  shell right-clicks. _(PR
  [#266](https://github.com/nowledge-co/con-terminal/pull/266) by
  [@sunny0826](https://github.com/sunny0826))_
- Fixed shifted text input in macOS terminal applications such as LazyGit, so
  `Shift+P` reaches apps as uppercase `P` instead of triggering lowercase `p`
  actions. _(PR [#264](https://github.com/nowledge-co/con-terminal/pull/264) by
  [@Eric-Song-Nop](https://github.com/Eric-Song-Nop))_

---

## `v0.1.0-beta.82` - 2026-08-07

### Changed

**Markdown**

- Updated the Mermaid renderer and HTML parser stack used by rich Markdown so
  diagram and inline HTML previews stay aligned with current upstream fixes.
  _(PR [#263](https://github.com/nowledge-co/con-terminal/pull/263) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Sidebar**

- Fixed pinned sidebar tab clicks so selecting the active tab no longer
  collapses the panel, preserving drag reorder and double-click rename flows.
  _(PR [#261](https://github.com/nowledge-co/con-terminal/pull/261) by
  [@sunny0826](https://github.com/sunny0826))_
- Fixed vertical-tab hover cards so they render as workspace overlays, stay
  inside the window, and clear stale hover state when the rail is hidden. _(PR
  [#262](https://github.com/nowledge-co/con-terminal/pull/262) by
  [@sunny0826](https://github.com/sunny0826))_

---

## `v0.1.0-beta.81` - 2026-08-06

### Added

**Editor**

- Added a file-tree affordance to open files in a dedicated editor tab. The
  editor tab is reused for additional files, keeps terminal pane layouts
  untouched, and remains separate from the existing row-click behavior. _(PR
  [#257](https://github.com/nowledge-co/con-terminal/pull/257) by
  [@sunny0826](https://github.com/sunny0826))_

### Fixed

**Editor**

- Fixed Markdown preview alignment in wide editor panes: code blocks, tables,
  and Mermaid diagrams now share the centered content column instead of
  spanning the pane edge, and long code lines scroll horizontally. _(PR
  [#258](https://github.com/nowledge-co/con-terminal/pull/258) by
  [@sunny0826](https://github.com/sunny0826))_

---

## `v0.1.0-beta.80` - 2026-08-05

### Added

**Editor**

- Added an in-place Markdown preview for editor tabs, with live refresh,
  rendered tables, code blocks, math, Mermaid diagrams, common inline HTML,
  and local or remote images resolved from the document. _(PR
  [#254](https://github.com/nowledge-co/con-terminal/pull/254) by
  [@sunny0826](https://github.com/sunny0826))_

### Fixed

**Editor**

- Fixed Markdown preview layout in wide editor panes: blockquotes now wrap
  inside the preview column, and prose is centered instead of hugging the left
  edge. _(PR [#255](https://github.com/nowledge-co/con-terminal/pull/255) by
  [@sunny0826](https://github.com/sunny0826))_

---

## `v0.1.0-beta.79` - 2026-07-25

### Changed

**Agent**

- Updated Con's built-in agent to Rig 0.40 while preserving live responses,
  tool approvals, conversation history, and existing turn limits. The agent now
  follows Rig's published release instead of an interim source revision. _(PR
  [#250](https://github.com/nowledge-co/con-terminal/pull/250) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Providers**

- Fixed ChatGPT Subscription model discovery after the Codex model catalog
  started requiring an explicit client compatibility version. **Fetch Models**
  now loads the account's available catalog, including GPT-5.6 models where
  available, and the offline fallback includes the GPT-5.6 family. _(PR
  [#252](https://github.com/nowledge-co/con-terminal/pull/252) by
  [@wey-gu](https://github.com/wey-gu); reported in issue
  [#251](https://github.com/nowledge-co/con-terminal/issues/251) by
  [@zjp1997720](https://github.com/zjp1997720))_

---

## `v0.1.0-beta.78` - 2026-07-15

### Added

**Agent**

- Added **Ask AI** for terminal selections. Use the terminal context menu or
  the configurable shortcut to place selected output in the agent input as a
  fenced block. Assistant replies can also switch into selectable-text mode,
  and thinking or tool-output details now have their own copy controls. _(PR
  [#248](https://github.com/nowledge-co/con-terminal/pull/248) by
  [@hhh2210](https://github.com/hhh2210))_

### Changed

**Providers**

- Made ChatGPT Subscription ready on first launch for users already signed in
  to Codex. Con now imports the compatible Codex auth cache during config load,
  chooses ChatGPT only when the provider is still an implicit, unconfigured
  default, and uses GPT-5.5 as the default ChatGPT model. Explicit provider and
  model choices remain unchanged. _(PR
  [#245](https://github.com/nowledge-co/con-terminal/pull/245) by
  [@hhh2210](https://github.com/hhh2210))_
- Updated the offline OpenAI model list with current GPT-5.5 and GPT-5.4
  options, while retaining supported o3 and GPT-4.1 models. These choices stay
  available when a compatible endpoint returns an incomplete model catalog.
  _(PR [#246](https://github.com/nowledge-co/con-terminal/pull/246) by
  [@dongshunyao](https://github.com/dongshunyao))_

### Fixed

**Agent**

- Fixed ChatGPT Subscription auth becoming stale after Codex rotates its
  credentials. Con refreshes imported credentials without overwriting a cache
  refreshed locally, clears stale state before manual reauthorization, and
  keeps the model picker focused on models available to subscription accounts.
  _(PR [#244](https://github.com/nowledge-co/con-terminal/pull/244) by
  [@hhh2210](https://github.com/hhh2210))_

**Chat**

- Fixed code-block copy feedback leaking between messages. Each copy control
  now keeps independent state, including code blocks nested inside lists and
  blockquotes. _(PR
  [#247](https://github.com/nowledge-co/con-terminal/pull/247) by
  [@hhh2210](https://github.com/hhh2210))_

---

## `v0.1.0-beta.77` - 2026-06-08

### Added

**Agent**

- Added Codex ChatGPT auth-cache import support so the built-in agent can
  reuse an existing authenticated Codex session instead of requiring a separate
  credential setup path. _(PR
  [#223](https://github.com/nowledge-co/con-terminal/pull/223) by
  [@hhh2210](https://github.com/hhh2210))_

### Changed

**Input Bar**

- Made multiline command input grow dynamically with its contents and stay
  readable while editing longer commands. _(PR
  [#235](https://github.com/nowledge-co/con-terminal/pull/235) by
  [@frostming](https://github.com/frostming), PR
  [#241](https://github.com/nowledge-co/con-terminal/pull/241) by
  [@sundy-li](https://github.com/sundy-li))_

### Fixed

**Agent**

- Prevented interactive agent-CLI panes from getting stuck behind prompt-wait
  detection that should only apply to shell-style panes. _(PR
  [#240](https://github.com/nowledge-co/con-terminal/pull/240) by
  [@Job28703](https://github.com/Job28703))_

**Providers**

- Fixed stale provider activation after credentials are cleared, so removed
  credentials no longer leave a provider looking usable in the UI. _(PR
  [#228](https://github.com/nowledge-co/con-terminal/pull/228) by
  [@tao12345666333](https://github.com/tao12345666333))_

**Terminal**

- Fixed Quick Terminal IME targeting on macOS so marked text and committed IME
  input route to the active terminal pane reliably. _(PR
  [#237](https://github.com/nowledge-co/con-terminal/pull/237) by
  [@sundy-li](https://github.com/sundy-li))_
- Improved terminal recovery when child processes exit or SSH sessions
  disconnect, keeping terminal state transitions aligned with the visible pane
  lifecycle. _(PR
  [#234](https://github.com/nowledge-co/con-terminal/pull/234) by
  [@frostming](https://github.com/frostming))_

---

## `v0.1.0-beta.76` - 2026-05-14

### Changed

**Sidebar**

- Polished the Files/Search sidebar controls, file-tree hover treatment, and
  dark-mode chrome affordances so sidebar tools stay readable without adding
  extra visual weight. _(PR
  [#215](https://github.com/nowledge-co/con-terminal/pull/215) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Build**

- Hardened Windows and Linux libghostty-vt builds against transient Zig package
  fetch failures during both `zig build -h` probing and the actual VT build, so
  release builds retry after prewarming Zig's package cache instead of
  misreporting a package-fetch error as a missing libghostty-vt build knob.
  _(PR [#215](https://github.com/nowledge-co/con-terminal/pull/215) by
  [@wey-gu](https://github.com/wey-gu))_

**Sidebar**

- Fixed a sidebar hover performance regression by keeping file-tree hover state
  on stable row elements and explicitly occluding sidebar surfaces from the
  embedded terminal underneath. _(PR
  [#215](https://github.com/nowledge-co/con-terminal/pull/215) by
  [@wey-gu](https://github.com/wey-gu))_

**Settings**

- Corrected the Settings unsaved-state colors in light and dark themes so the
  warning status uses the semantic warning color instead of the foreground color
  intended for text on warning backgrounds. _(PR
  [#215](https://github.com/nowledge-co/con-terminal/pull/215) by
  [@wey-gu](https://github.com/wey-gu))_

---

## `v0.1.0-beta.75` - 2026-05-13

### Fixed

**macOS**

- Changed the default Next/Previous Surface shortcuts from
  `Cmd+Option+]` / `Cmd+Option+[` to `Cmd+Control+]` / `Cmd+Control+[`
  because macOS resolves Option+bracket to punctuation before Con can receive
  it as a shortcut. Existing configs with exactly the old default values are
  migrated on load. _(Issue
  [#211](https://github.com/nowledge-co/con-terminal/issues/211), PR
  [#214](https://github.com/nowledge-co/con-terminal/pull/214) by
  [@wey-gu](https://github.com/wey-gu))_
- Fixed macOS IME commits in Ghostty terminal panes so Chinese/Pinyin and
  WeChat IME text repaints immediately, candidate windows stay anchored to the
  current cursor bounds, and marked preedit text is rendered at the terminal
  cursor. _(Issue
  [#210](https://github.com/nowledge-co/con-terminal/issues/210), PR
  [#213](https://github.com/nowledge-co/con-terminal/pull/213) by
  [@sundy-li](https://github.com/sundy-li))_

---

## `v0.1.0-beta.74` - 2026-05-13

### Changed

**Linux**

- Polished the Linux integrated titlebar and terminal surface so one-tab and
  multi-tab windows keep consistent titlebar controls, sidebar actions remain
  reachable from the chrome, X11 rounded corners match tiled/free-corner states
  more closely, and transparent terminal panes avoid double-tinted backgrounds.
  _(PR [#203](https://github.com/nowledge-co/con-terminal/pull/203) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Sidebar**

- In vertical tabs mode, creating or duplicating a tab while the left sidebar is
  hidden now reveals the compact tab rail, keeping existing tabs visible without
  opening the Files/Search drawer or the full session panel. _(PR
  [#212](https://github.com/nowledge-co/con-terminal/pull/212) by
  [@wey-gu](https://github.com/wey-gu))_

**Terminal**

- Hardened startup environment handling so terminals launched from wrapper
  shells do not inherit Conductor's temporary zsh integration directory as
  their own `ZDOTDIR`, preserving the user's normal prompt and shell rc loading.
  _(PR [#212](https://github.com/nowledge-co/con-terminal/pull/212) by
  [@wey-gu](https://github.com/wey-gu))_

---

## `v0.1.0-beta.73` - 2026-05-13

### Fixed

**Terminal**

- Improved terminal input responsiveness by keeping normal keypresses out of
  workspace focus-change handling, making input-bar pane metadata sync
  idempotent, reasserting the active input target when returning to a Con
  window from another app, and moving skill discovery filesystem work off the
  render path. _(PR
  [#208](https://github.com/nowledge-co/con-terminal/pull/208) by
  [@wey-gu](https://github.com/wey-gu))_

## `v0.1.0-beta.72` - 2026-05-12

### Changed

**Sidebar**

- New installs and configs that omit `appearance.tabs_orientation` now default
  to vertical tabs. Explicit `tabs_orientation = "horizontal"` configs still
  keep the horizontal top tab strip. _(PR
  [#205](https://github.com/nowledge-co/con-terminal/pull/205) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Build**

- Made the macOS libghostty build retry path prefetch Zig package
  dependencies into the same global cache used by the retry build, and use
  shallow git fetches for git-backed Zig packages, so transient Zig package
  fetch failures are less likely to block local or CI builds. _(PR
  [#204](https://github.com/nowledge-co/con-terminal/pull/204) by
  [@sundy-li](https://github.com/sundy-li))_

## `v0.1.0-beta.71` - 2026-05-12

### Added

**Editor**

- Added the first built-in code editor surface, with editable multi-file editor
  panes, file-tree and search panels in the left sidebar, syntax highlighting,
  basic diagnostics, save/undo/selection support, and editor panes that
  participate in the existing split-pane flow. _(PR
  [#179](https://github.com/nowledge-co/con-terminal/pull/179) by
  [@sundy-li](https://github.com/sundy-li))_

### Fixed

**Sidebar**

- Restored vertical tabs inside the left sidebar and brought back full sidebar
  hide/unhide behavior. `appearance.tabs_orientation` again controls horizontal
  vs. vertical tabs, and Files/Search now opens as a lightweight drawer instead
  of a permanent second sidebar beside vertical tabs. Follow-up normalization
  made Files/Search and sessions one left-sidebar system, so the active panel
  sits beside the tool/session rail instead of overlaying terminal content, uses
  the same total width as the unfolded session list, and folds cleanly back to
  the rail from any active panel. _(PR
  [#194](https://github.com/nowledge-co/con-terminal/pull/194) by
  [@wey-gu](https://github.com/wey-gu), PR
  [#200](https://github.com/nowledge-co/con-terminal/pull/200) by
  [@wey-gu](https://github.com/wey-gu), PR
  [#201](https://github.com/nowledge-co/con-terminal/pull/201) by
  [@wey-gu](https://github.com/wey-gu))_
- Added first-class Files and Search Files commands, menu items, command
  palette entries, editable keybindings, and defaults (`Cmd+Option+E` for
  Files on macOS, `Ctrl+Shift+E` for Files on Windows/Linux, and
  `Cmd/Ctrl+Shift+F` for Search). Search Files focuses the search query
  immediately. _(PR
  [#201](https://github.com/nowledge-co/con-terminal/pull/201) by
  [@wey-gu](https://github.com/wey-gu))_

**Editor**

- Closing the last file in an editor-only pane now returns the tab to a terminal
  instead of leaving an empty editor surface behind. _(PR
  [#201](https://github.com/nowledge-co/con-terminal/pull/201) by
  [@wey-gu](https://github.com/wey-gu))_
- Softened inactive editor and pane titles so unfocused file/editor chrome no
  longer renders with a heavy outlined look. _(PR
  [#201](https://github.com/nowledge-co/con-terminal/pull/201) by
  [@wey-gu](https://github.com/wey-gu))_

**macOS**

- Bundled Ghostty's compiled `xterm-ghostty` terminfo database inside the macOS
  app, so release builds can use the same terminal capabilities as local
  `cargo run` builds. The macOS verifier now fails the release if the terminfo
  entry is missing. _(PR
  [#195](https://github.com/nowledge-co/con-terminal/pull/195) by
  [@sundy-li](https://github.com/sundy-li))_

**Tabs**

- Fixed vertical tab titles so changing directories clears stale AI-generated
  labels immediately and ignores late summary responses for the previous
  directory. _(PR
  [#192](https://github.com/nowledge-co/con-terminal/pull/192) by
  [@iknowu10](https://github.com/iknowu10))_

### Changed

**Workspace**

- Replaced vertical tabs with an always-visible left activity rail and a
  collapsible Files/Search panel while keeping horizontal tabs as the tab
  surface. Existing `toggle_vertical_tabs` keybindings and
  `vertical_tabs_width` session values are accepted as compatibility aliases.
  _(PR [#179](https://github.com/nowledge-co/con-terminal/pull/179) by
  [@sundy-li](https://github.com/sundy-li))_

**Docs**

- Added shell-integration troubleshooting to the install guide, with manual
  zsh, bash, and fish fallback snippets for users whose startup files prevent
  Ghostty's auto-injection from loading. _(PR
  [#187](https://github.com/nowledge-co/con-terminal/pull/187) by
  [@iknowu10](https://github.com/iknowu10))_

## `v0.1.0-beta.70` - 2026-05-12

### Fixed

**AI Providers**

- Fixed MiniMax, Moonshot, and Z.AI provider settings so switching a provider
  between OpenAI-compatible and Anthropic protocol modes preserves the intended
  endpoint preset. Domestic/China base URLs no longer fall back to the global
  endpoint after saving and reopening Settings. _(Issue
  [#178](https://github.com/nowledge-co/con-terminal/issues/178), PR
  [#189](https://github.com/nowledge-co/con-terminal/pull/189) by
  [@wey-gu](https://github.com/wey-gu))_
- OpenAI Compatible providers can now be saved and used without an API key, so
  local services such as OMLX do not require a fake `sk-...` credential just to
  pass Settings validation. Explicit keys and env-var credentials still work
  for hosted endpoints. _(Issue
  [#147](https://github.com/nowledge-co/con-terminal/issues/147), PR
  [#189](https://github.com/nowledge-co/con-terminal/pull/189) by
  [@wey-gu](https://github.com/wey-gu))_

**UI**

- Fixed UI font-size scaling in terminal-adjacent chrome: the bottom input bar,
  agent panel messages, and run-trace cards now follow the Appearance → UI Size
  setting instead of staying at small fixed system sizes. _(Issue
  [#190](https://github.com/nowledge-co/con-terminal/issues/190), PR
  [#191](https://github.com/nowledge-co/con-terminal/pull/191) by
  [@wey-gu](https://github.com/wey-gu), reported by
  [@leek_ed](https://x.com/leek_ed))_

**macOS**

- Set `GHOSTTY_RESOURCES_DIR` from the installed app bundle when the build-time
  Ghostty resources path is unavailable, so manual shell-integration source
  fallbacks work in release builds as well as `cargo run` builds. _(PR
  [#188](https://github.com/nowledge-co/con-terminal/pull/188) by
  [@wey-gu](https://github.com/wey-gu), following PR
  [#187](https://github.com/nowledge-co/con-terminal/pull/187) by
  [@iknowu10](https://github.com/iknowu10))_

## `v0.1.0-beta.69` - 2026-05-11

### Added

**macOS**

- Added the standard <kbd>⌘</kbd><kbd>M</kbd> shortcut and Window menu action
  for minimizing Con windows, including terminal-focused windows. _(PR
  [#175](https://github.com/nowledge-co/con-terminal/pull/175) by
  [@chenghuzi](https://github.com/chenghuzi))_

**Developer Experience**

- Added a mise-pinned contributor toolchain for Rust, Zig, and CMake, and
  updated the hacking guide so new contributors can bootstrap the exact
  supported build tools with `mise install` before running the usual checks.
  _(Issue [#174](https://github.com/nowledge-co/con-terminal/issues/174), PR
  [#183](https://github.com/nowledge-co/con-terminal/pull/183) by
  [@tao12345666333](https://github.com/tao12345666333))_

### Changed

**Windows**

- Improved the default Windows terminal pane presentation with a small content
  inset and native-edge caption alignment, so the prompt no longer sits flush
  against the window edge, the inset gutter matches the terminal background,
  and the close button reaches the right edge of the titlebar. _(Issues
  [#131](https://github.com/nowledge-co/con-terminal/issues/131) and
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**Keyboard Shortcuts**

- Fixed shortcut lifecycle issues in Settings: changed shortcuts now apply
  immediately without restarting Con, duplicate assignments are blocked with a
  clear conflict warning, and the command palette shortcut now refocuses the
  palette instead of getting stuck when pressed repeatedly while modifiers are
  held. _(Issue
  [#182](https://github.com/nowledge-co/con-terminal/issues/182), PR
  [#186](https://github.com/nowledge-co/con-terminal/pull/186) by
  [@wey-gu](https://github.com/wey-gu))_

**Panes**

- Fixed pane-local surface tab strips so they use the same themed chrome
  backing as split-pane title bars instead of leaking transparent window
  content, and tightened their spacing, typography, and active/hover states.
  _(PR [#184](https://github.com/nowledge-co/con-terminal/pull/184) by
  [@wey-gu](https://github.com/wey-gu))_

**Windows, macOS, and Linux**

- New windows opened from an existing Con window now inherit the focused
  terminal's current directory instead of falling back to the shell default
  home directory. _(PR
  [#185](https://github.com/nowledge-co/con-terminal/pull/185) by
  [@iknowu10](https://github.com/iknowu10))_

**Windows**

- Improved Windows CJK rendering by anchoring fallback CJK glyphs to the
  terminal cell baseline, drawing East Asian text through a medium-weight
  DirectWrite format, and applying only a moderate CJK-specific grayscale
  contrast boost. This reduces vertical drift and thin-looking Chinese strokes
  without over-darkening Latin text or Nerd-Font prompt icons. _(Issues
  [#124](https://github.com/nowledge-co/con-terminal/issues/124),
  [#78](https://github.com/nowledge-co/con-terminal/issues/78), and
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_
- Raised the Windows alternate-screen background opacity floor for full-screen
  TUIs such as htop and vim to a solid app canvas, so their default-background
  screens match Windows Terminal/Ghostty instead of compositing with shell
  content underneath. Ordinary shell panes still keep the configured terminal
  glass effect. _(Issue [#34](https://github.com/nowledge-co/con-terminal/issues/34),
  PR [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_
- Polished Windows and Linux caption-button hover states so minimize and
  maximize/restore give visible feedback instead of only the close button
  feeling interactive. _(Issues
  [#18](https://github.com/nowledge-co/con-terminal/issues/18) and
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_
- Stopped stale Windows terminal frames from stretching while panes close,
  split, or resize, so existing terminal content no longer appears briefly
  zoomed before the next renderer frame arrives, and covered newly exposed
  stale-frame gaps with the terminal background instead of leaking transparent
  window content. _(Issue
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_
- Fixed restored Windows sessions so the saved working directory remains the
  ConPTY launch/current-directory fallback until shell integration reports a
  newer path. Con now also installs a lightweight PowerShell/pwsh prompt hook
  that emits cwd updates, so restored Windows sessions follow directory
  changes made after startup; the portable VT wrapper now captures OSC 7
  directly because libghostty-vt's terminal-only stream intentionally ignores
  cwd reports. _(Issue
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_

**Windows and Linux**

- Tightened portable terminal chrome coverage so pane dividers, vertical-tab
  edges, input bar motion, and agent-panel drag/show/hide transitions use
  terminal-colored seam covers and terminal-matched backing instead of
  transparent hit areas that could leak the desktop/window backdrop during fast
  movement. _(Issues
  [#18](https://github.com/nowledge-co/con-terminal/issues/18) and
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_
- Matched macOS terminal behavior on the portable backends: when a user has
  scrolled back and then types or pastes input, the viewport now returns to the
  live prompt instead of leaving the terminal parked in history. _(Issues
  [#18](https://github.com/nowledge-co/con-terminal/issues/18) and
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_
- Fixed terminal selection copy semantics on the portable backends:
  `Ctrl+C` now copies and clears the current selection when text is selected,
  while still sending interrupt when nothing is selected. The Linux preview
  also now supports left-drag text selection in the terminal pane and clears
  stale highlights when fresh terminal output replaces the grid. _(Issues
  [#177](https://github.com/nowledge-co/con-terminal/issues/177),
  [#18](https://github.com/nowledge-co/con-terminal/issues/18), and
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_
- Made portable terminal selections scrollback-stable: when the viewport moves
  while text is selected, the highlight now remains attached to the selected
  terminal content instead of staying pinned to the same screen rows. _(Issues
  [#18](https://github.com/nowledge-co/con-terminal/issues/18) and
  [#34](https://github.com/nowledge-co/con-terminal/issues/34), PR
  [#169](https://github.com/nowledge-co/con-terminal/pull/169) by
  [@wey-gu](https://github.com/wey-gu))_

## `v0.1.0-beta.68` - 2026-05-09

### Added

**Developer Experience**

- Added `con-test`, a sqllogictest-style E2E runner for driving real Con
  sessions through `con-cli`, with inline match modes, rewrite support, macOS
  E2E CI, and initial control-plane coverage for system, tab, pane, workspace,
  and agent-panel flows. _(PR
  [#170](https://github.com/nowledge-co/con-terminal/pull/170) by
  [@sundy-li](https://github.com/sundy-li), PR
  [#173](https://github.com/nowledge-co/con-terminal/pull/173) by
  [@sundy-li](https://github.com/sundy-li))_

**Agent and Settings**

- Added control-plane commands for opening the agent panel for a request and
  reading panel state, plus `[network]` proxy settings that can override or
  clear shell-inherited `HTTP_PROXY` / `HTTPS_PROXY` values. _(PR
  [#173](https://github.com/nowledge-co/con-terminal/pull/173) by
  [@sundy-li](https://github.com/sundy-li))_

### Changed

**Panes**

- Refined split-pane chrome so the active pane gets a compact accent-colored
  dot and pane title bars use the theme title-bar surface instead of
  transparency, keeping them readable over translucent windows. _(PR
  [#173](https://github.com/nowledge-co/con-terminal/pull/173) by
  [@sundy-li](https://github.com/sundy-li), PR
  [#176](https://github.com/nowledge-co/con-terminal/pull/176) by
  [@sundy-li](https://github.com/sundy-li))_

**Docs**

- Added the code editor integration design for the activity bar, file tree,
  and editor area layout that will host future editor work.

### Fixed

**Startup Environment**

- macOS and Linux GUI launches now inherit the user's login-shell environment
  before applying Con config, so PATH, language toolchains, and proxy variables
  match terminal-launched sessions more closely. _(PR
  [#173](https://github.com/nowledge-co/con-terminal/pull/173) by
  [@sundy-li](https://github.com/sundy-li))_

## `v0.1.0-beta.67` - 2026-05-08

### Fixed

**Tabs and Panes**

- New panes created by Con's agent/control plane now inherit the focused pane's
  working directory, and new tabs inherit the active terminal's restored working
  directory instead of dropping back to the shell default. _(Issue
  [#171](https://github.com/nowledge-co/con-terminal/issues/171), PR
  [#172](https://github.com/nowledge-co/con-terminal/pull/172) by
  [@wey-gu](https://github.com/wey-gu))_

## `v0.1.0-beta.66` - 2026-05-08

### Changed

**Settings**

- Matched the standalone Settings window titlebar to Con's main macOS window
  chrome while leaving the existing Windows and Linux settings-window frame
  behavior unchanged. _(PR
  [#166](https://github.com/nowledge-co/con-terminal/pull/166) by
  [@wey-gu](https://github.com/wey-gu))_

**Docs**

- Added the Quick Terminal screenshot to the user guide and screenshot gallery,
  credited its original contributor, and linked the hosted docs, changelog, and
  `llms.txt` agent index from the README. _(PR
  [#167](https://github.com/nowledge-co/con-terminal/pull/167) by
  [@wey-gu](https://github.com/wey-gu); Quick Terminal was introduced in PR
  [#135](https://github.com/nowledge-co/con-terminal/pull/135) by
  [@sundy-li](https://github.com/sundy-li))_

### Fixed

**Tabs and Panes**

- Preserved user-assigned tab accent colors across inactive tabs, vertical tab
  entries, and split-pane title bars at lower emphasis, and shortened pane
  titles to the final path component on Unix and Windows-style paths. _(PR
  [#168](https://github.com/nowledge-co/con-terminal/pull/168) by
  [@sundy-li](https://github.com/sundy-li))_

## `v0.1.0-beta.65` - 2026-05-07

### Added

**Tabs**

- Added tab context actions for Rename, Duplicate Tab, Close Tab, Close Other
  Tabs, Close Tabs to the Right, and per-tab accent colors across the horizontal
  tab strip and vertical sidebar. Duplicate Tab now preserves the full pane
  layout and each pane's working directory. _(PR
  [#162](https://github.com/nowledge-co/con-terminal/pull/162) by
  [@sundy-li](https://github.com/sundy-li))_
- Improved horizontal and vertical tab drag reordering with live drop previews
  and safer bounds handling, so fast drags clear stale drop targets instead of
  accidentally reordering after the cursor leaves the tab area. _(PR
  [#162](https://github.com/nowledge-co/con-terminal/pull/162) by
  [@sundy-li](https://github.com/sundy-li))_

### Changed

**Docs**

- Clarified official install paths versus community package-manager paths,
  including AUR `con-bin` maintainer credit for `czyt`, Scoop bucket credit for
  `EFLKumo`, and screenshot placement across the public docs. _(PR
  [#164](https://github.com/nowledge-co/con-terminal/pull/164) by
  [@wey-gu](https://github.com/wey-gu))_

### Fixed

**macOS**

- Fixed the remaining Intel Mac live-resize flash by ensuring Con no longer
  forces a synchronous Ghostty Metal draw before AppKit has committed the
  hosted surface's final layer bounds for that resize frame. _(Issue
  [#70](https://github.com/nowledge-co/con-terminal/issues/70), PR
  [#163](https://github.com/nowledge-co/con-terminal/pull/163) by
  [@wey-gu](https://github.com/wey-gu))_

**Windows**

- Fixed continuously updating TUIs such as Codex, `top`, `watch`, and `btop`
  freezing until the next click or key press. The Windows renderer now drains
  completed staging frames under sustained output, caps fallback presentation at
  the display cadence, and schedules the final catch-up frame when output stops.
  _(PR [#116](https://github.com/nowledge-co/con-terminal/pull/116) by [@wey-gu](https://github.com/wey-gu))_

## `v0.1.0-beta.64` - 2026-05-07

### Added

**macOS**

- Enabled Ghostty SSH shell-integration features for macOS terminal sessions,
  so remote SSH hosts can receive `xterm-ghostty` terminfo and terminal
  environment hints while Con still preserves its own cursor-style setting. _(PR [#159](https://github.com/nowledge-co/con-terminal/pull/159) by [@chenghuzi](https://github.com/chenghuzi))_

**Panes**

- Added pane title bars for split layouts, with direct close and fullscreen
  controls on each pane. _(PR [#149](https://github.com/nowledge-co/con-terminal/pull/149) by [@sundy-li](https://github.com/sundy-li))_
- Added pane title-bar dragging. Drag a pane title to rearrange split panes, or
  drop it into the tab strip to promote that pane into its own tab. _(PR [#149](https://github.com/nowledge-co/con-terminal/pull/149) by [@sundy-li](https://github.com/sundy-li))_
- Added an Appearance setting to hide pane title bars when you want the
  sparsest terminal-only layout. _(PR [#149](https://github.com/nowledge-co/con-terminal/pull/149) by [@sundy-li](https://github.com/sundy-li))_

### Fixed

**macOS**

- Fixed embedded Ghostty surface scale sync when moving a window between Retina
  and non-Retina displays. Existing panes now update display id, backing scale,
  layer scale, and pixel size together instead of keeping stale cell metrics. _(PR [#150](https://github.com/nowledge-co/con-terminal/pull/150) by [@chenghuzi](https://github.com/chenghuzi))_
- Fixed a live-resize flash regression on Intel Macs by committing embedded
  Ghostty backing size before moving the AppKit surface hierarchy, avoiding a
  one-frame mismatch between the Metal surface and terminal framebuffer. _(PR [#152](https://github.com/nowledge-co/con-terminal/pull/152) by [@wey-gu](https://github.com/wey-gu))_

**Windows, Linux**

- Fixed terminal `Ctrl+<punctuation>` C0 chords such as `Ctrl+]`, `Ctrl+/`,
  `Ctrl+@`, and `Ctrl+Space`, so tmux prefixes like `set -g prefix C-]` now
  send the expected control byte instead of being dropped or typed literally.
  The surface control API also accepts legacy `ctrl-2..8` aliases for
  automation, while interactive `Ctrl+1..9` remains reserved for tab switching
  on Windows and Linux. _(PR [#156](https://github.com/nowledge-co/con-terminal/pull/156) by [@wey-gu](https://github.com/wey-gu))_

**Panes**

- Fixed pane-drag preview cleanup so canceled or completed drags do not leave a
  stale tab-promotion preview behind. _(PR [#149](https://github.com/nowledge-co/con-terminal/pull/149) by [@sundy-li](https://github.com/sundy-li))_
- Fixed pane reordering so only true direct-sibling no-op moves are ignored;
  cross-subtree moves now land in the requested split position. _(PR [#149](https://github.com/nowledge-co/con-terminal/pull/149) by [@sundy-li](https://github.com/sundy-li))_

**Developer Experience**

- Made the portable CI Zig installer retry against Con's mirror if the primary
  Zig download fails. _(PR [#149](https://github.com/nowledge-co/con-terminal/pull/149) by [@sundy-li](https://github.com/sundy-li))_

## `v0.1.0-beta.63` - 2026-05-05

### Added

**macOS**

- Added **Quick Terminal** to the command palette on macOS, matching the View
  menu entry so it can be opened while Con is frontmost even when the global
  hotkey is disabled. _(PR [#146](https://github.com/nowledge-co/con-terminal/pull/146) by [@nowledge-co](https://github.com/nowledge-co))_

### Fixed

**macOS**

- Fixed pane-local surface geometry drift that could make TUI output appear
  clipped after switching between surfaces or changing pane layouts. Activating
  a surface now revalidates Ghostty's embedded terminal size against the
  current pane before exposing it. _(PR [#146](https://github.com/nowledge-co/con-terminal/pull/146) by [@nowledge-co](https://github.com/nowledge-co))_

**Linux**

- Fixed Linux preview terminal rows wrapping like prose in split panes. Rows now
  stay on one fixed terminal line and clip at the pane edge, which prevents TUI
  layouts from reflowing into later rows. _(PR [#146](https://github.com/nowledge-co/con-terminal/pull/146) by [@nowledge-co](https://github.com/nowledge-co))_

### Changed

**Developer Experience**

- Added low-noise macOS surface geometry diagnostics behind
  `CON_GHOSTTY_PROFILE`, so reproduced pane/surface clipping reports can include
  a log file and `con-cli surfaces list` snapshot without requiring a special
  debug build. _(PR [#146](https://github.com/nowledge-co/con-terminal/pull/146) by [@nowledge-co](https://github.com/nowledge-co))_

## `v0.1.0-beta.62` - 2026-05-05

### Added

**macOS**

- Added Quick Terminal, an optional macOS-only floating terminal window that
  slides down from the active screen, keeps its live tabs/panes while hidden,
  and can return focus to the app you were using before it appeared. It is off
  by default; enable it in Settings -> Keys and use Cmd-Backslash or your chosen
  shortcut.
- Added public Quick Terminal documentation covering setup, hide/show behavior,
  live state, destruction, and the difference from Summon / Hide Con.
- Added **New Window** to the macOS Dock menu, so right-clicking the Dock icon
  can open a fresh Con window without first focusing an existing one.

**Developer Experience**

- Added a cross-platform `justfile` for common build, run, test, release,
  install, and cleanup flows. The default recipes now dispatch through the
  Windows-safe `cargo w*` aliases when needed while staying simple on macOS and
  Linux.
- Kept project-local `.con/workspace.toml` profiles commit-friendly. Con still
  treats private runtime state separately, while generated layout profiles can
  live with the project when a team wants to share them.

### Changed

**Settings**

- Shortcut recording now temporarily suspends macOS global hotkeys and restores
  the previously active persisted shortcuts when recording is canceled, saved,
  or dismissed.

## `v0.1.0-beta.61` - 2026-05-05

### Added

**Tabs**

- Added inline rename for horizontal tabs. Double-click a tab title to edit it
  in place, with focus-time select-all, Enter/blur save, and Escape cancel.
- Added browser-style drag reorder for horizontal tabs, including left/right
  drop slots and a real trailing drop target after the last tab.

**Distribution**

- Bundled `con-cli` with every release artifact. macOS now ships it inside the
  app bundle and exposes it through Homebrew/script installs; Linux tarballs
  install both `con` and `con-cli`; Windows ZIP/script installs include
  `con-app.exe` and `con-cli.exe`.
- Added a conservative macOS launch-time self-heal for `~/.local/bin/con-cli`
  so manual-DMG installs and Sparkle-updated app bundles converge to the same
  CLI availability as installer/Homebrew installs without overwriting
  user-managed binaries.
- Added release verification for the macOS app bundle so a signed/notarized
  build cannot ship without the control-plane CLI.
- Added blocking release gates for installer/update safety. macOS and Linux
  release jobs now verify artifact layout before upload; Windows verifies the
  ZIP contains both `con-app.exe` and `con-cli.exe`; the finalizer refuses to
  publish a draft unless all expected assets, appcasts, and gh-pages installer
  scripts are present and point at the same tag.
- Tightened those release gates after review: appcasts are now parsed as XML,
  each macOS architecture publishes its own checksum asset, the finalizer runs
  from the tagged revision, and the macOS CLI shim ignores transient DMG/test
  app bundles.
- Hardened internal `v*-dev.*` release behavior so dev smoke tags are scoped to
  dev app names/bundle ids, never embed/update stable/beta appcasts, and never
  update Homebrew casks while the final gate still validates their artifact
  shape.
- Aligned the Linux runtime app id, desktop entry filename, and
  `StartupWMClass` as `co.nowledge.con` so Wayland and X11 launchers can group
  running Con windows with the installed app entry.
- Made the release finalizer sync hosted installer scripts from the tagged
  commit before promotion, so dev smoke tags can test the real `install.sh` /
  `install.ps1` path without moving beta/stable appcasts or Homebrew casks.
- Fixed macOS release signing order so the bundled `con-cli` executable is
  signed before the main app executable and notarized DMGs are not blocked by
  unsigned nested code.
- Documented that `con-cli` is part of the normal install path for surface
  orchestrators such as `pi-interactive-subagents`.

### Changed

**Settings**

- Settings header now shows last-saved time — "Saved just now", "Saved Xm ago", or "Saved Xh ago" with a check-circle icon after a successful save. The timestamp is seeded from `config.toml`'s modification time on init so the indicator persists across settings window reopens.
- Added `cmd-s` (macOS) / `ctrl-s` (Windows/Linux) keybinding to trigger Save Changes from anywhere in the settings panel.
- Added `cmd-w` (macOS) / `ctrl-w` (Windows/Linux) to close the standalone settings window or save-and-dismiss in panel mode. Closing via `cmd-w` now correctly reverts any unsaved standalone preview changes, matching the existing Escape path.
- Save Changes button in the standalone settings window now shows a `⌘ S` / `Ctrl S` keycap hint.

### Fixed

**Tabs**

- Made sidebar and horizontal tab rename lifecycles consistent: unchanged
  rename blur restores terminal focus without pinning smart AI/SSH/CWD labels,
  while real edits still persist as explicit user names.
- Prevented Escape-cancelled rename editors from committing through delayed blur
  events, even if the user immediately reopens rename on the same tab.
- Kept macOS titlebar dragging from stealing tab drag/click interactions while
  preserving the normal double-click titlebar behavior.

## `v0.1.0-beta.57` - 2026-05-03

### Added

**Restorable Workspaces**

- Added the first restorable-workspace implementation slice. Private session layout now round-trips every pane-local surface, including surface id, title, owner, cwd, active surface, and close-pane-when-last policy, instead of restoring only the active surface in each pane.
- Added the first private screen-text continuity slice. Con now snapshots bounded recent terminal text per pane-local surface and seeds it back through the terminal parser before the shell starts on macOS, Windows, and Linux, so restored text lives in terminal content instead of a UI overlay. This does not replay commands or export terminal text into workspace layouts.
- Added a typed, validated, layout-only `.con/workspace.toml` schema for future Con-generated export/import flows. The schema covers tabs, panes, surfaces, split geometry, cwd, and optional agent defaults, but deliberately excludes commands, conversations, history, scrollback, credentials, and trust decisions.
- Added the first layout-profile workflow. Use Command Palette or the Workspace menu to save the current window as a `.con/workspace.toml`, add a project folder/profile as tabs in the current window, or open a project folder/profile in a new window.
- Added project layout startup for explicit paths: `con <project-folder>` opens `<project-folder>/.con/workspace.toml` when it exists, while `con <workspace.toml>` opens that profile directly. Plain `con` still prefers private session restore.
- Documented the production restore model: local continuity and project-local memory come first, exported layouts are generated from user-tuned workspaces, and command/task files remain a separate future workflow.
- Documented the layout-profile "aha" flow and gesture semantics so users can distinguish automatic private restore, scratch windows, and explicit project layouts.
- Added a **Restore Terminal Text** privacy control in Settings -> General -> Continuity and a **Clear Restored Terminal History** Command Palette action that disables the feature and wipes saved restored text.

### Changed

**Startup**

- When a second Con process sees an already-live control endpoint, it now opens a fresh window session with shared history instead of cloning the last restored layout and agent conversation again. Full single-instance forwarding remains a follow-up under the app-state workspace model.
- Debug builds now use an isolated default control endpoint (`/tmp/con-debug.sock` on Unix and `\\.\pipe\con-debug` on Windows). This lets `cargo run -p con` coexist with an installed Con without suppressing the dev build's private workspace restore.

### Fixed

**Startup**

- Bounded the Unix and Windows control-endpoint probes used during second-process startup so a stale or wedged socket/pipe cannot hang Con while deciding whether to restore the saved workspace or open a fresh window.
- When `con <path>` cannot open the requested workspace profile, Con now opens a fresh shell with a visible terminal-layer error message instead of silently falling back to unrelated private restore state.
- Kept the workspace-profile error message visible even when restored terminal text continuity is disabled, without re-enabling private terminal-text restore for normal panes.

**Workspace Layout Profiles**

- Fixed cross-platform layout export so repo-relative `cwd` values are written with stable slash-separated paths on Windows, macOS, and Linux.
- Avoided capturing terminal text when exporting layout profiles, since exported profiles deliberately exclude runtime text history.
- Honored the **Restore Terminal Text** privacy setting when adding tabs from an imported layout profile, and prevented imported inactive native terminal views from staying visible behind the active tab.
- Kept terminal-text continuity default-on for existing beta users as well as new installs, while preserving the explicit Settings opt-out and **Clear Restored Terminal History** wipe action.
- Rejected edited layout profiles that define multiple panes without a layout tree, and made Con synthesize a safe split tree when exporting legacy private sessions that still have panes without layout metadata.
- Persisted shell cwd changes as soon as Ghostty reports OSC 7/PWD, so restored tabs, panes, and surfaces come back in the last reported directory instead of depending on a later unrelated save or graceful quit.
- Kept restored terminal text out of cwd decisions. Con now treats Ghostty's reported PWD as the authoritative path, avoiding stale restored prompt text overriding the live shell directory.

**Terminal, Windows and Linux Backends**

- Hardened split-pane dividers on Windows and Linux by giving the resize seam a real hit target while keeping the visible separator at one pixel. This avoids fragile overflow hit testing around terminal panes.
- Preserved restored terminal text on Linux when the IME system only prepares composition without actual marked text.

**Terminal, macOS**

- Made the embedded Ghostty initial-output restore hook fail soft for local best-effort builds while keeping macOS release packaging fail-hard, so upstream anchor drift blocks a release instead of silently shipping without terminal text seeding.
- Fixed cwd restore for macOS privacy-protected directories such as Documents and Downloads. Con's embedded Ghostty path now trusts the shell-integration cwd passed by the app instead of rejecting it during directory open/stat preflight, which macOS may deny even when the shell can `cd` there.

## `v0.1.0-beta.54` - 2026-05-02

### Added

**Control Plane**

- Added human-facing rename for pane-local surfaces. Surface names can now be changed from Command Palette, the terminal context menu, the app menu, or by double-clicking a surface tab in the pane-local strip. This complements the existing `surfaces.rename` CLI/API path for orchestrators.
- Added configurable shortcuts for every human surface action: create in pane, split right/down, next/previous, rename, and close. These now show in menus, Command Palette, and Keyboard Shortcuts.
- Polished pane-local surface chrome so humans can distinguish panes from surfaces: owned or named single-surface panes now show an in-pane surface rail, surface tabs use stable `Surface N` fallback names instead of duplicate terminal cwd titles, rename edits inline in the rail, and tabs expose close plus right-click Rename/Close actions.
- Clarified the human surface model in menus and docs: surfaces are tab-like sessions inside one pane, while surface split commands create a new visible pane first. The terminal context menu now also includes Settings for direct access.
- Refined the in-pane surface rail from a full-width header into compact local chrome, so a pane with multiple surfaces no longer looks like another nested pane split.

### Changed

**Command Palette**

- Normalized the command palette to Con's current design language: system UI typography, quieter selected-row treatment, a softer search well, and cleaner shortcut alignment.
- Rendered shortcuts as separate keycaps in the Command Palette and terminal context menu, with platform-aware labels on macOS, Windows, and Linux.

### Fixed

**Control Plane**

- Kept inactive pane-local surfaces sized to their host pane while they are hidden. TUI coding agents launched in background surfaces now receive the same terminal rows/columns they will have when focused, avoiding incorrect layout assumptions in multi-surface orchestrator workflows. Fixes [#108](https://github.com/nowledge-co/con-terminal/issues/108).
- Fixed a crash when committing an inline surface rename from the pane-local surface rail.

**Terminal, macOS**

- Fixed Clear Terminal from the app menu and Command Palette by passing the Ghostty binding action name correctly to the embedded terminal.

**Terminal, Windows and Linux Backends**

- Wired Clear Terminal for the preview backends so the shared menu and Command Palette action clears the local VT screen and scrollback there as well.

**Input Bar**

- Made AI command suggestions more tolerant of provider latency: if a completion arrives after the user has typed further into the same suggested command, Con now still shows the remaining ghost text instead of dropping the result as stale.

## `v0.1.0-beta.53` - 2026-05-02

### Fixed

**Terminal, macOS**

- Restored terminal glass after the macOS seam-leak hardening work. Modern macOS now lets Ghostty own terminal opacity/blur again instead of drawing a second translucent AppKit backing behind the Metal surface, while legacy macOS keeps its opaque fallback.
- Restored configured UI chrome opacity for the tab bar, input bar, side bar, and agent panel. Opaque terminal-colored mattes now stay limited to seam guards; the native full-window transition underlay is disabled whenever terminal glass is active so hide/show animations no longer make the terminal temporarily opaque. Modern macOS now also uses GPUI's native blurred window backdrop when terminal blur is enabled, and terminal-adjacent bottom bar, agent panel, top tab strip, and vertical-tabs geometry snap with short reservation/release guards so native Ghostty views are not exposed before AppKit has resized them.

## `v0.1.0-beta.52` - 2026-05-02

### Added

**Control Plane**

- Added Command Palette entries for pane-local terminal surfaces. Users can now create the first surface as a visible split, create additional surfaces inside the focused pane, cycle between surfaces in that pane, and close the current surface without reaching for `con-cli`.
- Added a terminal right-click context menu across macOS, Windows, and Linux. It exposes paste/copy/clear, pane split and zoom controls, pane-local surface controls, Focus Input, and Command Palette through the same action system as keybindings and the app menu.

## `v0.1.0-beta.50` - 2026-05-01

**Terminal, macOS**

- Now zoom window can be done double-click on titlebar
- Zoomed window fixed buttom one pixel spacing leaked

### Fixed

## `v0.1.0-beta.49` - 2026-05-01

### Fixed

**Terminal, macOS**

- Fixed macOS window zoom/fullscreen behavior so Con no longer leaves a bottom gap from terminal cell-sized AppKit resize increments, and restored double-click titlebar zoom behavior on the in-app tab bar.
- Improved macOS Monterey fallback for the embedded Ghostty terminal. On macOS 12 and older, Con now explicitly keeps Ghostty's hosted IOSurface layer geometry synchronized with the native surface view, addressing reports where old macOS showed an opaque black terminal area but no visible output. Modern macOS keeps Ghostty's existing layer ownership unchanged. Tracks [#20](https://github.com/nowledge-co/con-terminal/issues/20).

## `v0.1.0-beta.48` - 2026-05-01

### Fixed

**Terminal, Linux Backend (preview)**

- Fixed Linux release artifacts so `libghostty-vt` is built for a generic Zig target instead of the CI runner's native CPU. This avoids `Illegal instruction` startup crashes on WSL and older x86_64 hosts whose CPUs do not expose the same instruction-set extensions as the release builder. Fixes [#97](https://github.com/nowledge-co/con-terminal/issues/97).

**Agent Panel**

- Fixed inline LaTeX in markdown responses so `$...$` formulas are typeset through the same cached SVG pipeline as block math instead of being shown as raw formula text. Inline formulas now inherit the surrounding text color and sit inside the prose line box for cleaner spacing. Fixes [#88](https://github.com/nowledge-co/con-terminal/issues/88).

## `v0.1.0-beta.47` - 2026-05-01

### Added

**Terminal, Windows and Linux Backends**

- Added CJK IME text input for Windows and Linux terminal panes. IME commits now enter through GPUI's platform text-input path, preedit state tracks candidate positioning at the terminal cursor, and terminal-local control / alt / special key handling remains unchanged. The macOS embedded Ghostty path is unchanged.

**Workspace**

- Added a quick vertical-tabs toggle in the top bar, Command Palette, and Keyboard Shortcuts. Defaults: Cmd+B on macOS, Ctrl+Shift+B on Windows and Linux.
- Added draggable resizing for the pinned vertical-tabs panel. The resized width is persisted in session state and restored across launches.

**Control Plane**

- Added pane-local terminal surfaces for external orchestrators. Existing `panes.*` APIs, built-in agent tools, and benchmarks keep the active-surface pane contract, while new `tree.get` and `surfaces.*` RPC/CLI commands can create, split, wait for readiness, focus, rename, drive, read, and close terminal sessions inside a pane.

### Fixed

**Terminal, macOS**

- Reduced transparent-window flashes along moving chrome seams. Agent-panel transitions, input-bar transitions, the top-bar transition, the vertical-tabs edge, the input-bar edge, and pane dividers now use tiny opaque terminal-colored seam covers on macOS instead of exposing a transparent gap or UI-colored matte.
- Restored visible macOS terminal pane dividers without reopening the transparent-window leak path. Split separators are now a subtle opaque foreground tint precomposed over the terminal background, so users can read pane boundaries while every separator pixel still stays fully opaque.
- Polished the standalone Settings window's `Save Changes` action so it reads as compact header chrome: visible enough to find, but no longer a loud primary-blue slab.
- Further hardened macOS transparent-window composition by precomposing chrome surfaces over the terminal color and letting the native Ghostty host backing slightly overdraw under GPUI seams. Fast sidebar, agent-panel, input-bar, split, and zoom motion should no longer reveal bright desktop pixels through clear backing gaps.
- Stopped continuously reflowing the macOS terminal layout during the right agent-panel hide/show animation. The panel content still animates, but the terminal/panel boundary snaps to a stable geometry so fast toggles do not expose clear backing between GPUI and the native Ghostty view.
- Added a temporary native underlay below visible macOS Ghostty surfaces during chrome transitions. It catches clear-window backing during rapid input-bar, agent-panel, tab-strip, and vertical-tabs toggles without drawing a matte over terminal text.

**Control Plane**

- Hardened pane-local surface lifecycle behavior so owned ephemeral worker panes can close when their final surface closes, inactive surfaces keep correct pane membership, replacement surfaces are materialized before focus moves to them, and `surfaces.wait_ready` only reports `ready` once the terminal is live with shell integration.

## `v0.1.0-beta.46` - 2026-04-30

### Added

**Agent Providers**

- Updated Rig to 0.36.0 and added DeepSeek V4 model support. DeepSeek now defaults to `deepseek-v4-flash`, keeps `deepseek-v4-pro` available in the model picker, and preserves the legacy `deepseek-chat` and `deepseek-reasoner` aliases.

**Settings**

- Settings now opens as a separate window. You can adjust appearance, shortcuts, and provider configuration, save changes, and keep the settings window open while checking the terminal.
- Appearance controls now preview immediately while Settings stays open. Transparency, blur, background image strength, image layout, terminal font, UI font, and cursor style update live; Save Changes persists the current values.
- Polished the Settings save action so it uses Con's active theme colors instead of the generic primary button treatment.
- OpenAI-compatible providers can now fetch available models from the provider's `/models` endpoint when a Base URL and API key are configured.

**Keyboard**

- Added direct tab selection shortcuts: Cmd+1 through Cmd+9 on macOS, Ctrl+1 through Ctrl+9 on Windows and Linux.
- Added pane zoom for the focused split pane. Use Cmd+Shift+Enter on macOS or Alt+Shift+Enter on Windows and Linux to let one pane fill the tab's terminal area, then press it again to restore the split layout.
- Added macOS window cycling with Cmd+Backtick and Cmd+Shift+Backtick.
- Fixed macOS Cmd+Backtick window cycling when the terminal surface has focus.
- Fixed macOS Cmd+Backtick window cycling at the native window-event layer so it works even when the embedded terminal NSView is first responder.
- Fixed macOS Cmd+Backtick window cycling to also handle GPUI's shifted and localized symbol forms (`~`, `<`, `>`), matching the platform's keyboard-layout-aware shortcut behavior.
- Pane picker shortcuts are now scoped to the picker: open it with the configured pane-scope shortcut, then use bare 1-9 to toggle panes, A for all panes, and F for focused pane. A/F are consumed before the input bar sees them, and global Cmd/Ctrl+1-9 remains reserved for tab switching outside the picker.
- Fixed closing the last pane in one window so it closes only that window instead of quitting Con and killing sibling windows.

### Fixed

**Terminal, macOS**

- Fixed fast trackpad scrolling in macOS terminal panes so precise scroll events are sent through Ghostty's precision-scroll path instead of being treated as coarse wheel ticks.
- Reduced macOS terminal scroll-path overhead by syncing the native Ghostty scroll container only for visible tab surfaces while still draining background-tab title and process events.
- Fixed macOS split, nested split, zoom, and unzoom operations that could leave a blank or transparent pane region until the divider was manually resized.

**Workspace**

- Fixed the bottom input bar layout so it spans only the terminal area, staying out of the vertical tab sidebar and right agent panel.

**Settings**

- Fixed Settings live preview dismissal so unsaved appearance and theme changes are rolled back when the standalone Settings window is closed.
- Fixed OpenAI-compatible model discovery so fetched model lists are scoped to the configured Base URL instead of leaking across custom endpoints.
- Fixed OpenAI-compatible model discovery so newly fetched models immediately refresh all related Settings pickers, including active and suggestion model selectors.
- Fixed OpenAI-compatible model discovery for Base URLs with required query parameters, preserving the query while deriving the `/models` endpoint.
- Clarified OpenAI-compatible provider setup when `/models` is unavailable: fetching is optional, and users can type the model ID manually.
- Fixed standalone Settings cleanup so closing the last workspace window also closes Settings on Windows and Linux instead of leaving an orphaned process.
- Hardened OpenAI-compatible model discovery URL normalization so incomplete `/chat/completions` paths fail clearly instead of becoming relative `/models` URLs.

**Keyboard**

- Fixed direct tab selection shortcuts so terminal panes hand Cmd/Ctrl+1-9 back to the app instead of forwarding them to the shell.
- Fixed direct tab selection shortcuts so Cmd/Ctrl+1-9 keeps switching tabs while the pane picker is open; pane selection remains on bare 1-9 inside the picker.

**Windowing**

- Fixed new Con windows opening exactly on top of the previous one. New workspace windows now cascade from the active window while staying within the visible display area when display bounds are available, and still cascade when the platform cannot report bounds.
- Fixed new-window cascade wrapping so the 28px stagger is preserved on the axis that still fits when the other axis wraps to the display edge.
- Unified macOS Window menu cycling with the same native AppKit path used by Cmd+Backtick, so menu actions and keyboard shortcuts share one ordering model.

**Terminal, Windows Backend (preview)**

- Fixed Windows terminal text rendering to avoid RGB color fringing around glyphs. The DirectWrite atlas now uses grayscale antialiasing with neutral coverage compositing, making CJK and mono text look cleaner in screenshots, scaled displays, remote review, and transparent windows.

## `v0.1.0-beta.44` - 2026-04-29

### Added

**Terminal**

- Added terminal file-drop and file-path clipboard paste. Dropped files now paste quoted paths into the active terminal pane instead of being ignored.
- Added image clipboard forwarding for TUI agents such as Codex. When the clipboard contains image bytes, Con forwards a native Ctrl+V keypress so the TUI can attach the image through its own OS-clipboard workflow.
- Added conservative file-URI clipboard parsing so Linux file-manager copies exposed as `text/uri-list` paste as quoted file paths instead of raw `file://` text.
- Hardened terminal Copy/Paste action handling on Windows and Linux so menu/command-dispatched paste uses the same path as the terminal keyboard shortcut.

## `v0.1.0-beta.43` - 2026-04-28

### Added

**Terminal**

- Added terminal URL opening by modifier-clicking visible links: Cmd-click on macOS, Ctrl-click on Windows and Linux. macOS uses libghostty's native link action, while Windows and Linux use a bounded visible-row detector with pointer-cursor feedback so ordinary terminal rendering remains off the hot path.

### Fixed

**Agent Panel**

- Fixed generic Markdown code fences that contain shell prompts, including compact prompt forms like `$amp --version`, so they infer bash highlighting instead of rendering as unhighlighted `code` blocks.

**Terminal**

- Fixed terminal font selection accepting GPUI-only pseudo font families such as `.ZedMono`, which could make macOS Ghostty text render with uneven spacing or overlapping glyphs. Existing bad configs now fall back to the default terminal font, and the Terminal Font picker hides those pseudo families.

## `v0.1.0-beta.42` - 2026-04-27

### Fixed

**Terminal, Windows Backend (preview)**

- Fixed Windows terminal scroll direction so wheel and touchpad gestures follow the platform scroll intent instead of feeling reversed against classic scrolling setups.
- Fixed Tab and Shift+Tab in the Windows terminal so shells and TUIs receive completion/navigation keys instead of GPUI focus navigation swallowing them.
- Fixed Windows terminal scrollbar rendering to cache libghostty-vt scrollbar state by terminal generation instead of polling the expensive scrollbar query during every paint.
- Fixed Windows terminal scrollbar dragging so stale drag state is cleared if the mouse is released outside the terminal pane.

**Terminal, Linux Backend (preview)**

- Fixed Tab and Shift+Tab in the Linux terminal preview with the same terminal-local key capture used on macOS and Windows, preventing GPUI focus navigation from swallowing shell completion keys.

## `v0.1.0-beta.41` - 2026-04-27

### Fixed

**Terminal, Windows Backend (preview)**

- Fixed custom terminal fonts on Windows resolving through the wrong DirectWrite collection, which could make glyphs appear incomplete or spaced apart when a system font such as JetBrains Maple Mono was selected.
- Fixed Windows fallback rendering for CJK and missing default fonts by reserving Unicode-derived two-cell atlas slots for wide glyphs and falling back to installed monospace system fonts before proportional UI fonts.
- Fixed new Windows terminals starting in con's install directory. New panes now honor a valid explicit cwd and otherwise start from the user's home directory instead of inheriting the launcher directory.
- Fixed Windows terminal scrollback controls by adding a visible draggable scrollbar and wiring wheel gestures to libghostty-vt's viewport scrollback path when the shell has not requested mouse tracking.

------

## `v0.1.0-beta.40` - 2026-04-26

### Added

**Agent Panel**
- Added first-class rich Markdown blocks for Mermaid diagrams and LaTeX-style math. Mermaid code fences and display math now render off the UI thread into cached GPUI images with source fallbacks, while inline math uses dedicated math typography instead of being flattened into generic code.
- Mermaid diagrams now render with light/dark-aware colors so diagrams remain readable in both light and dark themes.

**Keyboard**
- Changed the input focus shortcut into a true Input / Terminal toggle. On macOS, Cmd+I now switches from the terminal to the input surface, then back to the first terminal pane when pressed from the input bar or agent-panel input.

### Fixed

**Agent Panel**
- Fixed Mermaid and display math inside nested Markdown containers such as blockquotes and lists so they use the same cached rich renderer as top-level blocks instead of falling back to source text.
- Fixed inline math detection so hyphenated prose and dollar ranges such as `$end-to-end$` or `$3-5$` are not misread as math.
- Fixed inline math rendering for identifier-style formulas such as `$theta$` and `$velocity$`.
- Fixed dark-theme Mermaid diagrams with custom light node fills so missing Mermaid `color:` styles are inferred from fill luminance instead of leaving white text on light shapes.
- Fixed display math SVGs in dark mode so formulas use theme-aware foreground colors and re-render correctly after theme switches.
- Fixed rich Mermaid and math render caching during streaming so replacing a parsed Markdown document with equivalent block content no longer discards already-rendered SVG images.
- Fixed repeated rich-render source-string allocation during ordinary repaint, reducing avoidable memory churn when scrolling chats with diagrams or formulas.
- Fixed display-math fallback styling so it uses the neutral block math style instead of inline-math chips inside an already styled block.
- Consolidated blockquote and list layout rendering for cached rich blocks and fallback Markdown blocks so future spacing and typography changes cannot drift between the two paths.

**Terminal**
- Fixed macOS IME English-mode commits in the terminal so direct ASCII input keeps the normal shell key path and does not disable shell ghost suggestions, while marked CJK composition still uses the IME text path. Buffered ASCII commits are now replayed as per-character key events instead of one synthetic multi-character key.

**Keyboard**
- Fixed the Input / Terminal toggle fallback so Cmd+I never focuses a hidden input surface. If neither the agent-panel input nor the bottom input bar is visible, Cmd+I now reveals and focuses the bottom input bar.

## `v0.1.0-beta.39` - 2026-04-26

### Added

**Workspace — Vertical Tabs**
- Added a vertical-tabs layout for the workspace tab strip. Toggle in *Settings → Appearance → Tabs → Vertical Tabs*.
- Two runtime states:
  - **Collapsed (rail)** — narrow icon rail (~44 px). Smart icon per tab. Hovering an icon pops a small **floating tab card** anchored to the cursor (name, optional subtitle, pane count, SSH / unread badges). The card is informational only — it never displaces the rail or the terminal pane and dismisses the moment the cursor leaves the icon. Drag an icon directly to reorder.
  - **Pinned panel** — full panel (~240 px) with two-line rows (name in system font, optional cwd / `user@host` subtitle in mono). Persisted across restart via `session.vertical_tabs_pinned`.
- Smart per-tab naming. Priority: **user override → AI summary → SSH host → focused process** (parsed from the OSC-set terminal title — `vim README.md`, `htop`, `less log.txt`, etc.) **→ cwd basename → shell**. Bare shells fall through so a row never reads as "bash" when there's something more useful.
- AI labels and icons via a new `TabSummaryEngine` in `con-core`. Each tab's `(cwd, recent commands, OSC title, recent terminal scrollback)` is summarized by the user's already-configured suggestion model into a 1–3 word label and one icon from a closed Phosphor set: `terminal`, `code`, `pulse`, `book-open`, `file-code`, `globe`.
- The AI summarizer uses a JSON response shape (`{"label": "...", "icon": "..."}`) with a tolerant bracket-balanced parser instead of the original `LABEL|ICON` plain-text format, so reasoning models and Markdown-fenced answers are handled without rendering malformed labels.
- Periodic AI re-summarization now polls `request_tab_summaries` every 3 s. The engine stays gated on `agent.suggestion_model.enabled`, short-circuits SSH tabs to host + globe with no LLM call, and uses a per-tab context cache plus 5 s success budget so re-asks stay bounded.
- Smart per-tab icons keyed off the focused process when no AI is available: `terminal` for shells, `globe` for SSH, `</>` for editors (vim/nvim/nano/emacs/helix/code), `pulse` for monitors (htop/top/btop/k9s), `book-open` for pagers (less/more/man/bat), `file-code` for git tools (git/lazygit/tig/gh). User-labelled tabs still get a smart icon picked from the live process / SSH signal — or from the AI summary when available.
- No emoji anywhere in the panel — every glyph is a Phosphor SVG.
- Long paths collapse to `…/parent/last`; `$HOME` collapses to `~`.
- Inline rename: double-click a row's label, or right-click → Rename. Enter commits, Esc cancels. The label is persisted via a new `user_label` field on `TabState`. Reset Name in the context menu clears the override and falls back to smart auto-naming.
- Right-click context menu per row: Rename / Duplicate / Reset Name (if user-labelled) / Move Up / Move Down / Close Tab / Close Other Tabs.
- Drag-to-reorder works in both rail and pinned modes. The dragged tab follows the cursor as a small floating chip; a 2-px primary-color line marks the drop target between rows in pinned mode; the rail uses a hover-bg pill to signal the drop target. Drop fires `SidebarReorder`, which the workspace applies in-place and persists.
- Visual hierarchy follows the design language: a single selection signal (elevated white pill — no accent bar duplication), action affordances (rename pencil, close X) hover-only on every row including the active one, surface separation via opacity blending (no borders, no shadows), system font for labels (terminal-chrome consistency reserved for the row icon and subtitle).
- Width tweens 220 ms ease-out cubic between rail and pinned; floating card appears instantly (Apple tooltip pattern, no transition).
- Pinned/collapsed state and the orientation choice persist across restarts (`vertical_tabs_pinned` in session, `appearance.tabs_orientation` in config). Horizontal tabs remain the default for backward compatibility with every shipped beta. Switching orientation at runtime takes effect immediately.

### Improved

**Agent Panel**
- Reworked long restored agent replies to cache rendered Markdown by block, so typing, scrolling, and revealing more content no longer recreate every paragraph/table/code layout inside a large assistant response.
- Fixed wide Markdown table scrolling so vertical transcript scroll no longer gets hijacked into sideways table movement, while preserving a structured rich-table layout with a native horizontal scrollbar for oversized tables.
- Added more deliberate transcript gutters so user and assistant messages no longer collide with the panel edge or scrollbar.
- Normalized con's embedded mono font family for GPUI-rendered code blocks and terminal chrome so Markdown code uses the intended IoskeleyMono face instead of platform font fallback.
- Hardened restored-session Markdown caching so stale assistant views are cleared when switching or clearing conversations, and in-flight Markdown parses cannot attach to the wrong message after reruns or state swaps.

**Terminal — Windows Backend (preview)**
- Made Windows command rendering feel substantially closer to native terminals by closing the full PTY-to-paint loop: ConPTY output wakes are preserved, stale readbacks are discarded with mailbox semantics, row-local D3D readbacks stay row-local through GPUI image handoff, and delayed command-start output remains latency-critical long enough to present fresh frames.
- Aligned the default Windows shell choice with Windows Terminal when possible by reading its configured default profile before falling back to `pwsh.exe`, `powershell.exe`, `%COMSPEC%`, and `cmd.exe`; `CON_SHELL` remains the explicit override.
- Added `CON_LOG_FILE` so Windows terminal profiling can write con's own logs directly to a file without PowerShell `*>` redirection, avoiding redirected standard handles that can steal shell output away from the ConPTY pane.
- Fixed Windows ConPTY child process creation so shells do not inherit con's redirected stdout/stderr handles during profiling, which could send the PowerShell banner/prompt into `con-profile.log` instead of the terminal pane.
- Reduced redundant Windows pre-echo repaint work by letting handled keyboard input wait for actual VT/ConPTY progress instead of forcing a speculative GPUI repaint on every key press.
- Reduced Windows output batching by preserving successive ConPTY wake signals instead of draining them into a single repaint request before GPUI has had a chance to present intermediate progress.
- Reduced Windows slideshow-style command redraws by treating the staging ring as a true mailbox during PTY-driven output: once a fresher VT snapshot is submitted, older completed readbacks are no longer presented ahead of it.
- Tightened Windows staging mailbox behavior further so command bursts discard older in-flight readback cache entries and present the newest completed frame instead of replaying intermediate full-screen snapshots.
- Extended Windows interactive low-latency mode long enough to cover delayed command-start output, so shells and TUIs that begin painting a few hundred milliseconds after Enter can still take the freshest-frame path instead of falling back to delayed non-blocking readback.
- Reduced Windows input/readback latency by deriving exact changed VT rows even when libghostty-vt reports a full render-state dirty flag, then copying only those pixel rows from the D3D render target into the staging readback texture for small terminal updates.
- Reduced Windows readback cost with dirty-row D3D copies while preserving translucent-terminal correctness by replacing dirty rows in a CPU backing frame instead of alpha-blending row overlays over stale pixels.
- Expanded Windows profiling behind `CON_GHOSTTY_PROFILE` so one run now captures ConPTY read-chunk cadence, renderer sub-stage timings (drain/draw/submit/block-drain), `RenderSession::render_frame`, and the GPUI image-wrap stage alongside the shared VT snapshot timings; idle unchanged frames are filtered by default, with `CON_GHOSTTY_PROFILE_VERBOSE=1` available for every-frame traces.

## `v0.1.0-beta.38` - 2026-04-24

### Fixed

**Interface**
- Completed pane-close keyboard handling instead of only showing dead UI. `Close Pane` is now a real configurable shortcut in Settings, with app-level defaults that do not collide with terminal EOF (`Cmd+Opt+W` on macOS, `Alt+Shift+W` on Windows/Linux). Windows/Linux pane split defaults now use `Alt+D` / `Alt+Shift+D` instead of fragile symbol-based bindings.
- `Close Pane` now escalates cleanly when nothing smaller remains to close: last pane closes the tab, and the last tab quits the app.
- Fixed the last-pane quit path so quitting through `Close Pane` or terminal-exit escalation uses the same session save and surface teardown path as the normal app quit action.
- Fixed the command palette scroll behavior on all platforms. Mouse-wheel scrolling and scrollbar dragging no longer snap back to the selected row on every repaint.
- Reduced agent-panel stalls on session-open, long live responses, and oversized reply expansion by making message markdown and persisted reasoning parse lazily, rendering in-flight replies as cheap plain-text previews, parsing full reply markdown off the UI thread when opened, and caching expensive inline markdown text-run transforms so repeated rich-render passes do less work.
- Reduced severe hangs when expanding long real-world agent replies in markdown. Inline code inside long prose/table cells now renders through single `StyledText` runs instead of exploding into large flex-wrapped chip trees.
- Reduced another major markdown-render cost for long agent replies. Assistant message bodies now live behind their own markdown-document entities, and fenced code blocks render as one highlighted text layout per block instead of one GPUI row per line.
- Reduced agent-panel rerender churn further by isolating whole assistant rows behind their own entities. Old restored sessions and live token streaming no longer force the parent panel loop to rebuild every assistant message row on each update.
- Moved the main agent transcript off a single flex-column scroll surface and onto GPUI's variable-height `ListState` path, so old long sessions no longer require the whole conversation tree to be measured and repainted as one document during scroll.
- Reduced restored-session interaction lag by stopping ordinary composer keystrokes from notifying the whole agent panel, keeping long restored markdown from hydrating in the background, and rendering agent-panel inputs with the terminal mono font.

**Terminal — Windows Backend (preview)**
- Reduced the first-frame flash when splitting a new pane on Windows. Brand-new panes now paint their configured terminal background while the first renderer image is still warming up, instead of briefly showing a transparent hole.

## `v0.1.0-beta.37` - 2026-04-24

### Improved

**Terminal — Linux Backend (preview)**
- Reduced Linux preview renderer CPU cost by caching per-row `StyledText` text/runs in `linux_view` and only rebuilding rows marked dirty by the VT snapshot, plus cursor-affected rows.
- Reduced Linux command-to-paint latency by waking the terminal view directly from PTY output instead of waiting for the workspace idle poll loop to discover new output.
- Added shared VT snapshot timing instrumentation behind `CON_GHOSTTY_PROFILE` so large command-start redraw costs on Windows/Linux can be measured directly.
- Documented the remaining Linux performance constraint explicitly: after the row-cache and direct-wake changes, the longer-term fix remains the planned glyph-atlas grid renderer and a lighter-weight shared VT snapshot contract.

### Fixed

**Release pipeline — multi-platform race**
- Fixed `irm https://nowled.ge/con-ps1 | iex` (and the `install.sh` equivalents) failing with `✗ no ZIP found for windows-x86_64` when the Linux release job won the create-and-upload race ahead of the Windows / macOS jobs. Each `release-{linux,macos,windows}.yml` now creates the GitHub release as `--draft`, so `/releases/latest` and the public REST API stay blind to the tag while assets are uploading. A new `release-finalize.yml` workflow watches all three platform workflows via `workflow_run` and atomically promotes the draft to public only once every platform reports a `success` conclusion for the same `head_sha`. If a platform fails, the draft stays drafted on purpose — better to block than to ship an incomplete release.

## `v0.1.0-beta.36` - 2026-04-24

### Fixed

- Windows con no longer be laggy!!!

## `v0.1.0-beta.35` - 2026-04-23

### Fixed

- Fixed Linux con cannot support older GLIBC

## `v0.1.0-beta.34` - 2026-04-23

### Added

**Terminal — Linux Backend (preview)**
- Added the first real Linux terminal pane: Unix PTY, shared `libghostty-vt` parser, GPUI styled-cell paint path, and the same app shell/control socket as macOS and Windows.
- Added Linux styled-cell rendering for SGR colors, bold, italic, underline, strikethrough, inverse, and block cursor state.
- Added Linux theme propagation from app settings into the VT parser at spawn time and during live theme changes.
- Added Linux client-side decorations, transparent rounded windows, and KWin Wayland backdrop blur where the compositor exposes it.

**Release and Packaging**
- Linux now has the same one-liner installer + auto-update story as macOS / Windows. The single `install.sh` (`curl -fsSL https://con-releases.nowledge.co/install.sh | sh`) detects the host OS and routes Darwin to the existing DMG flow or Linux to a new tarball flow that drops `con` into `~/.local/bin`, registers a `.desktop` launcher entry, and installs the 256x256 hicolor icon.
- A new `scripts/linux/release.sh` builds the artifact (`cargo build --release` with `CON_RELEASE_VERSION` / `CON_RELEASE_CHANNEL` baked in via `option_env!`, `--strip-debug`, stage + tar.gz + sha256), and a new `.github/workflows/release-linux.yml` mirrors the Windows release workflow: builds on `ubuntu-latest`, publishes the tarball to the same GitHub release the macOS / Windows jobs share, and updates the Sparkle-shaped appcast at `https://con-releases.nowledge.co/appcast/{channel}-linux-x86_64.xml` so the in-app notify-only updater can discover new builds.
- Added a notify-only Linux updater that polls the same Sparkle-shaped appcast XML the Windows backend uses, surfaces "Update now" in Settings → Updates, and re-runs `install.sh` with `CON_INSTALL_VERSION` pinned to the appcast's chosen version so beta-channel users do not silently get downgraded to stable when GitHub's `/releases/latest` skips prereleases. Sparkle itself stays macOS-only — Linux follows the Windows model (notify-only checker against the shared XML schema, then re-runs `install.sh` to apply).
- All three release workflows (`release-macos.yml`, `release-windows.yml`, `release-linux.yml`) now mark `v*-dev.*` tags as `--prerelease` when calling `gh release create`. Dev tags are internal smoke tests and must not appear in `/releases/latest`. Beta tags stay as regular releases for now because con is still in an all-betas era — the latest beta is what every fresh install should download. When stable v0.1.0 ships, `*-beta.*` will move under the same `--prerelease` rule so beta and stable channels stop colliding on `/releases/latest`.

### Improved

**Terminal — Linux Backend (preview)**
- Resolved IoskeleyMono font-family lookup on Linux so the embedded mono font is used instead of a proportional fallback.
- Reduced Linux terminal latency by replacing a full snapshot equality check with a generation-counter comparison and tightening the idle poll loop.

**Terminal — Windows Backend (preview)**
- Reduced Windows renderer hot-path work by skipping redundant default-background blank-cell instances, avoiding full-frame zero-fill before D3D readback, and only sorting the instance stream when a frame contains overflowing wide glyphs.
- Reduced Windows interactive terminal latency by marking user-driven updates latency-critical so the freshest submitted frame wins over stale readback, while keeping resize/fullscreen frames on the non-blocking staging path.

### Fixed

**Terminal — Linux Backend (preview)**
- Stopped the Linux "Waiting for shell prompt..." placeholder from flashing when long-lived shells enter alt-screen TUIs.

**Terminal — Windows Backend (preview)**
- Fixed Windows settings, theme, session, history, OAuth, conversation, and global skills paths so they no longer use `con` as a filesystem segment, avoiding Win32 reserved-device-name failures such as `os error 267`.
- Fixed Windows `con-ghostty` builds that failed in Ghostty/uucode with `uucode_build_tables.exe: FileNotFound` by defaulting Zig's global cache to a short path when `ZIG_GLOBAL_CACHE_DIR` is unset.
- Hardened Windows Zig cache selection so `con-ghostty` only auto-selects a short cache path when the chosen directory is actually writable, falling back cleanly instead of failing later inside Zig.
- Fixed Windows fullscreen/maximize redraws in alt-screen apps such as Neovim by forcing a full VT snapshot after resize/full invalidation instead of trusting per-row dirty flags for newly exposed rows.
- Fixed a Windows maximize/fullscreen hang where the first interactive redraw after a large resize could block the UI thread trying to rescue stale readback slots; the staging ring now uses mailbox semantics, stays non-blocking under true backlog, and still allows low-latency interactive presents when a clean slot is available.
- Fixed Windows resize/render-state drift by snapshotting Ghostty's actual render-state rows/cols during asynchronous resize catch-up and by including snapshot geometry in the renderer invalidation key so the corrected frame is not skipped as "unchanged".
- Fixed Windows VT resize to report Ghostty the renderer's real cell metrics instead of re-deriving per-cell pixels from pane size and grid count.
- Fixed a Windows input-latency hole where the low-latency hint could be consumed before ConPTY echo/prompt output advanced the VT; input-driven fast presents now stay armed until the next VT generation arrives.
- Fixed a Windows typing-latency cadence issue where sustained local input could fall back to stale-frame presents mid-burst; interactive low-latency mode now stays armed across a short typing/paste burst instead of only a single echoed generation.
- Fixed Windows terminal-pane bounds capture to measure a dedicated full-size wrapper during prepaint, avoiding zoom/maximize cases where sizing from image/layout children could stop the image quad short and hide the bottom command row.
- Reduced Windows steady-state input latency by moving terminal image generation out of the prepaint callback and into the main render path, removing an extra frame where freshly rendered images previously only became visible on the following paint.
- Reduced a Windows staging-ring latency edge where mailbox submission could miss an already-clean slot and unnecessarily disqualify the freshest-frame path during interactive typing.

**Interface**
- Fixed the macOS 12 fallback layering path. Monterey keeps an opaque top-level window to prevent desktop bleed-through, but the GPUI root above the embedded Ghostty `NSView` stays transparent so the terminal surface is not painted over by the fallback background.

### Documentation

- Updated the Linux implementation notes, tracker mirror, and postmortem for the first real Linux terminal preview.
- Updated build-system and Windows-port notes to document the shared `con-paths` crate and the current Windows renderer performance track.

## `v0.1.0-beta.33` - 2026-04-22

### Improved

**Terminal — Windows Backend (preview)**
- Tuned low-hanging renderer performance issues in the Windows D3D11/DirectWrite path.
- Wired terminal theme colors, opacity, and transparency into the Windows renderer.
- Hardened Windows resize/readback behavior, theme switching clear colors, and DWM backdrop behavior across the beta feedback loop.

## `v0.1.0-beta.32` - 2026-04-21

### Added

**Release and Packaging**
- Added Windows in-place auto-update flow and appcast support.

### Fixed

**Release and Packaging**
- Preserved the full beta tag in Windows release/appcast version metadata so beta update comparisons do not collapse to `0.1.0`.
- Made the PowerShell installer ASCII-safe for `irm | iex` usage.
- Updated release/download documentation for the renamed `con-terminal` repository.

## `v0.1.0-beta.31` - 2026-04-21

### Added

**Interface**
- Added a titlebar gear button on Windows and Linux for opening Settings.

**Release and Packaging**
- Added Windows PowerShell installer and release palette entries.

### Fixed

**Terminal — Windows Backend (preview)**
- Switched Windows builds to the console subsystem behavior needed for terminal process startup.
- Baked the full release channel/version into the Windows binary for update polling and user-visible version display.

## `v0.1.0-beta.30` - 2026-04-21

### Added

**Terminal — Windows Backend (preview)**
- Shipped the runtime-validated Phase 3b Windows glyph-atlas renderer: ConPTY plus `libghostty-vt`, D3D11/DirectWrite rendering, HLSL shaders, glyph atlas packing, Nerd Font/PUA handling, wide-glyph ordering, and cursor z-order handling.
- Promoted Windows from planned work to the first beta surface in the docs.

### Fixed

**Release and Packaging**
- Hardened the Windows release workflow against Defender interference, Zig/uucode spawn flakes, Windows runner instability, and MAX_PATH-sensitive cache paths.
- Renamed in-tree repository URLs from `nowledge-co/con` to `nowledge-co/con-terminal`.

## `v0.1.0-beta.29` - 2026-04-21

Tag-only release workflow iteration folded into the `v0.1.0-beta.30` GitHub release notes.

### Changed
- Renamed repository URLs to `nowledge-co/con-terminal`.

## `v0.1.0-beta.28` - 2026-04-21

Tag-only release workflow iteration folded into the `v0.1.0-beta.30` GitHub release notes.

### Fixed
- Pinned the Windows release runner to `windows-2022` and added on-failure diagnostics.

## `v0.1.0-beta.27` - 2026-04-21

Tag-only release workflow iteration folded into the `v0.1.0-beta.30` GitHub release notes.

### Fixed
- Retried Windows release builds around intermittent Zig/uucode spawn failures.

## `v0.1.0-beta.26` - 2026-04-21

Tag-only release workflow iteration folded into the `v0.1.0-beta.30` GitHub release notes.

### Fixed
- Disabled Defender real-time scanning during Windows release builds.

## `v0.1.0-beta.25` - 2026-04-21

Tag-only release workflow iteration folded into the `v0.1.0-beta.30` GitHub release notes.

### Added
- Advanced the Windows beta port toward the first published Windows preview.

## `v0.1.0-beta.24` - 2026-04-20

### Improved
- Shipped CI fixes, documentation updates, and broad UI/terminal polish before the Windows beta push.

## `v0.1.0-beta.23` - 2026-04-20

### Added
- Landed Windows support phase 0: non-macOS build scaffolding, portability gates, and an initial Windows-renderable terminal path.

## `v0.1.0-beta.22` - 2026-04-16

### Improved
- Shipped a broad optimization pass across terminal/app behavior.

## `v0.1.0-beta.21` - 2026-04-16

### Added
- Added Kimi provider support for coding workflows.

### Fixed
- Fixed Homebrew promotion and release workflow YAML issues.

## `v0.1.0-beta.20` - 2026-04-15

### Added
- Added Homebrew cask and a one-line install path.

### Improved
- Shipped another beta optimization pass and Zig CI hardening.

## `v0.1.0-beta.19` - 2026-04-15

### Fixed
- Polished terminal cursor behavior for CJK IME input.

## `v0.1.0-beta.18` - 2026-04-15

### Added
- Added early installer work and beta feature polish.

### Fixed
- Fixed Vim quit handling, IME text input issues, macOS hotkeys, terminal flashing, and Ghostty Zig CI pinning.

## `v0.1.0-beta.14` - 2026-04-14

### Improved
- Optimized settings, markdown table rendering, and CJK rendering paths.

## `v0.1.0-beta.3` - 2026-04-14

### Added
- Added configurable UI font size.

### Fixed
- Fixed a markdown code-block rendering performance issue.

## `v0.1.0-beta.1` - 2026-04-14

### Added
- Initial public beta of con.
- Added the first release workflow, app bundle/release infrastructure, documentation, and upstream dependency cleanup.

### Fixed
- Fixed early Sparkle startup/release-upload issues and ordered-list wrapping alignment.
