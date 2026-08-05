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

/// Smart-name + smart-icon for tab metadata.
///
/// Priority:
/// 1. **User-supplied label** (set via inline rename or context menu)
///    — terminal-icon, no subtitle.
/// 2. **SSH host** (e.g. `prod-1.example.com`) — globe icon, subtitle
///    is `user@host` if available.
/// 3. **Focused process** parsed out of the OSC-set terminal title
///    (e.g. `vim README.md`, `htop`, `less log.txt`) — icon picked by
///    process kind (editor / monitor / pager / shell), subtitle is the
///    cwd basename.
/// 4. **CWD basename** — terminal icon, no subtitle.
/// 5. **Shell name** (`bash`, `zsh`, `fish`) — terminal icon, no
///    subtitle.
/// 6. Fallback `Tab N` — terminal icon, no subtitle.
pub(super) fn smart_tab_presentation(
    user_label: Option<&str>,
    ai_label: Option<&str>,
    ai_icon: Option<&'static str>,
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
            icon,
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
            icon,
            is_ssh: is_ssh_session,
        };
    }

    // 3. SSH host short-name (no AI needed for this).
    if let Some(host) = hostname.map(str::trim).filter(|s| !s.is_empty()) {
        return VerticalTabPresentation {
            name: host.to_string(),
            subtitle: cwd_subtitle(current_dir),
            icon: "phosphor/globe.svg",
            is_ssh: true,
        };
    }

    // 4. Focused-process heuristic.
    if let Some(raw) = title.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((command, icon)) = parse_focused_process(raw) {
            return VerticalTabPresentation {
                name: command,
                subtitle: cwd_subtitle(current_dir),
                icon,
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
                    icon: "phosphor/terminal.svg",
                    is_ssh: false,
                };
            }
        }
    }

    if let Some(raw) = title.map(str::trim).filter(|s| !s.is_empty()) {
        return VerticalTabPresentation {
            name: raw.to_string(),
            subtitle: None,
            icon: "phosphor/terminal.svg",
            is_ssh: false,
        };
    }

    VerticalTabPresentation {
        name: format!("Tab {}", tab_index + 1),
        subtitle: None,
        icon: "phosphor/terminal.svg",
        is_ssh: false,
    }
}

pub(super) fn tab_rename_initial_label(
    user_label: Option<&str>,
    ai_label: Option<&str>,
    ai_icon: Option<&'static str>,
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
        let p = smart_tab_presentation(
            None, None, None, None, Some("main.rs"), None, 0, true,
        );
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "main.rs");
        assert_eq!(p.subtitle, None);
        assert!(!p.is_ssh);
    }

    #[test]
    fn editor_only_tab_falls_back_to_editor_name() {
        let p = smart_tab_presentation(None, None, None, None, None, None, 2, true);
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "Editor");
    }

    #[test]
    fn editor_only_tab_respects_user_label() {
        let p = smart_tab_presentation(
            Some("My Notes"), None, None, None, Some("main.rs"), None, 0, true,
        );
        assert_eq!(p.name, "My Notes");
        assert_eq!(p.icon, "phosphor/file-code.svg");
    }

    #[test]
    fn terminal_tab_keeps_terminal_icon_when_editor_only_flag_false() {
        let p = smart_tab_presentation(None, None, None, None, None, None, 0, false);
        assert_eq!(p.icon, "phosphor/terminal.svg");
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
