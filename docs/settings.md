# Settings

con should feel good before you change anything. Use Settings when you want to
make the terminal fit your hands: choose a theme, connect an AI provider, tune
suggestions, add skills, or change shortcuts.

Open Settings from the app menu on macOS, the gear button on Windows and Linux,
or the Command Palette. Appearance changes apply as you tune them, so you can
leave Settings open while checking opacity, blur, fonts, and background images.

## If Con Cannot Start

If Con cannot read your settings, it opens a recovery window instead of silently
starting with defaults or changing your file. The window shows the file path and
the error. Choose **Open Settings File** to correct it, then **Try Again**.

This can happen after opening an older Con version with settings written by a
newer one. In that case, use **Check for Updates** (or **Get Latest Con**) rather
than deleting settings you still need. **Copy Details** gives you the exact
error if you need help. Your terminal session is not started until settings
validate successfully.

## General

General contains app-level behavior:

- update channel and version
- manual update check when the current channel supports it
- default shell command for new panes
- saved terminal text privacy
- skill source folders

**Default Shell** overrides the command Con starts in new panes. Leave it blank
to follow the platform default: your Windows Terminal profile on Windows, or
`$SHELL` on macOS and Linux. Arguments and quoted paths are supported. Saving
the setting and restarting Con affects new panes; existing shells are left
alone.

If you prefer con not to save terminal text between launches, turn off
**Restore Terminal Text** in General. Layout profiles never include terminal
text. To wipe terminal text already saved on disk, run **Clear Restored Terminal
History** from Command Palette.

## Command Palette

Press <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>P</kbd> on macOS, or
<kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>P</kbd> on Windows and Linux.

The Command Palette is the fastest way to find actions you do not use every
minute. Search for Settings, workspace profiles, pane actions, surface actions,
updates, privacy actions, and other commands. When an action has a shortcut,
con shows it beside the action.

## Appearance

Appearance controls the parts of con you look at all day:

<p align="center">
  <img width="1080" alt="Con Appearance settings with theme and transparency controls" src="https://github.com/user-attachments/assets/c730d158-139b-46d5-adbb-0cd48d38b603" />
</p>

- terminal theme
- terminal and UI fonts
- app icon
- terminal opacity and blur
- background image
- tab position
- pane title bars in split layouts

**App Icon** starts with Tail A6 for new configurations. Classic and the other
raccoons remain available; an existing saved choice stays yours.

- **macOS:** preview in the Dock and Cmd-Tab. Save Appearance to keep the choice
  and update the installed `.app` icon used by Finder and system search. Search
  tools may cache their icons.
- **Windows:** preview in running window icons, including the task switcher.
  Pinned shortcuts and Explorer keep the icon bundled with the app.
- **Linux:** preview X11 window icons where the window manager displays them.
  Save to update the per-user application launcher icon, also used by Wayland
  desktops. Your desktop may cache the old image until the launcher or session
  refreshes. Live Wayland window previews are not supported yet. Flatpak keeps
  its packaged launcher icon rather than writing outside the sandbox.

Save Appearance to keep a choice for the next launch. Discarding an unsaved
preview returns to your saved icon. All installation packages use Tail A6.

On macOS, the installed app must be writable; running from a read-only disk image or with `cargo run` only changes the running icon. Con reapplies the saved file icon at startup after an app update. Custom icons use macOS file metadata, not changes to the signed icon resource. This metadata can make `codesign --strict` reject a customized installation even though ordinary signature verification passes; release artifacts are still verified strictly before distribution.

Start with readability. Pick a theme with clear contrast, then adjust opacity or
blur only if the terminal remains easy to scan.

UI and terminal font sizes also scale the icons beside their text. Icon growth
is intentionally bounded, so larger type remains balanced in narrow panes and
compact window chrome.

Use **Add Fallback** when the primary terminal font does not cover CJK, emoji,
or symbols. Fallbacks are searched in the displayed order; use the arrow buttons
to reorder them and the trash button to remove one. Con always keeps its bundled
Nerd Font after your choices for prompt icons, followed by the operating system
fallback cascade. Changes preview immediately in open terminal panes.

The same ordered list can be configured in `con.conf` (`con-terminal.conf` on Windows):

```ini
font-family = "JetBrains Mono"
font-family = "Sarasa Mono SC"
font-family = "Segoe UI Emoji"
```

Pane title bars appear only when a tab has multiple panes. Keep them on if you
want direct close/fullscreen controls and drag-to-rearrange. Turn them off if
you prefer a sparse terminal surface and use shortcuts or the terminal context
menu for pane actions.

Additional `font-family` entries are optional. Con appends its bundled icon
font before the system fallback cascade. See [Configuration](impl/configuration.md)
for migration, native includes, and platform limits.

con can import Ghostty themes. Copy a theme, choose **Load from Clipboard**,
preview it, then save it when it feels right.

## AI providers

The Providers section stores the connection details for the model hosts you use.
The AI section chooses the active provider and model for the agent panel,
Command Palette AI actions, and AI fallback suggestions.

con supports Anthropic, OpenAI, Codex Local, ChatGPT, GitHub Copilot, OpenAI-compatible
hosts, MiniMax, Moonshot, Z.AI, DeepSeek, Groq, Gemini, Ollama, OpenRouter,
Mistral, Together, Cohere, Perplexity, and xAI.

Use the strongest model for agent work when the answer matters. Use a faster or
cheaper model for suggestions if you want inline help without slowing down the
terminal.

ChatGPT and GitHub Copilot can use OAuth, so you can leave the API key empty for
those sign-in flows. ChatGPT fetches its subscription model catalog after sign-in;
use **Fetch Models** to refresh it later. Con refreshes expired OAuth access tokens
before discovery and retains the last successful catalog across restarts, scoped
to the account and endpoint. OpenAI-compatible hosts can fetch
models from `/models` when the host supports it. If the host has no models
endpoint, type the model ID manually and save it.

ChatGPT Subscription defaults to GPT-5.6 Sol. Its fallback list includes Astra,
Sol, Terra, and Luna; actual availability depends on the account. **Reasoning
effort** offers the levels supported by both the model catalog and Con's current
adapter (up to `xhigh`). Leave it on **Provider default** to omit the override.
The configuration key is `con.agent.providers.chatgpt.reasoning_effort = high`.
Saved retired subscription models remain visible with a replacement hint;
Con does not silently rewrite the selection. These retirement checks do not
apply to the separate OpenAI API provider.

The provider picker in the agent panel shows configured providers. If a provider
is missing there, configure it first in Settings.

### Use an installed Codex CLI

Choose **Codex Local** in Providers if you already use Codex on this computer.
Con uses Codex's login and endpoint configuration; you do not need to enter
credentials again in Con.

1. Install a recent Codex CLI and run `codex login`, or keep your existing
   working Codex configuration.
2. Select **Codex Local** in Providers. Leave the model blank to follow Codex's
   configured default, or choose **Fetch Models** and select a model.
3. Choose **Codex Local** as the active provider in AI and save your settings.
   For an existing tab, choose it in the agent panel's provider picker; changing
   the global default does not replace that tab's current choice.

Con starts a separate local app-server for each request. Model requests still
go to the service configured in Codex; this is not offline inference. Con's
terminal context, tools, and approval controls remain in use. Running Codex
sessions are left alone.

If Con cannot find Codex, check that `codex` is on PATH. On macOS, Con also looks
inside Codex and ChatGPT app bundles. For another location, set `CON_CODEX_BIN`
to the executable's absolute path in the environment used to launch Con. A CLI
that lacks the required app-server APIs reports an error; Con does not silently
switch providers. See [local Codex configuration](impl/configuration.md#local-codex-provider)
for the protocol requirements and configuration keys.

## Tool approval

The agent should act in view. Leave tool approval on when you are in an
unfamiliar repository, a production shell, or any session where mistakes are
expensive.

Auto-Approve Tools lets the agent run tools without asking for each action. Use
it only in workspaces where you trust the task, the model, and the recovery
path.

## Command suggestions

Suggestions are terminal help, not a second prompt.

con checks local command history first. If history has no strong match, AI
Command Suggestions can ask the configured suggestion provider for a fallback.
You can turn this off, or route suggestions to a different provider and model
from the main agent.

Treat ghost text as a proposal. Accept it when it is what you meant, ignore it
when it is not.

## Skills

Skills are slash commands backed by a `SKILL.md` file. They are useful for work
you repeat: release checks, project playbooks, debugging routines, writing
rules, or team-specific workflows.

Type `/` in the input bar or agent panel to browse available skills.

Project skills live with the current workspace:

- `skills/`
- `.agents/skills/`
- `.con/skills/`

Global skills follow you across projects:

- `~/.config/con/skills`
- `~/.agents/skills`

On Windows, the config skills folder is `~/.config/con-terminal/skills`.

Keep project skills for shared project habits. Keep global skills for personal
habits. If names collide, the project skill wins so a repository can define its
own local meaning.

For the workflow loop, see [Skills and workflows](skills-and-workflows.md).

## Shortcuts

The Keys section shows editable shortcuts for app, pane, and surface actions.
It also includes the optional global Summon / Hide Con shortcut. That shortcut
is off by default because global hotkeys can conflict with launchers and window
managers.

On macOS, Keys also includes Quick Terminal. It is off by default. When enabled,
it opens a dedicated floating Con window from anywhere, separate from the main
window. The default shortcut is <kbd>⌘</kbd> <kbd>Backslash</kbd>, and you can
record a different one if it conflicts with your setup.

For the full behavior, including hide/show state and how it differs from Summon
/ Hide Con, see [Quick Terminal](quick-terminal.md).

Change shortcuts when the default conflicts with muscle memory. Leave them alone
when the built-in flow already works. A good setup should reduce decisions, not
create a private language you have to remember.
