//! One-time, opt-in Ghostty import before creating any terminal sessions.
use con_core::{
    Config,
    config::transfer::{ghostty_config_candidates, prepare_import},
    session::Session,
};
use gpui::*;
use gpui_component::{
    ActiveTheme, Disableable, Sizable,
    button::{Button, ButtonVariant, ButtonVariants},
};
use std::path::{Path, PathBuf};

use crate::startup_args::StartupArgs;

#[derive(Default)]
struct ImportPending(bool);
impl Global for ImportPending {}

pub(crate) fn pending(cx: &App) -> bool {
    cx.try_global::<ImportPending>()
        .is_some_and(|state| state.0)
}

fn marker_path() -> PathBuf {
    con_paths::app_data_dir().join("first-run-complete")
}

fn eligible(config: &Path, session: &Path, marker: &Path) -> bool {
    [
        config.to_owned(),
        config.with_file_name("config.ghostty"),
        config.with_file_name("config.toml"),
        session.to_owned(),
        marker.to_owned(),
    ]
    .iter()
    .all(|path| matches!(path.symlink_metadata(), Err(error) if error.kind() == std::io::ErrorKind::NotFound))
}

fn remember_choice() {
    let marker = marker_path();
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(marker.parent().expect("app data directory"))?;
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
        {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error),
        }
    })();
    if let Err(error) = result {
        log::warn!("Could not remember first-run choice: {error}");
    }
}

pub(crate) fn start(config: Config, startup: StartupArgs, cx: &mut App) {
    if !eligible(
        &Config::config_path(),
        &Session::session_path(),
        &marker_path(),
    ) {
        launch(config, startup, cx);
        return;
    }
    let sources = ghostty_config_candidates();
    if sources.is_empty() {
        remember_choice();
        launch(config, startup, cx);
        return;
    }
    cx.set_global(ImportPending(true));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(560.0), px(440.0)), cx)),
        window_min_size: Some(size(px(375.0), px(360.0))),
        titlebar: Some(TitlebarOptions {
            title: Some("Welcome to Con".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    if let Err(error) = cx.open_window(options, |window, cx| {
        window.on_window_should_close(cx, |_, cx| {
            cx.quit();
            true
        });
        let view = cx.new(|cx| Welcome {
            config,
            startup: Some(startup),
            sources,
            busy: false,
            error: None,
            focus: cx.focus_handle(),
        });
        view.read(cx).focus.clone().focus(window, cx);
        cx.new(|cx| gpui_component::Root::new(view, window, cx).bg(cx.theme().background))
    }) {
        log::error!("Could not open import prompt: {error}");
        cx.quit();
    }
}

fn launch(config: Config, startup: StartupArgs, cx: &mut App) {
    cx.set_global(ImportPending(false));
    crate::open_con_window_with_startup(
        config,
        crate::startup_session(&startup),
        true,
        Some(startup),
        cx,
    );
}

struct Welcome {
    config: Config,
    startup: Option<StartupArgs>,
    sources: Vec<PathBuf>,
    busy: bool,
    error: Option<String>,
    focus: FocusHandle,
}

impl Welcome {
    fn continue_with(
        &mut self,
        source: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error = None;
        let current = self.config.clone();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                // App-wide keybindings and network clients already use `current`.
                // A different on-disk profile needs a fresh app initialization,
                // including when the user chooses not to import.
                if !eligible(&Config::config_path(), &Session::session_path(), &marker_path()) {
                    anyhow::bail!("Con configuration or session state changed while this prompt was open. Quit and reopen Con to load it.");
                }
                let config = if let Some(source) = source {
                    let prepared = prepare_import(&source, &current, &Config::config_path())?;
                    crate::workspace::ConWorkspace::validate_native_config_candidate(prepared.config())
                        .map_err(anyhow::Error::msg)?;
                    let config = prepared.config().clone();
                    prepared.commit_new()?;
                    config
                } else {
                    current
                };
                remember_choice();
                Ok::<_, anyhow::Error>(config)
            }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(config) => {
                        if let Some(startup) = this.startup.take() {
                            window.remove_window();
                            cx.defer(move |cx| launch(config, startup, cx));
                        }
                    }
                    Err(error) => { this.error = Some(error.to_string()); cx.notify(); }
                }
            });
        }).detach();
    }
}

impl Render for Welcome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut content = div().flex().flex_col().gap_4().p(px(24.0)).w_full().min_w_0()
            .child(div().text_size(px(22.0)).font_weight(FontWeight::SEMIBOLD).child("Bring your Ghostty settings"))
            .child(div().text_sm().text_color(theme.muted_foreground)
                .child("Import fonts, colors, and terminal settings. Your Ghostty files stay unchanged. Some settings may not apply to Con."))
            .child(div().text_sm().child("Configured startup commands will run when the terminal opens."));
        for (index, source) in self.sources.iter().enumerate() {
            let source = source.clone();
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded(px(12.0))
                    .bg(theme.foreground.opacity(0.07))
                    .child(
                        div()
                            .text_size(px(12.0))
                            .child(source.display().to_string()),
                    )
                    .child(
                        div().flex().child(
                            Button::new(("first-run-import", index))
                                .primary()
                                .small()
                                .label("Import and Start")
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.continue_with(Some(source.clone()), window, cx)
                                })),
                        ),
                    ),
            );
        }
        if let Some(error) = &self.error {
            content = content.child(
                div()
                    .p_3()
                    .rounded(px(8.0))
                    .bg(theme.danger.opacity(0.08))
                    .text_sm()
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        if self.busy {
            content = content.child(div().text_sm().child("Validating configuration…"));
        }
        content = content
            .child(
                div().flex().child(
                    Button::new("first-run-defaults")
                        .with_variant(ButtonVariant::Secondary)
                        .small()
                        .label(if self.error.is_some() {
                            "Continue Without Import"
                        } else {
                            "Use Con Defaults"
                        })
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.continue_with(None, window, cx)),
                        ),
                ),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .text_color(theme.muted_foreground)
                    .child("You can import later in Settings → Config."),
            );
        div()
            .id("first-run")
            .size_full()
            .min_w_0()
            .overflow_y_scroll()
            .track_focus(&self.focus)
            .on_action(|_: &crate::Quit, _, cx| cx.quit())
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(".SystemUIFont")
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::eligible;

    #[test]
    fn only_a_fresh_profile_is_eligible() {
        let root = std::env::temp_dir().join(format!("con-first-run-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let config = root.join("con.conf");
        let session = root.join("session.json");
        let marker = root.join("first-run-complete");
        assert!(eligible(&config, &session, &marker));
        for path in [
            &config,
            &session,
            &marker,
            &root.join("config.ghostty"),
            &root.join("config.toml"),
        ] {
            std::fs::write(path, "").unwrap();
            assert!(!eligible(&config, &session, &marker), "{}", path.display());
            std::fs::remove_file(path).unwrap();
            std::os::unix::fs::symlink(root.join("missing-dotfile"), path).unwrap();
            assert!(
                !eligible(&config, &session, &marker),
                "dangling {}",
                path.display()
            );
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }
}
