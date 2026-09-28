use std::path::{Path, PathBuf};

use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{ActiveTheme, Sizable as _};
use url::Url;

struct ConfigRecovery {
    path: PathBuf,
    error: String,
    status: Option<String>,
    restart_required: bool,
}

fn config_file_to_open(primary: &Path) -> PathBuf {
    if primary.symlink_metadata().is_ok() {
        return primary.to_path_buf();
    }
    for name in ["config.ghostty", "config.toml"] {
        let path = primary.with_file_name(name);
        if path.symlink_metadata().is_ok() {
            return path;
        }
    }
    primary.to_path_buf()
}

pub(crate) fn restart() -> Result<(), std::io::Error> {
    let executable = std::env::current_exe()?;
    std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .spawn()?;
    Ok(())
}

impl ConfigRecovery {
    fn open_file(&mut self, cx: &mut Context<Self>) {
        match Url::from_file_path(&self.path) {
            Ok(url) => cx.open_url(url.as_str()),
            Err(()) => {
                self.status = Some("Could not open this path. Copy the details instead.".into());
                cx.notify();
            }
        }
    }

    fn copy_details(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(format!(
            "Con could not load {}\n{}",
            self.path.display(),
            self.error
        )));
        self.status = Some("Details copied".into());
        cx.notify();
    }

    fn retry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let config = match crate::load_validated_config() {
            Ok(config) => config,
            Err(error) => {
                self.error = error;
                self.path = config_file_to_open(&con_core::Config::config_path());
                self.status = Some("Still unable to load settings".into());
                cx.notify();
                return;
            }
        };

        if self.restart_required {
            if let Err(error) = restart() {
                self.status = Some(format!("Could not restart Con: {error}"));
                cx.notify();
                return;
            }
            cx.quit();
        } else {
            window.remove_window();
            crate::open_con_window(
                config,
                crate::fresh_window_session_with_history(),
                false,
                cx,
            );
        }
    }
}

impl Render for ConfigRecovery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let background = theme.background;
        let foreground = theme.foreground;
        let muted = foreground.opacity(0.64);
        let mono = theme.mono_font_family.clone();
        let update_button =
            if cfg!(target_os = "macos") && crate::updater::status().can_check_manually() {
                Button::new("recovery-check-updates")
                    .label("Check for Updates")
                    .small()
                    .ghost()
                    .on_click(|_, _, _| crate::updater::check_for_updates())
            } else {
                Button::new("recovery-get-latest")
                    .label("Get Latest Con")
                    .small()
                    .ghost()
                    .on_click(|_, _, cx| {
                        cx.open_url("https://github.com/nowledge-co/con-terminal/releases/latest");
                    })
            };

        div()
            .size_full()
            .bg(background)
            .p(px(28.0))
            .flex()
            .flex_col()
            .gap(px(14.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        svg()
                            .path("phosphor/warning.svg")
                            .size(px(22.0))
                            .text_color(theme.warning),
                    )
                    .child(
                        div()
                            .text_size(px(21.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(foreground)
                            .child("Con couldn't load its settings"),
                    ),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .line_height(relative(1.4))
                    .text_color(muted)
                    .child("Your settings are unchanged. Correct the file, then try again. If you recently switched to an older Con, updating may resolve the error."),
            )
            .child(
                div()
                    .id("config-recovery-details")
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .p(px(14.0))
                    .flex_1()
                    .min_h(px(72.0))
                    .overflow_y_scroll()
                    .rounded(px(8.0))
                    .bg(theme.secondary_active.opacity(0.35))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child("SETTINGS FILE"),
                    )
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_size(px(11.0))
                            .text_color(foreground)
                            .child(self.path.display().to_string()),
                    )
                    .child(
                        div()
                            .font_family(mono)
                            .text_size(px(11.0))
                            .line_height(relative(1.35))
                            .text_color(theme.danger)
                            .child(self.error.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        Button::new("recovery-open-config")
                            .label("Open Settings File")
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.open_file(cx))),
                    )
                    .child(
                        Button::new("recovery-copy-details")
                            .label("Copy Details")
                            .ghost()
                            .on_click(cx.listener(|this, _, _, cx| this.copy_details(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("recovery-retry")
                            .label("Try Again")
                            .primary()
                            .on_click(cx.listener(|this, _, window, cx| this.retry(window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(11.0))
                    .text_color(muted)
                    .child(self.status.clone().unwrap_or_else(|| "No settings were reset or overwritten.".into()))
                    .child(div().flex_1())
                    .child(update_button)
                    .child(
                        Button::new("recovery-quit")
                            .label("Quit Con")
                            .small()
                            .ghost()
                            .on_click(|_, _, cx| cx.quit()),
                    ),
            )
    }
}

pub(crate) fn open(error: String, restart_required: bool, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(620.0), px(430.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some("Con Settings".into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    let path = config_file_to_open(&con_core::Config::config_path());
    if let Err(window_error) = cx.open_window(options, move |window, cx| {
        let view = cx.new(|_| ConfigRecovery {
            path,
            error,
            status: None,
            restart_required,
        });
        cx.new(|cx| gpui_component::Root::new(view, window, cx).bg(cx.theme().background))
    }) {
        eprintln!("con: could not show settings recovery window: {window_error}");
    } else {
        crate::updater::init();
    }
}

#[cfg(test)]
mod tests {
    use super::config_file_to_open;

    #[test]
    fn recovery_opens_the_actual_source_without_changing_it() {
        let dir =
            std::env::temp_dir().join(format!("con-config-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let primary = dir.join("con.conf");
        let legacy = dir.join("config.toml");
        let previous = dir.join("config.ghostty");

        assert_eq!(config_file_to_open(&primary), primary);
        std::fs::write(&legacy, "invalid legacy file").unwrap();
        assert_eq!(config_file_to_open(&primary), legacy);
        std::fs::write(&previous, "invalid previous file").unwrap();
        assert_eq!(config_file_to_open(&primary), previous);
        std::fs::write(&primary, "invalid primary file").unwrap();
        assert_eq!(config_file_to_open(&primary), primary);
        assert_eq!(
            std::fs::read_to_string(&primary).unwrap(),
            "invalid primary file"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
