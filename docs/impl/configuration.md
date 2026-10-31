# Configuration

Con uses one primary, line-oriented configuration file. It accepts Ghostty's native
`key = value` terminal settings and adds namespaced Con settings; it does not require
TOML or GPUI configuration.

## Location and startup behavior

| Platform | Primary file |
| --- | --- |
| macOS | `~/Library/Application Support/con/con.conf` |
| Linux | `$XDG_CONFIG_HOME/con/con.conf`, defaulting to `~/.config/con/con.conf` |
| Windows | `%APPDATA%\con-terminal\con-terminal.conf` |

Windows reserves `CON` even with an extension, so it uses `con-terminal.conf`.
The filename does not change the syntax.

The primary file is authoritative. When absent, Con first copies the former
`config.ghostty` in the same directory verbatim, preserving relative includes,
resources, and comments. If that is also absent, it migrates `config.toml`.
Both originals remain unchanged as read-only migration sources. Creation never
clobbers a concurrently created primary file. A malformed higher-priority file is
an error, not a reason to fall back to an older file.

When loading fails, the app opens a settings recovery window rather than starting
a terminal with silent defaults. It displays the actual selected source file,
keeps the parse/validation error visible, and offers file opening, error copying,
updating, and retry. Retry validates from disk; a cold-start recovery relaunches
the app only after validation succeeds. The recovery path does not start a
terminal session, restore windows, or rewrite the configuration. The error is
also printed to stderr if the UI cannot open. An older binary encountering a
new `con.*` key remains an error: users should update the binary or explicitly
edit their configuration, never have settings silently dropped.

Session restoration, authentication, and history remain separate JSON data. Cargo
manifests and `.cargo/config.toml` are unrelated to this format.

## Syntax and ownership

Use Ghostty's native keys for terminal behavior. Repeated keys retain their native
meaning; for example, the first non-empty `font-family` is the primary face and later
entries are fallbacks. Native examples include `font-family`, `font-size`, `theme`,
`foreground`, `background`, `palette`, `cursor-style`, `background-opacity`, and
`background-image`.

Con-owned fields are dotted keys rooted at `con.appearance`, `con.agent`,
`con.keybindings`, `con.skills`, `con.network`, or `con.experimental`. The part
after the namespace uses
the existing Rust field's `snake_case` spelling. Lists are represented by repeating
the key. Unknown or malformed `con.*` keys are errors; unknown native keys are
preserved for the native terminal parser.

```ini
# Con configuration
con.version = 1

# Native terminal settings. Repetition defines the font fallback order.
font-family = "Ioskeley Mono"
font-family = "Symbols Nerd Font"
font-size = 14
theme = "flexoki-dark"
cursor-style = bar
background-opacity = 0.88

# Con application settings use Rust field names.
con.appearance.ui_opacity = 0.92
con.appearance.tabs_orientation = vertical
con.appearance.hide_pane_title_bar = false
con.agent.provider = anthropic
con.agent.max_turns = 8
con.keybindings.toggle_agent = "secondary-l"
con.skills.project_paths = ".agents/skills"
con.skills.project_paths = ".con/skills"
con.network.https_proxy = "http://127.0.0.1:1086"
con.experimental.handoff = true
```

`con.version = 1` is the current Con schema marker. Native-owned values must use
their Ghostty names rather than aliases such as `con.terminal.font_size`.

### Magpie gateway

Choose **Magpie** in Settings → Providers. Start the Magpie app (or
`magpie serve`), click **Fetch Models**, and choose a model or enter its exact
ID. Magpie exposes models as `provider/model` and routing groups as `group/name`;
Con forwards those IDs unchanged. There is no built-in model default because
available providers and groups are configured in Magpie.

```ini
con.version = 1
con.agent.provider = magpie
# Example only: choose an ID returned by your gateway.
con.agent.providers.magpie.model = deepseek/deepseek-chat
# Optional: omit for http://127.0.0.1:3425/v1.
con.agent.providers.magpie.base_url = http://127.0.0.1:3425/v1
```

Local connections need no vendor key in Con: Magpie owns upstream credentials.
Con sends the attribution token `magpie-con`, following Magpie's
[integration contract](https://github.com/yetone/magpie/blob/main/docs/integrating.md).
For a remote gateway, set its Base URL (including `/v1`) and supply a gateway key
in Settings, `con.agent.providers.magpie.api_key_env`, or `MAGPIE_API_KEY`.
An explicit key/env-var setting takes precedence; Con never falls back to
`OPENAI_API_KEY` for Magpie. Missing explicit environment variables report an
error instead of silently using the local attribution token.

Inference uses Rig's OpenAI Chat Completions client with streaming and Con's
existing tools and approval flow. Model discovery reads `/v1/models` from the
same gateway; fetched catalogs are scoped to the provider and endpoint.
This integration does not install/start Magpie, edit Magpie's settings, or add
Con to Magpie's Agents page (that requires an upstream adapter).

### Local Codex provider

Choose **Codex Local** in Settings → Providers, then select it as the active AI
provider. Con launches `codex app-server --listen stdio://`; Codex manages its own
authentication, configured endpoint, and default model. Con does not copy Codex
credentials or require an API key or OAuth login for this provider.

Install the Codex CLI and sign in with `codex login` first (or keep an existing
working Codex configuration). This is a local **client**, not offline inference:
model availability and any required account/billing remain determined by Codex.

```ini
con.version = 1
con.agent.provider = codex-local
# Optional. Omit to use the model selected in Codex's configuration.
con.agent.providers.codex-local.model = gpt-5.3-codex
```

**Fetch Models** queries the installed app-server's `model/list`, not the public
OpenAI API or the ChatGPT Subscription catalog. Clear the model input to restore
Codex's configured default. Codex owns sampling/reasoning and token limits; Con's
HTTP endpoint, credentials, temperature, and token-ceiling overrides are not
forwarded. For this provider, `con.agent.max_turns` bounds Con dynamic tool calls
per request.

The executable is found on PATH (including Con's native CLI fallback directories).
On macOS, the bundled CLI in `Codex.app` or `ChatGPT.app` under `/Applications`
or `~/Applications` is also supported, including both `Contents/Resources/codex`
and `Contents/Resources/codex-cli/bin/codex`. Set `CON_CODEX_BIN` to an absolute
executable path if discovery picks the wrong installation. Codex honors its own
`CODEX_HOME`; Con never edits that directory's authentication/config files.

Con uses dedicated ephemeral threads, injects the recent conversation history,
and keeps tools on its shared Rig registry and approval flow. Native Codex shell
execution, hooks, apps/plugins, and configured MCP servers are disabled; the
Codex thread stays read-only and native approval requests are declined. Commands
and writes use Con tools. Cancellation terminates only Con's dedicated app-server,
not a running Codex app or daemon. On Unix, the dedicated process group is stopped
as well, including children launched by an npm shim. Stdio setup requests have a 30-second timeout;
a turn has a 15-minute limit.

This integration uses Codex's experimental `dynamicTools` and
`thread/inject_items` APIs. Use a recent Codex version supporting both; unsupported
protocols report an error rather than falling back to a different provider.

### Includes and provenance

`config-file` includes belong to Ghostty's native configuration graph. Con-specific
fields are read only from the primary root file; do not put `con.*` fields in an
included file. Con preserves comments, ordering, unknown native entries, and authored
values when Settings changes an unrelated field.

When the root has `config-file` includes, Settings refuses edits to native terminal
fields rather than appending a root override that may be ineffective. Edit the
included Ghostty file directly or remove the include first. Con-only fields can
remain in the root file.

Settings uses an optimistic external-edit check, then writes a private temporary file
and atomically replaces the destination. This detects changes seen before the write,
but it is not an atomic compare-and-swap against arbitrary editors. If the primary
configuration is a valid symlink managed by a dotfile tool, Con updates its target
atomically and preserves the link. Dangling links are treated as configuration errors.

## Themes and portability

`theme` is a native Ghostty setting. Con's built-in names such as `flexoki-light` and
`flexoki-dark` map to Con terminal palettes and drive the surrounding UI colors. On
macOS, when a selected name is not a Con built-in, it is left to Ghostty's native
theme lookup. Explicit native colors and palettes remain native settings. A
`config-file` include takes precedence over Con's palette seeding; edit that native
configuration when changing its theme.

macOS uses the real libghostty parser. Windows and Linux implement only Con's portable
subset over their own terminal backends, so a Ghostty setting accepted on macOS is
not a promise of identical behavior elsewhere. In particular, native Ghostty keybind
actions do not automatically become equivalent Con host-action shortcuts; configure
those with `con.keybindings.*` or Settings.

### Terminal and chrome backgrounds

The tab bar, vertical tab sidebar, and input bar share the terminal's effective
background color and opacity. Each base is painted once, with no foreground tint
or additional UI-opacity curve. On macOS the values come from libghostty after
theme lookup, includes, explicit overrides, and color-scheme selection.

`con.appearance.ui_opacity` remains supported for independent panels, popups, and
selection surfaces (Settings: **Panels and Popups**). It no longer controls those
three terminal-adjacent bases. No configuration migration is required.

On macOS, `background-blur` controls one window-wide native blur; GPUI does not add
a separate visual-effect material. `background-blur-radius` is an old alias of
the same setting, not an additional effect. If both are present, the later entry
wins. Prefer a single `background-blur = 70` (or `false`). Liquid Glass values
such as `macos-glass-regular` require native-app window integration that Con does
not yet provide: Con logs a warning and uses unblurred transparency instead.
Background images and per-cell/OSC background overrides remain terminal content;
they are not copied into the surrounding chrome.

## Import and export

### First launch (macOS)

With no Con configuration, previous session, or completed first-run choice,
Con discovers Ghostty configurations and offers **Import and Start** or
**Use Con Defaults** before creating any terminal. Multiple sources are shown
separately; no source is selected silently. Import validates and copies the chosen
file and its dependencies, then starts the terminal without a restart. Configured
startup commands may execute only after confirmation. Invalid imports leave the
destination absent and let the user retry or continue without importing.

Existing Con users load or migrate their configuration without this prompt.
With no Ghostty source, Con starts with defaults immediately. The choice is
remembered as `first-run-complete` in the app data directory, not in `con.conf`;
later imports remain available in Settings. Closing the prompt quits without
recording a choice. First-run import never replaces an existing Con file, even
if another process creates one while the prompt is open. If the profile changes
during the prompt, both choices require quitting and reopening Con so app-wide
settings and terminal settings initialize from the same configuration.

### Settings

The **Configuration** page opens the current file or directory. On macOS it also
finds Ghostty's current and legacy filenames in Application Support and the XDG
config directory. Select a detected source or choose another file explicitly.

Ghostty loads only named default files, not every file in these directories:
`config` followed by `config.ghostty`, in the XDG directory first and then macOS
Application Support. When several exist, Ghostty layers them. Con imports one
selected root and its `config-file` dependencies, not the merged defaults from
all detected roots. Other filenames require an explicit selection or include;
Con's own `con.conf` does not become a Ghostty default merely by being copied
into Ghostty's directory.

Import prepares and validates a private snapshot before asking for confirmation.
It replaces native terminal settings as a unit, retains Con's `con.*` settings,
and leaves Ghostty's files unchanged. Save or discard Settings drafts first.
An existing Con file is backed up as `previous.ghostty` in the import directory;
the result shows that path. Keep the import directory: the new configuration may
reference resources inside it. Cancelled and failed preparations are removed.

Use **Restart Con…** after import, then confirm the restart. Running commands and
agent tasks stop; save unsaved editor files first. Existing terminal sessions are
not reconfigured, and Settings saves are blocked until restart so stale controls cannot overwrite the
import. Imported commands may run when a new terminal session starts. Includes
retain the native-edit restrictions described above. Ghostty application actions
are not necessarily implemented by Con.

External edits detected before preparation or commit abort the import. Like
ordinary Settings saves, this is optimistic conflict detection, not filesystem
compare-and-swap against arbitrary external editors during the final replacement.
Windows and Linux keep the configuration-file controls but do not offer GUI
import until their backend can validate the imported native configuration.

### Command line

These commands operate on files without connecting to a running Con app:

```sh
con-cli config import-ghostty --from PATH [--to PATH]
con-cli config export-ghostty --to PATH [--from PATH]
```

Import defaults `--to` to Con's primary config; export defaults `--from` to it. Both
refuse to clobber an existing destination. They create self-contained snapshots by
copying and rewriting referenced `config-file`, theme, and background-image resources.
Custom shaders and GTK CSS files are also copied. Valid symlinks are followed for
dotfile-managed sources, while dangling links fail without publishing a partial
snapshot. Missing optional includes are
omitted so the snapshot cannot acquire future dependencies from the source directory.
Transfers bound both input and rewritten output to 16 MiB, with at most 128 input
resources and 16 nested includes. These are safety limits, not format restrictions.

Export removes Con fields and comments. Native values are otherwise retained and may
contain credentials, tokens, hostnames, paths, or other secrets, so review the entire
export before sharing it.

## Workspace layout profiles

Project layouts use `.con/workspace.ghostty`, a separate format identified by:

```ini
format = "con.workspace.layout"
version = 2
root = "."
```

Repeated `tab`, `pane`, and `surface` declarations define display order. Named `node`
declarations and `node.first`, `node.second`, or `node.pane` references define each
tab's split tree. This file describes layout, not global terminal configuration.

Legacy `.con/workspace.toml` format v1 remains readable as a fallback. New saves and
exports must write `workspace.ghostty`; Con never writes the legacy TOML profile.


### Native agent continuity

`con.appearance.resume_agent_sessions` defaults to `true`. On macOS it enables
one automatic exact native conversation resume per restored eligible surface,
independently of terminal screen replay. See [native agent sessions](../native-agent-sessions.md).
Saved native IDs/store paths belong to private session JSON and are excluded
from shared `.con/workspace.ghostty` profiles.
