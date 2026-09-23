use super::*;
#[cfg(target_os = "macos")]
use con_core::config::transfer::prepare_import;
use con_core::config::transfer::{PreparedImport, ghostty_config_candidates};
use std::path::PathBuf;

pub(super) enum ConfigurationImport {
    Idle,
    Preparing,
    Ready {
        source: PathBuf,
        prepared: Box<PreparedImport>,
    },
    Applying,
    Error(String),
}

#[derive(Default)]
pub(super) struct ConfigurationImportStatus {
    busy: bool,
    restart_required: bool,
    backup: Option<PathBuf>,
}

impl Global for ConfigurationImportStatus {}

impl ConfigurationImportStatus {
    pub(super) fn blocks_settings(cx: &App) -> bool {
        cx.try_global::<Self>()
            .is_some_and(|state| state.busy || state.restart_required)
    }
}

impl SettingsPanel {
    pub(super) fn discover_configuration_sources(&mut self, cx: &mut Context<Self>) {
        if self.configuration_sources.is_some() {
            return;
        }
        self.configuration_sources = Some(Vec::new());
        cx.spawn(async move |this, cx| {
            let sources = cx
                .background_executor()
                .spawn(async { ghostty_config_candidates() })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.configuration_sources = Some(sources);
                cx.notify();
            });
        })
        .detach();
    }

    fn choose_configuration_source(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a Ghostty configuration".into()),
        });
        cx.spawn_in(window, async move |this, window| {
            let source = paths.await.ok()?.ok()??.into_iter().next()?;
            window
                .update(|_, cx| {
                    let _ =
                        this.update(cx, |this, cx| this.prepare_configuration_import(source, cx));
                })
                .ok()
        })
        .detach();
    }

    fn prepare_configuration_import(&mut self, source: PathBuf, cx: &mut Context<Self>) {
        if ConfigurationImportStatus::blocks_settings(cx) {
            return;
        }
        if self.has_unsaved_changes(cx) {
            self.configuration_import = ConfigurationImport::Error(
                "Save or discard your Settings changes before importing.".into(),
            );
            cx.notify();
            return;
        }
        let current = self.config.clone();
        self.configuration_import = ConfigurationImport::Preparing;
        cx.set_global(ConfigurationImportStatus {
            busy: true,
            ..Default::default()
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let selected = source.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(target_os = "macos")]
                    {
                        let prepared = prepare_import(&selected, &current, &Config::config_path())?;
                        crate::workspace::ConWorkspace::validate_native_config_candidate(
                            prepared.config(),
                        )
                        .map_err(anyhow::Error::msg)?;
                        Ok::<_, anyhow::Error>(prepared)
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let _ = (selected, current);
                        anyhow::bail!(
                            "Settings import requires the native Ghostty backend on macOS."
                        )
                    }
                })
                .await;
            cx.update(|cx| cx.set_global(ConfigurationImportStatus::default()));
            let _ = this.update(cx, |this, cx| {
                this.configuration_import = match result {
                    Ok(prepared) => ConfigurationImport::Ready {
                        source,
                        prepared: Box::new(prepared),
                    },
                    Err(error) => ConfigurationImport::Error(error.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn commit_configuration_import(&mut self, cx: &mut Context<Self>) {
        if ConfigurationImportStatus::blocks_settings(cx) {
            return;
        }
        if self.has_unsaved_changes(cx) {
            self.configuration_import = ConfigurationImport::Error(
                "Settings changed after preparation. Save or discard them, then import again."
                    .into(),
            );
            cx.notify();
            return;
        }
        let ConfigurationImport::Ready { prepared, .. } =
            std::mem::replace(&mut self.configuration_import, ConfigurationImport::Idle)
        else {
            return;
        };
        self.configuration_import = ConfigurationImport::Applying;
        cx.set_global(ConfigurationImportStatus {
            busy: true,
            ..Default::default()
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { (*prepared).commit() })
                .await;
            cx.update(|cx| {
                cx.set_global(ConfigurationImportStatus {
                    busy: false,
                    restart_required: result.is_ok(),
                    backup: result.as_ref().ok().cloned().flatten(),
                });
            });
            let _ = this.update(cx, |this, cx| {
                this.configuration_import = match result {
                    Ok(_) => ConfigurationImport::Idle,
                    Err(error) => ConfigurationImport::Error(error.to_string()),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_configuration_file(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async {
                    let path = Config::config_path();
                    std::fs::create_dir_all(
                        path.parent()
                            .ok_or_else(|| anyhow::anyhow!("Missing config directory"))?,
                    )?;
                    let mut options = std::fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(0o600);
                    }
                    match options.open(&path) {
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error.into()),
                    }
                    Ok::<_, anyhow::Error>(path)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(path) => {
                        if let Ok(url) = Url::from_file_path(path) {
                            cx.open_url(url.as_str());
                        }
                    }
                    Err(error) => {
                        this.configuration_import = ConfigurationImport::Error(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_configuration(&mut self, _window: &Window, cx: &mut Context<Self>) -> Div {
        let blocked = ConfigurationImportStatus::blocks_settings(cx);
        let restart = cx
            .try_global::<ConfigurationImportStatus>()
            .is_some_and(|s| s.restart_required);
        let backup = cx
            .try_global::<ConfigurationImportStatus>()
            .and_then(|s| s.backup.clone());
        let muted = cx.theme().muted_foreground;
        let warning = cx.theme().warning;
        let page = section_content(
            "Configuration",
            "Your terminal settings, in one place.",
            cx.theme(),
        )
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .child(
            card(cx.theme(), self.card_opacity())
                .p(px(16.0))
                .w_full()
                .min_w_0()
                .gap_3()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Current configuration"),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .text_xs()
                        .text_color(muted)
                        .child(Config::config_path().display().to_string()),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("configuration-open")
                                .with_variant(gpui_component::button::ButtonVariant::Secondary)
                                .small()
                                .label("Open File")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.open_configuration_file(cx)),
                                ),
                        )
                        .child(
                            Button::new("configuration-folder")
                                .with_variant(gpui_component::button::ButtonVariant::Secondary)
                                .small()
                                .label("Open Folder")
                                .on_click(|_, _, cx| {
                                    cx.reveal_path(&con_paths::app_config_dir());
                                }),
                        ),
                ),
        );
        let mut content = card(cx.theme(), self.card_opacity())
            .p(px(16.0)).w_full().min_w_0().gap_3()
            .child(div().flex().flex_col().gap_2()
                .child(div().font_weight(FontWeight::MEDIUM).child("Import from Ghostty"))
                .child(div().text_sm().text_color(muted).child("Replace terminal settings. Keep Con’s AI, app shortcuts, and other Con-specific settings. Ghostty files stay unchanged.")));

        if restart {
            content = content.child(div().text_color(warning).child("Imported. Restart Con to use the new configuration. Existing terminal sessions have not changed."));
            if let Some(backup) = backup {
                content = content
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(format!("Previous configuration: {}", backup.display())),
                    )
                    .child(
                        Button::new("configuration-backup")
                            .ghost()
                            .small()
                            .label("Show Backup")
                            .on_click(move |_, _, cx| cx.reveal_path(&backup)),
                    );
            }
            content = content.child(div().flex().child(Button::new("configuration-restart").primary().small().label("Restart Con…")
                .on_click(|_, window, cx| {
                    let answer = window.prompt(PromptLevel::Warning, "Restart Con?", Some("Running terminal commands and agent tasks will stop. Save any unsaved editor files before restarting."), &["Cancel", "Restart Con"], cx);
                    cx.spawn(async move |cx| {
                        if answer.await.ok() == Some(1) {
                            cx.update(|cx| cx.restart());
                        }
                    }).detach();
                })));
            return page.child(content);
        }

        if !cfg!(target_os = "macos") {
            return page.child(content.child(div().text_sm().text_color(muted).child("Settings import is currently available on macOS. Other platforms support a portable subset of Ghostty settings.")));
        }
        if !matches!(self.configuration_import, ConfigurationImport::Ready { .. }) {
            if let Some(sources) = &self.configuration_sources {
                if sources.is_empty() {
                    content =
                        content.child(div().text_sm().text_color(muted).child(
                            "No default Ghostty configuration found. Choose a file to import.",
                        ));
                }
                for (index, source) in sources.iter().enumerate() {
                    let source = source.clone();
                    content = content.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(source.display().to_string()),
                            )
                            .child(
                                Button::new(("configuration-source", index))
                                    .primary()
                                    .small()
                                    .label("Import This File")
                                    .disabled(blocked)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.prepare_configuration_import(source.clone(), cx)
                                    })),
                            ),
                    );
                }
            }
            content = content.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("configuration-choose")
                            .with_variant(gpui_component::button::ButtonVariant::Secondary)
                            .small()
                            .label("Choose File…")
                            .disabled(blocked)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_configuration_source(window, cx)
                            })),
                    )
                    .child(
                        Button::new("configuration-rescan")
                            .ghost()
                            .small()
                            .label("Refresh")
                            .disabled(blocked)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.configuration_sources = None;
                                this.discover_configuration_sources(cx);
                            })),
                    ),
            );
        }
        match &self.configuration_import {
            ConfigurationImport::Idle => {}
            ConfigurationImport::Preparing => {
                content = content.child("Preparing and validating configuration…");
            }
            ConfigurationImport::Applying => {
                content = content.child("Saving configuration…");
            }
            ConfigurationImport::Error(error) => {
                content = content.child(div().flex().flex_col().gap_2()
                    .child(div().text_sm().text_color(warning).child(error.clone()))
                    .child(div().text_sm().text_color(muted).child("Resolve the reported issue and retry, or choose another configuration file.")));
            }
            ConfigurationImport::Ready { source, prepared } => {
                let includes = prepared
                    .config()
                    .native_entries()
                    .iter()
                    .any(|entry| entry.key == "config-file");
                content = content.child(div().font_weight(FontWeight::MEDIUM).child("Ready to import"))
                    .child(div().text_sm().text_color(muted).child(source.display().to_string()))
                    .child(div().text_sm().child("Con will back up the current file and replace its terminal settings. Restart to apply. Shell commands configured by this file may run in new terminal sessions."));
                if includes {
                    content = content.child(div().text_sm().text_color(warning).child("This configuration includes other files. Edit native terminal settings in those copied files; Settings cannot override them."));
                }
                content = content.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            Button::new("configuration-confirm")
                                .primary()
                                .small()
                                .label("Import Configuration")
                                .disabled(blocked)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.commit_configuration_import(cx)
                                })),
                        )
                        .child(
                            Button::new("configuration-cancel")
                                .ghost()
                                .small()
                                .label("Cancel")
                                .disabled(blocked)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.configuration_import = ConfigurationImport::Idle;
                                    cx.notify();
                                })),
                        ),
                );
            }
        }
        page.child(content)
    }
}
