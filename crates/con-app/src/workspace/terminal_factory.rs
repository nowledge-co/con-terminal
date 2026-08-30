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

// ── Terminal factory functions ────────────────────────────────
//
// Standalone so they can be called both during ConWorkspace::new()
// (before `self` exists) and from create_terminal() (after).

pub(super) fn make_ghostty_terminal(
    app: &std::sync::Arc<con_ghostty::GhosttyApp>,
    cwd: Option<&str>,
    restored_screen_text: Option<&[String]>,
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
