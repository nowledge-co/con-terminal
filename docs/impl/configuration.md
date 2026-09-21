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

Session restoration, authentication, and history remain separate JSON data. Cargo
manifests and `.cargo/config.toml` are unrelated to this format.

## Syntax and ownership

Use Ghostty's native keys for terminal behavior. Repeated keys retain their native
meaning; for example, the first non-empty `font-family` is the primary face and later
entries are fallbacks. Native examples include `font-family`, `font-size`, `theme`,
`foreground`, `background`, `palette`, `cursor-style`, `background-opacity`, and
`background-image`.

Con-owned fields are dotted keys rooted at `con.appearance`, `con.agent`,
`con.keybindings`, `con.skills`, or `con.network`. The part after the namespace uses
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
```

`con.version = 1` is the current Con schema marker. Native-owned values must use
their Ghostty names rather than aliases such as `con.terminal.font_size`.

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
but it is not an atomic compare-and-swap against arbitrary editors.

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
if another process creates one while the prompt is open.

### Settings

The **Configuration** page opens the current file or directory. On macOS it also
finds Ghostty's current and legacy filenames in Application Support and the XDG
config directory. Select a detected source or choose another file explicitly.

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
Custom shaders and GTK CSS files are also copied. Missing optional includes are
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
