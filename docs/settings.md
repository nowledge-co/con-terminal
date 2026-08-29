# Settings

con should feel good before you change anything. Use Settings when you want to
make the terminal fit your hands: choose a theme, connect an AI provider, tune
suggestions, add skills, or change shortcuts.

Open Settings from the app menu on macOS, the gear button on Windows and Linux,
or the Command Palette. Appearance changes apply as you tune them, so you can
leave Settings open while checking opacity, blur, fonts, and background images.

## General

General contains app-level behavior:

- update channel and version
- manual update check when the current channel supports it
- saved terminal text privacy
- skill source folders

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

**App Icon** chooses which raccoon appears in the Dock and Cmd-Tab while con is running. The installed `.app` icon in Finder stays Classic. Save Appearance to keep the selection.

Start with readability. Pick a theme with clear contrast, then adjust opacity or
blur only if the terminal remains easy to scan.

Pane title bars appear only when a tab has multiple panes. Keep them on if you
want direct close/fullscreen controls and drag-to-rearrange. Turn them off if
you prefer a sparse terminal surface and use shortcuts or the terminal context
menu for pane actions.

con can import Ghostty themes. Copy a theme, choose **Load from Clipboard**,
preview it, then save it when it feels right.

## AI providers

The Providers section stores the connection details for the model hosts you use.
The AI section chooses the active provider and model for the agent panel,
Command Palette AI actions, and AI fallback suggestions.

con supports Anthropic, OpenAI, ChatGPT, GitHub Copilot, OpenAI-compatible
hosts, MiniMax, Moonshot, Z.AI, DeepSeek, Groq, Gemini, Ollama, OpenRouter,
Mistral, Together, Cohere, Perplexity, and xAI.

Use the strongest model for agent work when the answer matters. Use a faster or
cheaper model for suggestions if you want inline help without slowing down the
terminal.

ChatGPT and GitHub Copilot can use OAuth, so you can leave the API key empty for
those sign-in flows. After signing in to ChatGPT, use **Fetch Models** to load
the models available to that subscription. OpenAI-compatible hosts can fetch
models from `/models` when the host supports it. If the host has no models
endpoint, type the model ID manually and save it.

The provider picker in the agent panel shows configured providers. If a provider
is missing there, configure it first in Settings.

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
