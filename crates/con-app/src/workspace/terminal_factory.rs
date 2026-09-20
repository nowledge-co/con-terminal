use super::*;

pub(super) fn theme_to_ghostty_colors(theme: &TerminalTheme) -> con_ghostty::TerminalColors {
    let mut palette = [[0u8; 3]; 16];
    for (i, c) in theme.ansi.iter().enumerate() {
        palette[i] = [c.r, c.g, c.b];
    }
    con_ghostty::TerminalColors {
        foreground: [theme.foreground.r, theme.foreground.g, theme.foreground.b],
        background: [theme.background.r, theme.background.g, theme.background.b],
        palette,
    }
}

#[cfg(target_os = "macos")]
pub(super) fn native_appearance_theme(
    name: impl Into<String>,
    foreground: [u8; 3],
    background: [u8; 3],
    palette: &[[u8; 3]; 256],
) -> TerminalTheme {
    let color = |rgb: [u8; 3]| con_terminal::Color::rgb(rgb[0], rgb[1], rgb[2]);
    TerminalTheme {
        name: name.into(),
        foreground: color(foreground),
        background: color(background),
        ansi: std::array::from_fn(|index| color(palette[index])),
    }
}

/// Expand legacy lowercase Con theme names to palette defaults. Preserve
/// Ghostty's case-sensitive catalog names and explicit native color overrides.
#[cfg(target_os = "macos")]
pub(crate) fn native_config_for_app(config: &Config) -> Result<String, String> {
    let source = config
        .native_config_text()
        .map_err(|error| error.to_string())?;
    let native_theme = source.lines().rev().find_map(|line| {
        line.split_once('=').and_then(|(key, value)| {
            (key.trim() == "theme").then(|| {
                let value = value.trim();
                value
                    .strip_prefix('"')
                    .and_then(|v| v.strip_suffix('"'))
                    .unwrap_or(value)
                    .to_string()
            })
        })
    });
    if native_theme.is_none()
        && source.lines().any(|line| {
            line.split_once('=')
                .is_some_and(|(key, _)| key.trim() == "config-file")
        })
    {
        return Ok(source);
    }
    let selected = native_theme
        .as_deref()
        .unwrap_or(config.terminal.theme.as_str());
    let Some(theme) =
        TerminalTheme::by_name(selected).filter(|_| selected == selected.to_lowercase())
    else {
        // An unknown name belongs to Ghostty's native theme lookup and must
        // not be shadowed by Con's fallback palette.
        return Ok(source);
    };
    let mut seeded = theme.to_ghostty_format();
    for line in source.lines() {
        let is_snapshotted_theme = line
            .split_once('=')
            .is_some_and(|(key, _)| key.trim() == "theme");
        if !is_snapshotted_theme {
            seeded.push_str(line);
            seeded.push('\n');
        }
    }
    Ok(seeded)
}

#[cfg(target_os = "macos")]
pub(crate) fn native_config_base_dir(config: &Config) -> Result<std::path::PathBuf, String> {
    let app_dir = con_paths::app_config_dir();
    let base_dir = config
        .base_path()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| app_dir.clone());
    if base_dir == app_dir && !base_dir.is_dir() {
        std::fs::create_dir_all(&base_dir)
            .map_err(|error| format!("create app config directory: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&base_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("secure app config directory: {error}"))?;
        }
    }
    if !base_dir.is_dir() {
        return Err(format!(
            "native Ghostty config base directory does not exist: {}",
            base_dir.display()
        ));
    }
    Ok(base_dir)
}

#[cfg(target_os = "macos")]
impl ConWorkspace {
    pub(crate) fn validate_native_config_candidate(config: &Config) -> Result<(), String> {
        let text = native_config_for_app(config)?;
        let base_dir = native_config_base_dir(config)?;
        con_ghostty::GhosttyApp::validate_native_config(text, base_dir).map(|_| ())
    }

    pub(super) fn sync_native_color_scheme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ghostty_app.set_color_scheme(matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        ));
        if let Some(effective) = self.ghostty_app.native_appearance() {
            self.terminal_theme = native_appearance_theme(
                self.config.terminal.theme.clone(),
                effective.foreground,
                effective.background,
                &effective.palette,
            );
            self.terminal_opacity = effective.background_opacity as f32;
            self.sync_gpui_theme_appearance(&self.terminal_theme.clone(), window, cx);
            cx.notify();
        }
    }
}

// ── Terminal factory functions ────────────────────────────────
//
// Standalone so they can be called both during ConWorkspace::new()
// (before `self` exists) and from create_terminal() (after).

pub(super) fn make_ghostty_terminal(
    app: &std::sync::Arc<con_ghostty::GhosttyApp>,
    cwd: Option<&str>,
    restored_screen_text: Option<&[String]>,
    initial_working_directory: Option<std::path::PathBuf>,
    command: Option<crate::startup_args::TerminalCommand>,
    font_size: f32,
    window: &mut Window,
    cx: &mut Context<ConWorkspace>,
) -> TerminalPane {
    let app = app.clone();
    let cwd = cwd.filter(|cwd| !cwd.is_empty()).map(str::to_string);
    let restored_screen_text = restored_screen_text
        .map(|lines| lines.to_vec())
        .filter(|lines| !lines.is_empty());
    #[cfg(target_os = "linux")]
    let view = cx.new(|cx| {
        let cwd = initial_working_directory.or_else(|| cwd.map(std::path::PathBuf::from));
        crate::ghostty_view::GhosttyView::new(
            app,
            cwd,
            restored_screen_text,
            command,
            font_size,
            cx,
        )
    });
    #[cfg(not(target_os = "linux"))]
    let view = {
        debug_assert!(
            initial_working_directory.is_none(),
            "startup working directories are Linux-only"
        );
        debug_assert!(command.is_none(), "startup commands are Linux-only");
        cx.new(|cx| {
            crate::ghostty_view::GhosttyView::new(app, cwd, restored_screen_text, font_size, cx)
        })
    };
    let pane = TerminalPane::new(view);
    subscribe_terminal_pane(&pane, window, cx);
    pane
}

pub(super) fn find_git_worktree_root(start: &std::path::Path) -> Option<std::path::PathBuf> {
    start
        .ancestors()
        .find(|candidate| {
            let marker = candidate.join(".git");
            marker.is_dir() || marker.is_file()
        })
        .map(std::path::Path::to_path_buf)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::native_config_for_app;
    use con_core::Config;

    #[test]
    fn native_theme_names_and_include_only_sources_are_not_replaced() {
        for source in [
            "theme = Dracula\n",
            "config-file = colors.conf\n",
            "theme = light:Day,dark:Night\n",
        ] {
            let config = Config::parse_ghostty(source).unwrap();
            assert_eq!(native_config_for_app(&config).unwrap(), source);
        }
    }

    #[test]
    fn last_legacy_theme_is_seeded_before_explicit_native_colors() {
        let config = Config::parse_ghostty("theme = flexoki-dark\ntheme = flexoki-light\nbackground = 010203\nconfig-file = child\n").unwrap();
        let text = native_config_for_app(&config).unwrap();
        assert!(!text.contains("theme ="));
        assert!(text.starts_with("foreground = #100f0f\nbackground = #fffcf0\n"));
        assert!(text.ends_with("background = 010203\nconfig-file = child\n"));
    }
}
