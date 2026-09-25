use con_core::terminal_title::TitleIndicator;

/// Attention first, then focused activity, then the first active surface in
/// tree order. No timing-based arbitration: different frame rates cannot make
/// the representative indicator jump between terminals.
pub(super) fn tab_title_indicator(
    tree: &crate::pane_tree::PaneTree,
    cx: &gpui::App,
) -> Option<TitleIndicator> {
    let focused = tree.focused_terminal_entity_id();
    tree.all_surface_terminals()
        .into_iter()
        .filter_map(|terminal| {
            terminal.title_indicator(cx).map(|indicator| {
                let priority = (
                    matches!(indicator, TitleIndicator::Attention(_)),
                    Some(terminal.entity_id()) == focused,
                );
                (priority, indicator)
            })
        })
        .fold(None, |best, candidate| match best {
            Some((priority, _)) if priority >= candidate.0 => best,
            _ => Some(candidate),
        })
        .map(|(_, indicator)| indicator)
}

/// Derive a display name for a pane from available signals.
///
/// Priority:
/// 1. Proven remote hostname
/// 2. CWD directory name (skip bare home directories like `/Users/name`)
/// 3. Raw terminal title
/// 4. Fallback "Pane N"
pub(super) fn pane_display_name(
    hostname: &Option<String>,
    title: &Option<String>,
    current_dir: &Option<String>,
    pane_id: usize,
) -> String {
    // SSH session → show hostname
    if let Some(host) = hostname {
        return host.clone();
    }

    // CWD basename
    if let Some(dir) = current_dir {
        let path = std::path::Path::new(dir);
        // Skip bare home directories (e.g., /Users/weyl → "weyl" is confusing)
        let is_bare_home = matches!(
            path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy()),
            Some(ref name) if name == "home" || name == "Users"
        ) && path
            .parent()
            .and_then(|p| p.parent())
            .map_or(false, |pp| pp.parent().is_none());

        if !is_bare_home {
            if let Some(base) = path.file_name() {
                return base.to_string_lossy().to_string();
            }
        }
    }

    // Raw title from the visible surface
    if let Some(title) = title {
        let title = title.trim();
        if !title.is_empty() {
            return title.to_string();
        }
    }

    format!("Pane {}", pane_id + 1)
}

/// One row's worth of presentation data for tab metadata.
/// Computed by the workspace from the live tab state and pushed to
/// the panel via `sync_sessions`.
pub(super) struct VerticalTabPresentation {
    pub(super) name: String,
    pub(super) subtitle: Option<String>,
    pub(super) icon: &'static str,
    pub(super) is_ssh: bool,
}

/// Map a cached agent-CLI classification to its brand logo.
///
/// Unknown values and `None` fall through so the existing SSH / AI /
/// heuristic icon path can run.
pub(super) fn agent_cli_icon(agent_cli: Option<&str>) -> Option<&'static str> {
    match agent_cli? {
        "codex" => Some("agents/codex.svg"),
        "claude" => Some("agents/claude.svg"),
        "opencode" => Some("agents/opencode.svg"),
        "gemini" => Some("agents/gemini.svg"),
        "herdr" => Some("agents/herdr.svg"),
        "kimi" => Some("agents/kimi.svg"),
        "cursor" => Some("agents/cursor.svg"),
        "grok" => Some("agents/grok.svg"),
        "pi" => Some("agents/pi.svg"),
        "mimo" => Some("agents/mimo.svg"),
        "qoder" => Some("agents/qoder.svg"),
        "droid" => Some("agents/droid.svg"),
        "hermes" => Some("agents/hermes.svg"),
        "kiro" => Some("agents/kiro.svg"),
        "cline" => Some("agents/cline.svg"),
        "qwen" => Some("agents/qwen.svg"),
        "amp" => Some("agents/amp.svg"),
        "kilo" => Some("agents/kilo.svg"),
        "goose" => Some("agents/goose.svg"),
        "dim" => Some("agents/dim.svg"),
        _ => None,
    }
}

/// Map a foreground process name to a known interactive agent/TUI.
///
/// Screen-text classification (`classify_screen_agent_cli`) cannot see
/// these TUIs: some never print a stable brand marker, and others ship
/// as interpreter scripts whose process name is just `node`/`python3`.
/// A native binary's foreground process name is the most reliable
/// signal we have.
///
/// `grok` is matched by prefix because its binary carries the version,
/// e.g. `grok-1.0.34-macos-aarch64`.
pub(super) fn agent_from_process_name(name: &str) -> Option<&'static str> {
    let normalized = name.trim().to_ascii_lowercase();
    let name = normalized.strip_suffix(".exe").unwrap_or(&normalized);
    if name.starts_with("grok-") {
        return Some("grok");
    }
    match name {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "opencode" => Some("opencode"),
        "herdr" => Some("herdr"),
        "kimi" => Some("kimi"),
        "mimo" => Some("mimo"),
        "droid" => Some("droid"),
        "kiro-cli" | "kiro" => Some("kiro"),
        "crush" => Some("crush"),
        "goose" => Some("goose"),
        "amp" => Some("amp"),
        "dim" => Some("dim"),
        _ => None,
    }
}

/// A foreground shell is an authoritative negative signal: old agent banners
/// can remain visible after the TUI exits and must not keep the tab branded.
pub(super) fn process_name_is_shell(name: &str) -> bool {
    let name = name.trim().trim_start_matches('-').to_ascii_lowercase();
    matches!(
        name.as_str(),
        "sh" | "bash"
            | "dash"
            | "zsh"
            | "fish"
            | "nu"
            | "elvish"
            | "xonsh"
            | "ksh"
            | "csh"
            | "tcsh"
            | "pwsh"
            | "pwsh.exe"
            | "powershell"
            | "powershell.exe"
            | "cmd"
            | "cmd.exe"
    )
}

/// Map an OSC-set terminal title to a known agent CLI.
///
/// Some CLIs announce themselves only through the window title, and
/// their visible screen changes too much to anchor on. `grok` titles
/// itself `grok` (or `<session> - grok`); `pi` prefixes its title with
/// the `π` glyph; qwen/crush/kilo ship as interpreter scripts and put
/// their product name (plus the current directory) in the title.
pub(super) fn agent_from_osc_title(title: Option<&str>) -> Option<&'static str> {
    let title = title?.trim();
    if title.is_empty() {
        return None;
    }
    let lower = title.to_ascii_lowercase();
    if lower == "grok" || lower.ends_with(" - grok") {
        return Some("grok");
    }
    if title.contains('π') {
        return Some("pi");
    }
    if lower == "qwen" || lower.starts_with("qwen - ") || lower.starts_with("qwen code") {
        return Some("qwen");
    }
    if lower == "crush" || lower.starts_with("crush ") {
        return Some("crush");
    }
    if lower == "kilo" || lower.starts_with("kilo cli") || lower.starts_with("kilo code") {
        return Some("kilo");
    }
    if lower.contains(" - amp - ") {
        return Some("amp");
    }
    // DimAgent's CLI sets its title to exactly `dim`.
    if lower == "dim" {
        return Some("dim");
    }
    None
}

/// Map visible screen text to a known agent CLI.
///
/// Complements `classify_screen_agent_cli` (codex / claude / opencode)
/// with CLIs that ship as interpreter scripts, where the process name
/// carries no information. Markers are brand strings that survive
/// version bumps; version numbers and paths stay out of the patterns.
pub(super) fn agent_from_screen_text(lines: &[String]) -> Option<&'static str> {
    let joined = lines.join("\n").to_ascii_lowercase();
    if joined.contains("kimi code") {
        return Some("kimi");
    }
    if joined.contains("cursor agent") {
        return Some("cursor");
    }
    if joined.contains("grok build") {
        return Some("grok");
    }
    // Pi's prompt footer is stable while the session runs; the banner
    // ("pi v0.85.1") scrolls away, so anchor on the footer hints too.
    if joined.contains("escape interrupt") && joined.contains("ctrl+o more") {
        return Some("pi");
    }
    if joined.contains("mimocode") || joined.contains("mimo code") {
        return Some("mimo");
    }
    if joined.contains("welcome to qoder cli") {
        return Some("qoder");
    }
    if joined.contains("factory's ai coding agent") {
        return Some("droid");
    }
    if joined.contains("hermes agent") {
        return Some("hermes");
    }
    if joined.contains("welcome to kiro cli") {
        return Some("kiro");
    }
    if joined.contains("gemini cli") {
        return Some("gemini");
    }
    // Cline ships as a node script whose title carries no brand; its
    // permission prompts and question dialogs do.
    if joined.contains("cline needs permission")
        || joined.contains("cline is asking a question")
        || joined.contains("let cline use this tool")
    {
        return Some("cline");
    }
    None
}

const AGENT_CLI_SCREEN_SCAN_ATTEMPTS: u8 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AgentCliObservation {
    pub(super) terminal_id: u64,
    pub(super) foreground_process_group_id: Option<u64>,
    pub(super) title_agent: Option<&'static str>,
    pub(super) input_generation: u64,
}

/// Per-tab state for bounded agent detection.
///
/// Reading a terminal's visible screen is materially more expensive than
/// observing its process, title, or input generation. A changed observation
/// opens a short retry window so script-based TUIs can finish their initial
/// paint; a stable tab performs no screen reads after that window closes.
#[derive(Default)]
pub(super) struct AgentCliDetectionState {
    observation: Option<AgentCliObservation>,
    screen_scan_attempts_remaining: u8,
}

impl AgentCliDetectionState {
    pub(super) fn observe(&mut self, observation: AgentCliObservation) -> bool {
        if self.observation.as_ref() == Some(&observation) {
            return false;
        }
        self.observation = Some(observation);
        self.screen_scan_attempts_remaining = AGENT_CLI_SCREEN_SCAN_ATTEMPTS;
        true
    }

    pub(super) fn take_screen_scan_attempt(&mut self) -> bool {
        if self.screen_scan_attempts_remaining == 0 {
            return false;
        }
        self.screen_scan_attempts_remaining -= 1;
        true
    }

    pub(super) fn finish(&mut self) {
        self.screen_scan_attempts_remaining = 0;
    }

    pub(super) fn is_exhausted(&self) -> bool {
        self.screen_scan_attempts_remaining == 0
    }
}

/// Pump-path throttle: refresh immediately on the first call, then at
/// most once per `min_interval`.
pub(super) fn should_refresh_agent_cli(
    last_refresh: Option<std::time::Instant>,
    now: std::time::Instant,
    min_interval: std::time::Duration,
) -> bool {
    last_refresh.is_none_or(|last| now.saturating_duration_since(last) >= min_interval)
}

fn icon_or_agent(agent_cli: Option<&str>, fallback: &'static str) -> &'static str {
    agent_cli_icon(agent_cli).unwrap_or(fallback)
}

/// Smart-name + smart-icon for tab metadata.
///
/// Name priority:
/// 1. **User-supplied label** (set via inline rename or context menu)
/// 2. **AI label**
/// 3. **SSH host** (e.g. `prod-1.example.com`)
/// 4. **Focused process** parsed out of the OSC-set terminal title
/// 5. **CWD basename**
/// 6. **Shell name** (`bash`, `zsh`, `fish`)
/// 7. Fallback `Tab N`
///
/// Icon priority (terminal tabs only):
/// agent CLI logo > SSH globe > AI icon > heuristic.
pub(super) fn smart_tab_presentation(
    user_label: Option<&str>,
    ai_label: Option<&str>,
    ai_icon: Option<&'static str>,
    agent_cli: Option<&str>,
    hostname: Option<&str>,
    title: Option<&str>,
    current_dir: Option<&str>,
    tab_index: usize,
    is_editor_only: bool,
) -> VerticalTabPresentation {
    // Editor-only tabs (no terminal panes): fixed file icon, name
    // priority user label → AI label → tab title → "Editor".
    if is_editor_only {
        let name = user_label
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| ai_label.map(str::trim).filter(|s| !s.is_empty()))
            .or_else(|| title.map(str::trim).filter(|s| !s.is_empty()))
            .unwrap_or("Editor")
            .to_string();
        return VerticalTabPresentation {
            name,
            subtitle: None,
            icon: "phosphor/file-code.svg",
            is_ssh: false,
        };
    }

    let is_ssh_session = hostname.map(|h| !h.trim().is_empty()).unwrap_or(false);

    // Helper: pick the heuristic icon (used when no AI / SSH signal).
    let heuristic_icon = || {
        if let Some(raw) = title.map(str::trim).filter(|s| !s.is_empty()) {
            parse_focused_process(raw)
                .map(|(_, ic)| ic)
                .unwrap_or("phosphor/terminal.svg")
        } else {
            "phosphor/terminal.svg"
        }
    };

    // 1. User label always wins for the name.
    if let Some(label) = user_label.map(str::trim).filter(|s| !s.is_empty()) {
        let icon = if is_ssh_session {
            "phosphor/globe.svg"
        } else {
            // Prefer the AI-suggested icon for user-labelled tabs;
            // fall back to the heuristic.
            ai_icon.unwrap_or_else(heuristic_icon)
        };
        return VerticalTabPresentation {
            name: label.to_string(),
            subtitle: cwd_subtitle(current_dir),
            icon: icon_or_agent(agent_cli, icon),
            is_ssh: is_ssh_session,
        };
    }

    // 2. AI label sits between user label and heuristics — never
    //    overrides an explicit user choice, but does override the
    //    "vim README.md" / "htop" parse output.
    if let Some(label) = ai_label.map(str::trim).filter(|s| !s.is_empty()) {
        let icon = if is_ssh_session {
            "phosphor/globe.svg"
        } else {
            ai_icon.unwrap_or_else(heuristic_icon)
        };
        return VerticalTabPresentation {
            name: label.to_string(),
            subtitle: cwd_subtitle(current_dir),
            icon: icon_or_agent(agent_cli, icon),
            is_ssh: is_ssh_session,
        };
    }

    // 3. SSH host short-name (no AI needed for this).
    if let Some(host) = hostname.map(str::trim).filter(|s| !s.is_empty()) {
        return VerticalTabPresentation {
            name: host.to_string(),
            subtitle: cwd_subtitle(current_dir),
            icon: icon_or_agent(agent_cli, "phosphor/globe.svg"),
            is_ssh: true,
        };
    }

    // 4. Focused-process heuristic.
    if let Some(raw) = title.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((command, icon)) = parse_focused_process(raw) {
            return VerticalTabPresentation {
                name: command,
                subtitle: cwd_subtitle(current_dir),
                icon: icon_or_agent(agent_cli, icon),
                is_ssh: false,
            };
        }
    }

    if let Some(dir) = current_dir {
        let path = std::path::Path::new(dir);
        let is_bare_home = matches!(
            path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy()),
            Some(ref name) if name == "home" || name == "Users"
        ) && path
            .parent()
            .and_then(|p| p.parent())
            .map_or(false, |pp| pp.parent().is_none());
        if !is_bare_home {
            if let Some(base) = path.file_name() {
                return VerticalTabPresentation {
                    name: base.to_string_lossy().into_owned(),
                    subtitle: None,
                    icon: icon_or_agent(agent_cli, "phosphor/terminal.svg"),
                    is_ssh: false,
                };
            }
        }
    }

    if let Some(raw) = title.map(str::trim).filter(|s| !s.is_empty()) {
        return VerticalTabPresentation {
            name: raw.to_string(),
            subtitle: None,
            icon: icon_or_agent(agent_cli, "phosphor/terminal.svg"),
            is_ssh: false,
        };
    }

    VerticalTabPresentation {
        name: format!("Tab {}", tab_index + 1),
        subtitle: None,
        icon: icon_or_agent(agent_cli, "phosphor/terminal.svg"),
        is_ssh: false,
    }
}

pub(super) fn tab_rename_initial_label(
    user_label: Option<&str>,
    ai_label: Option<&str>,
    ai_icon: Option<&'static str>,
    agent_cli: Option<&str>,
    hostname: Option<&str>,
    title: Option<&str>,
    current_dir: Option<&str>,
    tab_index: usize,
    is_editor_only: bool,
) -> String {
    if let Some(label) = user_label.filter(|label| !label.trim().is_empty()) {
        label.to_string()
    } else {
        smart_tab_presentation(
            user_label,
            ai_label,
            ai_icon,
            agent_cli,
            hostname,
            title,
            current_dir,
            tab_index,
            is_editor_only,
        )
        .name
    }
}

pub(super) fn cwd_subtitle(current_dir: Option<&str>) -> Option<String> {
    let dir = current_dir?;
    let home = std::env::var("HOME").ok();
    if let Some(home) = home.as_deref() {
        if dir == home {
            return Some("~".to_string());
        }
        if let Some(rest) = dir.strip_prefix(home) {
            if rest.starts_with('/') {
                let trimmed = format!("~{rest}");
                return Some(shorten_path(&trimmed));
            }
        }
    }
    Some(shorten_path(dir))
}

pub(super) fn shorten_path(path: &str) -> String {
    const MAX_LEN: usize = 32;
    if path.chars().count() <= MAX_LEN {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 {
        return path.to_string();
    }
    let last = parts.last().copied().unwrap_or("");
    let parent = parts.get(parts.len() - 2).copied().unwrap_or("");
    let prefix = if path.starts_with('/') { "/" } else { "" };
    format!("{prefix}…/{parent}/{last}")
}

/// Parse a terminal title to extract the focused command and pick an
/// icon for it. Heuristic — terminals OSC-set their title to things
/// like `"vim README.md - ~/proj"` or `"htop"`. We strip trailing
/// `" — cwd"` suffixes, take the first word, and bucket it.
///
/// Returns `None` if the title looks like a bare shell name; the
/// caller falls through to cwd / shell naming so the row reads as a
/// shell session, not as a `bash`-named process.
pub(super) fn parse_focused_process(title: &str) -> Option<(String, &'static str)> {
    let trimmed = title
        .split(" — ")
        .next()
        .or_else(|| title.split(" - ").next())
        .unwrap_or(title)
        .trim();
    if trimmed.is_empty() {
        return None;
    }
    let first_word = trimmed
        .split(|c: char| c.is_whitespace() || c == ':')
        .next()
        .unwrap_or("")
        .trim_start_matches('/');
    let basename = first_word.rsplit('/').next().unwrap_or(first_word);
    let lower = basename.to_ascii_lowercase();
    match lower.as_str() {
        // Bare shells aren't an interesting "process" — fall through
        // so the row gets named by cwd or user label instead.
        "bash" | "sh" | "zsh" | "fish" | "dash" | "ksh" | "ion" | "nu" | "pwsh" | "powershell"
        | "cmd" | "tmux" | "screen" => None,

        "vim" | "nvim" | "vi" | "neovim" | "nano" | "emacs" | "ed" | "helix" | "hx" | "kakoune"
        | "kak" | "micro" | "code" | "codium" | "subl" => {
            Some((trimmed.to_string(), "phosphor/code.svg"))
        }

        "htop" | "top" | "btop" | "btm" | "atop" | "iotop" | "glances" | "nvtop" | "bashtop"
        | "ctop" | "k9s" => Some((trimmed.to_string(), "phosphor/pulse.svg")),

        "less" | "more" | "most" | "bat" | "cat" | "tail" | "head" | "view" | "man" => {
            Some((trimmed.to_string(), "phosphor/book-open.svg"))
        }

        "ssh" | "mosh" => Some((trimmed.to_string(), "phosphor/globe.svg")),

        "git" | "lazygit" | "tig" | "gh" => Some((trimmed.to_string(), "phosphor/file-code.svg")),

        _ => Some((trimmed.to_string(), "phosphor/terminal.svg")),
    }
}

/// Derive a display title for an editor tab.
///
/// - `Some(path)` → `path.file_name()` as string
/// - `None` → `"Editor"`
pub(crate) fn editor_tab_title(active_file: Option<&std::path::Path>) -> String {
    match active_file.and_then(|p| p.file_name()) {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "Editor".into(),
    }
}

#[cfg(test)]
mod tests_editor_tab_title {
    use super::*;

    #[test]
    fn test_basename_extraction() {
        let path = std::path::Path::new("/a/b/main.rs");
        assert_eq!(editor_tab_title(Some(path)), "main.rs");
    }

    #[test]
    fn test_file_without_extension() {
        let path = std::path::Path::new("/a/b/Makefile");
        assert_eq!(editor_tab_title(Some(path)), "Makefile");
    }

    #[test]
    fn test_none_returns_editor() {
        assert_eq!(editor_tab_title(None), "Editor");
    }

    #[test]
    fn test_path_ending_with_slash_returns_dir_name() {
        // Trailing slash means file_name() returns the directory name ("b"), not empty
        let path = std::path::Path::new("/a/b/");
        assert_eq!(editor_tab_title(Some(path)), "b");
    }

    #[test]
    fn test_root_path() {
        let path = std::path::Path::new("/");
        assert_eq!(editor_tab_title(Some(path)), "Editor");
    }

    #[test]
    fn test_hidden_file() {
        let path = std::path::Path::new("/a/b/.gitignore");
        assert_eq!(editor_tab_title(Some(path)), ".gitignore");
    }
}

#[cfg(test)]
mod tests_smart_tab_presentation_editor_only {
    use super::*;

    #[test]
    fn editor_only_tab_uses_file_code_icon_and_title() {
        let p =
            smart_tab_presentation(None, None, None, None, None, Some("main.rs"), None, 0, true);
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "main.rs");
        assert_eq!(p.subtitle, None);
        assert!(!p.is_ssh);
    }

    #[test]
    fn editor_only_tab_falls_back_to_editor_name() {
        let p = smart_tab_presentation(None, None, None, None, None, None, None, 2, true);
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "Editor");
    }

    #[test]
    fn editor_only_tab_respects_user_label() {
        let p = smart_tab_presentation(
            Some("My Notes"),
            None,
            None,
            None,
            None,
            Some("main.rs"),
            None,
            0,
            true,
        );
        assert_eq!(p.name, "My Notes");
        assert_eq!(p.icon, "phosphor/file-code.svg");
    }

    #[test]
    fn terminal_tab_keeps_terminal_icon_when_editor_only_flag_false() {
        let p = smart_tab_presentation(None, None, None, None, None, None, None, 0, false);
        assert_eq!(p.icon, "phosphor/terminal.svg");
    }
}

#[cfg(test)]
mod tests_agent_cli_icon {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn maps_known_agent_clis_and_falls_back() {
        assert_eq!(agent_cli_icon(Some("codex")), Some("agents/codex.svg"));
        assert_eq!(agent_cli_icon(Some("claude")), Some("agents/claude.svg"));
        assert_eq!(
            agent_cli_icon(Some("opencode")),
            Some("agents/opencode.svg")
        );
        assert_eq!(agent_cli_icon(Some("gemini")), Some("agents/gemini.svg"));
        assert_eq!(agent_cli_icon(Some("herdr")), Some("agents/herdr.svg"));
        assert_eq!(agent_cli_icon(Some("kimi")), Some("agents/kimi.svg"));
        assert_eq!(agent_cli_icon(Some("cursor")), Some("agents/cursor.svg"));
        assert_eq!(agent_cli_icon(Some("grok")), Some("agents/grok.svg"));
        assert_eq!(agent_cli_icon(Some("pi")), Some("agents/pi.svg"));
        assert_eq!(agent_cli_icon(Some("mimo")), Some("agents/mimo.svg"));
        assert_eq!(agent_cli_icon(Some("qoder")), Some("agents/qoder.svg"));
        assert_eq!(agent_cli_icon(Some("droid")), Some("agents/droid.svg"));
        assert_eq!(agent_cli_icon(Some("hermes")), Some("agents/hermes.svg"));
        assert_eq!(agent_cli_icon(Some("kiro")), Some("agents/kiro.svg"));
        assert_eq!(agent_cli_icon(Some("cline")), Some("agents/cline.svg"));
        assert_eq!(agent_cli_icon(Some("qwen")), Some("agents/qwen.svg"));
        assert_eq!(agent_cli_icon(Some("amp")), Some("agents/amp.svg"));
        assert_eq!(agent_cli_icon(Some("kilo")), Some("agents/kilo.svg"));
        assert_eq!(agent_cli_icon(Some("goose")), Some("agents/goose.svg"));
        assert_eq!(agent_cli_icon(Some("dim")), Some("agents/dim.svg"));
        // Crush has no usable monochrome mark; the icon falls back.
        assert_eq!(agent_cli_icon(Some("crush")), None);
        assert_eq!(agent_cli_icon(None), None);
        assert_eq!(agent_cli_icon(Some("unknown")), None);
        assert_eq!(agent_cli_icon(Some("")), None);
    }

    #[test]
    fn process_name_maps_only_known_tuis() {
        assert_eq!(agent_from_process_name("herdr"), Some("herdr"));
        assert_eq!(agent_from_process_name(" herdr "), Some("herdr"));
        assert_eq!(agent_from_process_name("kimi"), Some("kimi"));
        assert_eq!(agent_from_process_name("mimo"), Some("mimo"));
        assert_eq!(agent_from_process_name("droid"), Some("droid"));
        assert_eq!(agent_from_process_name("kiro-cli"), Some("kiro"));
        assert_eq!(agent_from_process_name("kiro"), Some("kiro"));
        // grok's binary carries its version suffix.
        assert_eq!(
            agent_from_process_name("grok-1.0.34-macos-aarch64"),
            Some("grok")
        );
        assert_eq!(agent_from_process_name("crush"), Some("crush"));
        assert_eq!(agent_from_process_name("goose"), Some("goose"));
        assert_eq!(agent_from_process_name("amp"), Some("amp"));
        // The npm amp build reports itself as `amp.exe` on macOS.
        assert_eq!(agent_from_process_name("amp.exe"), Some("amp"));
        assert_eq!(agent_from_process_name("dim"), Some("dim"));
        assert_eq!(agent_from_process_name("zsh"), None);
        assert_eq!(agent_from_process_name("node"), None);
        assert_eq!(agent_from_process_name("python3"), None);
        assert_eq!(agent_from_process_name("codex"), Some("codex"));
        assert_eq!(agent_from_process_name("Claude.EXE"), Some("claude"));
        assert_eq!(agent_from_process_name("opencode"), Some("opencode"));
        assert_eq!(agent_from_process_name("claude-helper"), None);
        assert_eq!(agent_from_process_name(""), None);
    }

    #[test]
    fn recognizes_shells_that_make_stale_screen_markers_non_authoritative() {
        for shell in [
            "zsh",
            "-zsh",
            "bash",
            "fish",
            "nu",
            "pwsh.exe",
            "PowerShell.EXE",
            "cmd.exe",
        ] {
            assert!(process_name_is_shell(shell), "missed shell {shell}");
        }
        assert!(!process_name_is_shell("node"));
        assert!(!process_name_is_shell("codex"));
    }

    #[test]
    fn osc_title_maps_known_titles() {
        assert_eq!(agent_from_osc_title(Some("grok")), Some("grok"));
        assert_eq!(agent_from_osc_title(Some("Grok")), Some("grok"));
        assert_eq!(
            agent_from_osc_title(Some("my-session - grok")),
            Some("grok")
        );
        assert_eq!(agent_from_osc_title(Some("π - tmp")), Some("pi"));
        assert_eq!(agent_from_osc_title(Some("Qwen - tmp")), Some("qwen"));
        assert_eq!(agent_from_osc_title(Some("crush /tmp")), Some("crush"));
        assert_eq!(agent_from_osc_title(Some("Kilo CLI")), Some("kilo"));
        assert_eq!(
            agent_from_osc_title(Some("my-repo - amp - main")),
            Some("amp")
        );
        assert_eq!(agent_from_osc_title(Some("dim")), Some("dim"));
        assert_eq!(agent_from_osc_title(Some("San3an.local: tmp")), None);
        // Partial words must not match.
        assert_eq!(agent_from_osc_title(Some("grokking")), None);
        assert_eq!(agent_from_osc_title(Some("qwen-project — zsh")), None);
        assert_eq!(agent_from_osc_title(Some("crushing-bugs — zsh")), None);
        assert_eq!(agent_from_osc_title(Some("kilobytes — zsh")), None);
        assert_eq!(agent_from_osc_title(None), None);
        assert_eq!(agent_from_osc_title(Some("   ")), None);
    }

    #[test]
    fn screen_text_maps_script_shipped_agents() {
        let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            agent_from_screen_text(&lines(&["Kimi Code updated to v2.0.1"])),
            Some("kimi")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Cursor Agent", "v2026.09.10"])),
            Some("cursor")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Grok Build  1.0.34"])),
            Some("grok")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&[
                "escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more"
            ])),
            Some("pi")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Welcome to Qoder CLI"])),
            Some("qoder")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Welcome to Kiro CLI V3!"])),
            Some("kiro")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Hermes Agent", "Nous Research"])),
            Some("hermes")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Gemini CLI v0.60.0"])),
            Some("gemini")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&[
                "Droid - Factory's AI coding agent in your terminal"
            ])),
            Some("droid")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Cline needs permission", "Approve tool call?"])),
            Some("cline")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["let cline use this tool"])),
            Some("cline")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["$ ls -la", "total 0"])),
            None
        );
        assert_eq!(agent_from_screen_text(&[]), None);
    }

    #[test]
    fn should_refresh_agent_cli_throttles_after_first_call() {
        let now = Instant::now();
        let interval = Duration::from_secs(1);
        assert!(should_refresh_agent_cli(None, now, interval));
        assert!(!should_refresh_agent_cli(
            Some(now),
            now + Duration::from_millis(500),
            interval
        ));
        assert!(should_refresh_agent_cli(
            Some(now),
            now + Duration::from_secs(1),
            interval
        ));
    }

    #[test]
    fn screen_detection_is_bounded_until_terminal_context_changes() {
        let observation = AgentCliObservation {
            terminal_id: 7,
            foreground_process_group_id: None,
            title_agent: None,
            input_generation: 2,
        };
        let mut state = AgentCliDetectionState::default();

        assert!(state.observe(observation));
        for _ in 0..AGENT_CLI_SCREEN_SCAN_ATTEMPTS {
            assert!(state.take_screen_scan_attempt());
        }
        assert!(!state.take_screen_scan_attempt());
        assert!(!state.observe(observation));
        assert!(!state.take_screen_scan_attempt());

        let changed = AgentCliObservation {
            input_generation: 3,
            ..observation
        };
        assert!(state.observe(changed));
        assert!(state.take_screen_scan_attempt());
        state.finish();
        assert!(state.is_exhausted());
        assert!(!state.take_screen_scan_attempt());
    }
}

#[cfg(test)]
mod tests_smart_tab_presentation_agent_cli {
    use super::*;

    #[test]
    fn detected_codex_uses_brand_logo_without_label() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("codex"),
            None,
            Some("zsh"),
            None,
            0,
            false,
        );
        assert_eq!(p.icon, "agents/codex.svg");
        assert_eq!(p.name, "zsh");
    }

    #[test]
    fn detected_codex_keeps_user_label_but_uses_logo() {
        let p = smart_tab_presentation(
            Some("My Session"),
            None,
            Some("phosphor/rocket.svg"),
            Some("codex"),
            None,
            Some("vim README.md"),
            None,
            0,
            false,
        );
        assert_eq!(p.name, "My Session");
        assert_eq!(p.icon, "agents/codex.svg");
    }

    #[test]
    fn agent_logo_wins_over_ssh_globe() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("codex"),
            Some("prod-1.example.com"),
            Some("ssh prod-1.example.com"),
            Some("/home/src"),
            0,
            false,
        );
        assert_eq!(p.name, "prod-1.example.com");
        assert_eq!(p.icon, "agents/codex.svg");
        assert!(p.is_ssh);
    }

    #[test]
    fn editor_tab_ignores_agent_logo() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("codex"),
            None,
            Some("main.rs"),
            None,
            0,
            true,
        );
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "main.rs");
    }

    #[test]
    fn no_agent_keeps_ssh_globe() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            None,
            Some("prod-1.example.com"),
            Some("ssh prod-1.example.com"),
            Some("/home/src"),
            0,
            false,
        );
        assert_eq!(p.icon, "phosphor/globe.svg");
        assert_eq!(p.name, "prod-1.example.com");
        assert!(p.is_ssh);
    }

    #[test]
    fn no_agent_keeps_heuristic_process_icon() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            None,
            None,
            Some("vim README.md"),
            None,
            0,
            false,
        );
        assert_eq!(p.icon, "phosphor/code.svg");
        assert_eq!(p.name, "vim README.md");
    }

    #[test]
    fn no_agent_keeps_ai_icon_under_user_label() {
        let p = smart_tab_presentation(
            Some("Deploy"),
            None,
            Some("phosphor/rocket.svg"),
            None,
            None,
            Some("zsh"),
            None,
            0,
            false,
        );
        assert_eq!(p.name, "Deploy");
        assert_eq!(p.icon, "phosphor/rocket.svg");
    }

    #[test]
    fn unknown_agent_falls_through_to_heuristic() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("unknown"),
            None,
            None,
            None,
            0,
            false,
        );
        assert_eq!(p.icon, "phosphor/terminal.svg");
        assert_eq!(p.name, "Tab 1");
    }
}

pub(super) fn longest_common_prefix<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for value in iter {
        let shared = prefix
            .chars()
            .zip(value.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix = prefix.chars().take(shared).collect();
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}
