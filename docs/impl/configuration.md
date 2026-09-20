# Configuration

Con uses one primary, line-oriented configuration file. It accepts Ghostty's native
`key = value` terminal settings and adds namespaced Con settings; it does not require
TOML or GPUI configuration.

## Location and startup behavior

| Platform | Primary file |
| --- | --- |
| macOS | `~/Library/Application Support/con/config.ghostty` |
| Linux | `$XDG_CONFIG_HOME/con/config.ghostty`, defaulting to `~/.config/con/config.ghostty` |
| Windows | `%APPDATA%\con-terminal\config.ghostty` |

The former `config.toml` at the same location is retained indefinitely as a
read-only migration source for now. Con migrates it only when `config.ghostty` is
absent, writes the new file without clobbering a file that appeared concurrently,
and leaves the TOML original unchanged. Once `config.ghostty` exists it is
authoritative: a malformed new file is an error, not a reason to fall back to TOML.

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
fields are read only from the root `config.ghostty`; do not put `con.*` fields in an
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
