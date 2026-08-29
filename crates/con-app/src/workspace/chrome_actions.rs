use super::*;

impl ConWorkspace {
    pub(super) fn toggle_agent_panel(
        &mut self,
        _: &ToggleAgentPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agent_panel_open = !self.agent_panel_open;
        let duration = Self::terminal_adjacent_chrome_duration(self.agent_panel_open, 290, 220);
        #[cfg(target_os = "macos")]
        if !duration.is_zero() {
            self.arm_chrome_transition_underlay(duration + Duration::from_millis(80));
        }
        #[cfg(target_os = "macos")]
        if duration.is_zero() {
            self.arm_agent_panel_snap_guard(cx);
        }
        self.agent_panel_motion
            .set_target(if self.agent_panel_open { 1.0 } else { 0.0 }, duration);
        if self.agent_panel_open {
            if self.input_bar_visible {
                self.input_bar.focus_handle(cx).focus(window, cx);
            } else {
                let focused_inline = self
                    .agent_panel
                    .update(cx, |panel, cx| panel.focus_inline_input(window, cx));
                if !focused_inline {
                    self.focus_agent_inline_input_next_frame(window, cx);
                }
            }
        } else {
            self.focus_terminal(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn toggle_input_bar(
        &mut self,
        _: &crate::ToggleInputBar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input_bar_visible = !self.input_bar_visible;
        if !self.input_bar_visible {
            self.pane_scope_picker_open = false;
        }
        let duration = Self::terminal_adjacent_chrome_duration(self.input_bar_visible, 210, 160);
        #[cfg(target_os = "macos")]
        if !duration.is_zero() {
            self.arm_chrome_transition_underlay(duration + Duration::from_millis(80));
        }
        #[cfg(target_os = "macos")]
        if duration.is_zero() {
            self.arm_input_bar_snap_guard(cx);
        }
        self.input_bar_motion
            .set_target(if self.input_bar_visible { 1.0 } else { 0.0 }, duration);
        if self.input_bar_visible {
            self.input_bar.focus_handle(cx).focus(window, cx);
        } else if self.agent_panel_open {
            let focused_inline = self
                .agent_panel
                .update(cx, |panel, cx| panel.focus_inline_input(window, cx));
            if !focused_inline {
                self.focus_agent_inline_input_next_frame(window, cx);
            }
        } else {
            if let Some(t) = self.try_active_terminal() {
                t.focus(window, cx);
            }
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn toggle_left_panel(
        &mut self,
        _: &ToggleLeftPanel,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.left_panel_open = !self.left_panel_open;
        if !self.vertical_tabs_enabled() {
            self.sidebar_tools_open = self.left_panel_open;
        } else if !self.left_panel_open {
            self.sidebar.update(cx, |sidebar, cx| {
                sidebar.clear_rail_only_override(cx);
            });
        }
        self.activity_bar.update(cx, |bar, cx| {
            bar.left_panel_open = if self.vertical_tabs_enabled() {
                self.sidebar_tools_open
            } else {
                self.left_panel_open
            };
            cx.notify();
        });
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn reveal_vertical_tab_rail_for_new_tab_if_needed(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        if !should_reveal_vertical_tab_rail_for_new_tab(
            self.vertical_tabs_enabled(),
            self.left_panel_open,
        ) {
            return;
        }

        self.left_panel_open = true;
        self.sidebar_tools_open = false;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.show_rail_only_preserving_pinned(cx);
        });
        self.activity_bar.update(cx, |bar, cx| {
            bar.left_panel_open = false;
            cx.notify();
        });
        cx.notify();
    }

    pub(super) fn collapse_sidebar(
        &mut self,
        _: &CollapseSidebar,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.left_panel_open {
            return;
        }
        self.left_panel_open = false;
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.clear_rail_only_override(cx);
        });
        self.activity_bar.update(cx, |bar, cx| {
            bar.left_panel_open = false;
            cx.notify();
        });
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn toggle_pane_scope_picker(
        &mut self,
        _: &TogglePaneScopePicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.input_bar.read(cx).mode() == InputMode::Agent
            || self.input_bar.read(cx).pane_infos().len() <= 1
        {
            return;
        }

        if !self.input_bar_visible {
            self.input_bar_visible = true;
            let duration = Self::terminal_adjacent_chrome_duration(true, 180, 180);
            #[cfg(target_os = "macos")]
            if duration.is_zero() {
                self.arm_input_bar_snap_guard(cx);
            }
            self.input_bar_motion.set_target(1.0, duration);
        }

        self.pane_scope_picker_open = !self.pane_scope_picker_open;
        if self.pane_scope_picker_open {
            self.input_bar.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_pane_scope_picker(&mut self, cx: &mut Context<Self>) {
        if self.pane_scope_picker_open {
            self.pane_scope_picker_open = false;
            cx.notify();
        }
    }

    pub(super) fn set_scope_broadcast(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_bar.update(cx, |bar, cx| {
            bar.set_broadcast_scope(window, cx);
        });
        self.sync_active_terminal_focus_states(cx);
        self.input_bar.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn set_scope_focused(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input_bar.update(cx, |bar, cx| {
            bar.set_focused_scope(cx);
        });
        self.sync_active_terminal_focus_states(cx);
        self.input_bar.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn toggle_scope_pane_by_id(
        &mut self,
        pane_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input_bar.update(cx, |bar, cx| {
            bar.toggle_scope_pane(pane_id, window, cx);
        });
        self.sync_active_terminal_focus_states(cx);
        self.input_bar.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn toggle_scope_pane_by_index(
        &mut self,
        pane_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panes = self.input_bar.read(cx).pane_infos();
        if let Some(pane) = panes.get(pane_index) {
            self.toggle_scope_pane_by_id(pane.id, window, cx);
        }
    }

    pub(super) fn active_pane_layout(&self, cx: &App) -> PaneLayoutState {
        self.tabs[self.active_tab].pane_tree.to_state(cx, false)
    }

    pub(super) fn clear_restored_terminal_history(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut next_config = self.config.clone();
        next_config.appearance.restore_terminal_text = false;
        crate::app_icon::merge_saved_into(&mut next_config.appearance.app_icon);
        if let Err(err) = next_config.save() {
            Self::show_layout_profile_error(
                window,
                cx,
                "Could not clear restored terminal history",
                err,
            );
            return;
        }
        self.config = next_config;

        self.settings_panel.update(cx, |panel, cx| {
            panel.set_persisted_restore_terminal_text(false, cx);
        });
        if let Some(panel) = self.settings_window_panel.clone() {
            panel.update(cx, |panel, cx| {
                panel.set_persisted_restore_terminal_text(false, cx);
            });
        }

        self.flush_session_save(cx);
        Self::show_layout_profile_info(
            window,
            cx,
            "Restored terminal history cleared",
            "Terminal text restore is now off. Re-enable it in Settings > General > Continuity."
                .to_string(),
        );
    }

    pub(super) fn layout_profile_export_root(&self, cx: &App) -> std::path::PathBuf {
        self.try_active_terminal()
            .and_then(|t| t.current_dir(cx))
            .map(std::path::PathBuf::from)
            .and_then(|path| {
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                find_git_worktree_root(&path).or(Some(path))
            })
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    pub(super) fn show_layout_profile_error(
        window: &mut Window,
        cx: &mut Context<Self>,
        message: &str,
        err: impl std::fmt::Display,
    ) {
        let detail = err.to_string();
        let _ = window.prompt(PromptLevel::Critical, message, Some(&detail), &["OK"], cx);
    }

    pub(super) fn show_layout_profile_info(
        window: &mut Window,
        cx: &mut Context<Self>,
        message: &str,
        detail: String,
    ) {
        let _ = window.prompt(PromptLevel::Info, message, Some(&detail), &["OK"], cx);
    }

    pub(super) fn save_current_layout_profile_to(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = crate::workspace_layout_root_for_file(&path);
        let session = self.snapshot_session_with_options(cx, false);
        let layout = WorkspaceLayout::from_session(&session, &root);

        if let Err(err) = layout.save(&path) {
            Self::show_layout_profile_error(window, cx, "Could not save layout profile", err);
            return;
        }

        let detail = format!(
            "{}\n\nThe file contains layout intent only: tabs, panes, surfaces, cwd, and agent defaults. It does not include terminal text, command history, conversations, credentials, or commands to run.",
            path.display()
        );
        Self::show_layout_profile_info(window, cx, "Layout profile saved", detail);
        cx.reveal_path(&path);
    }

    pub(super) fn export_workspace_layout(
        &mut self,
        _: &ExportWorkspaceLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.layout_profile_export_root(cx);
        let export_dir = root.join(".con");
        if let Err(err) = std::fs::create_dir_all(&export_dir) {
            Self::show_layout_profile_error(window, cx, "Could not prepare .con directory", err);
            return;
        }

        let save_path = cx.prompt_for_new_path(&export_dir, Some("workspace.toml"));
        cx.spawn_in(window, async move |this, window| {
            let path = save_path.await.ok()?.ok()??;
            window
                .update(|window, cx| {
                    let _ = this.update(cx, |workspace, cx| {
                        workspace.save_current_layout_profile_to(path, window, cx);
                    });
                })
                .ok()?;
            Some(())
        })
        .detach();
    }

    pub(super) fn tab_from_state(
        &mut self,
        tab_state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Tab {
        let ghostty_app = self.ghostty_app.clone();
        let font_size = self.font_size;
        let restore_terminal_text = self.config.appearance.restore_terminal_text;
        let pane_tree = if let Some(layout) = &tab_state.layout {
            let mut restore_terminal =
                |restore_cwd: Option<&str>,
                 restored_screen_text: Option<&[String]>,
                 force_restored_screen_text: bool| {
                    let restored_screen_text =
                        if restore_terminal_text || force_restored_screen_text {
                            restored_screen_text
                        } else {
                            None
                        };
                    make_ghostty_terminal(
                        &ghostty_app,
                        restore_cwd,
                        restored_screen_text,
                        font_size,
                        window,
                        cx,
                    )
                };
            PaneTree::from_state(layout, tab_state.focused_pane_id, &mut restore_terminal)
        } else {
            PaneTree::new(self.create_terminal(tab_state.cwd.as_deref(), window, cx))
        };
        let summary_id = self.next_tab_summary_id;
        self.next_tab_summary_id += 1;

        Tab {
            pane_tree,
            title: if tab_state.title.trim().is_empty() {
                format!("Terminal {}", summary_id + 1)
            } else {
                tab_state.title.clone()
            },
            user_label: tab_state.user_label.clone(),
            ai_label: None,
            ai_icon: None,
            color: tab_state.color,
            summary_id,
            summary_epoch: 0,
            needs_attention: false,
            session: AgentSession::new(),
            agent_routing: if tab_state.agent_routing.is_empty() {
                Self::default_agent_routing(self.harness.config())
            } else {
                tab_state.agent_routing.clone()
            },
            panel_state: PanelState::new(),
            runtime_trackers: RefCell::new(HashMap::new()),
            runtime_cache: RefCell::new(HashMap::new()),
            shell_history: Self::restore_shell_history(tab_state),
        }
    }

    pub(super) fn append_workspace_layout_session(
        &mut self,
        session: Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if session.tabs.is_empty() {
            return;
        }

        let old_active = self.active_tab;
        let first_new = self.tabs.len();
        let imported_active = session.active_tab.min(session.tabs.len().saturating_sub(1));

        for tab_state in &session.tabs {
            let tab = self.tab_from_state(tab_state, window, cx);
            self.tabs.push(tab);
        }

        if self.sync_tab_strip_motion() {
            #[cfg(target_os = "macos")]
            self.arm_top_chrome_snap_guard(cx);
        }

        self.active_tab = first_new + imported_active;
        let incoming = std::mem::replace(
            &mut self.tabs[self.active_tab].panel_state,
            PanelState::new(),
        );
        let outgoing = self
            .agent_panel
            .update(cx, |panel, cx| panel.swap_state(incoming, cx));
        if old_active < self.tabs.len() {
            self.tabs[old_active].panel_state = outgoing;
        }

        for (tab_idx, tab) in self.tabs.iter().enumerate() {
            if tab_idx == self.active_tab {
                continue;
            }
            for terminal in tab.pane_tree.all_surface_terminals() {
                terminal.set_focus_state(false, cx);
                terminal.set_native_view_visible(false, cx);
            }
        }

        for terminal in self.tabs[self.active_tab].pane_tree.all_terminals() {
            terminal.ensure_surface(window, cx);
        }
        self.sync_active_tab_native_view_visibility(cx);
        if let Some(terminal) = self.tabs[self.active_tab].pane_tree.try_focused_terminal() {
            terminal.focus(window, cx);
        }
        self.sync_active_terminal_focus_states(cx);
        self.sync_sidebar(cx);
        self.request_tab_summaries(cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn add_workspace_layout_tabs_from_path(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match crate::session_from_workspace_layout_path(&path) {
            Ok(session) => {
                let count = session.tabs.len();
                self.append_workspace_layout_session(session, window, cx);
                Self::show_layout_profile_info(
                    window,
                    cx,
                    "Layout profile added",
                    format!(
                        "Added {count} tab{} from {}.",
                        if count == 1 { "" } else { "s" },
                        path.display()
                    ),
                );
            }
            Err(err) => {
                Self::show_layout_profile_error(window, cx, "Could not open layout profile", err);
            }
        }
    }

    pub(super) fn prompt_for_workspace_layout_path(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        open_in_new_window: bool,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: false,
            prompt: Some("Choose a project folder or Con workspace layout".into()),
        });

        cx.spawn_in(window, async move |this, window| {
            let path = paths.await.ok()?.ok()??.into_iter().next()?;
            window
                .update(|window, cx| {
                    let _ = this.update(cx, |workspace, cx| {
                        if open_in_new_window {
                            match crate::session_from_workspace_layout_path(&path) {
                                Ok(session) => {
                                    let config = Config::load().unwrap_or_default();
                                    crate::open_con_window(config, session, false, cx);
                                }
                                Err(err) => Self::show_layout_profile_error(
                                    window,
                                    cx,
                                    "Could not open layout profile",
                                    err,
                                ),
                            }
                        } else {
                            workspace.add_workspace_layout_tabs_from_path(path, window, cx);
                        }
                    });
                })
                .ok()?;
            Some(())
        })
        .detach();
    }

    pub(super) fn add_workspace_layout_tabs(
        &mut self,
        _: &AddWorkspaceLayoutTabs,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_for_workspace_layout_path(window, cx, false);
    }

    pub(super) fn open_workspace_layout_window(
        &mut self,
        _: &OpenWorkspaceLayoutWindow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_for_workspace_layout_path(window, cx, true);
    }

    pub(super) fn release_active_terminal_mouse_selection(&self, cx: &mut App) {
        if self.active_tab >= self.tabs.len() {
            return;
        }
        for terminal in self.tabs[self.active_tab].pane_tree.all_terminals() {
            terminal.release_mouse_selection(cx);
        }
    }

    pub(super) fn render_scope_leaf(
        &self,
        pane_id: usize,
        pane: &PaneInfo,
        display_indices: &HashMap<usize, usize>,
        selected_ids: &HashSet<usize>,
        focused_id: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let is_selected = selected_ids.contains(&pane_id);
        let is_focused = pane_id == focused_id;
        let display_index = display_indices.get(&pane_id).copied().unwrap_or(0);
        let status_color = if !pane.is_alive {
            theme.danger
        } else if pane.is_busy {
            theme.warning
        } else {
            theme.success
        };
        let label = if let Some(host) = &pane.hostname {
            host.clone()
        } else if pane.name.is_empty() {
            format!("Pane {}", pane.id)
        } else {
            pane.name.clone()
        };
        let status_text = if !pane.is_alive {
            Some("offline")
        } else if pane.is_busy {
            Some("busy")
        } else if pane.hostname.is_some() {
            Some("remote")
        } else {
            None
        };
        let base_tile_surface = if theme.is_dark() {
            theme
                .title_bar
                .opacity(if is_focused { 0.80 } else { 0.70 })
        } else {
            theme
                .foreground
                .opacity(if is_focused { 0.044 } else { 0.034 })
        };
        let hover_tile_surface = if theme.is_dark() {
            theme.title_bar.opacity(0.84)
        } else {
            theme.foreground.opacity(0.052)
        };

        div()
            .id(SharedString::from(format!("scope-pane-{pane_id}")))
            .h_full()
            .w_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .px(px(10.0))
            .py(px(9.0))
            .rounded(px(9.0))
            .cursor_pointer()
            .bg(if is_selected {
                theme.primary.opacity(0.085)
            } else {
                base_tile_surface
            })
            .hover(|s| {
                s.bg(if is_selected {
                    theme.primary.opacity(0.12)
                } else {
                    hover_tile_surface
                })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.toggle_scope_pane_by_id(pane_id, window, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(div().size(px(5.0)).rounded_full().bg(status_color))
                                    .child(
                                        div()
                                            .text_size(px(11.2))
                                            .line_height(px(14.0))
                                            .font_family(theme.mono_font_family.clone())
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(if is_selected {
                                                theme.primary
                                            } else {
                                                theme.foreground
                                            })
                                            .min_w_0()
                                            .overflow_hidden()
                                            .overflow_x_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(label),
                                    ),
                            )
                            .children(status_text.map(|text| {
                                div()
                                    .text_size(px(10.0))
                                    .line_height(px(12.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground.opacity(0.62))
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(text)
                            })),
                    )
                    .child(
                        div().flex().items_center().gap(px(4.0)).child(
                            div()
                                .size(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.0))
                                .bg(if is_selected {
                                    theme.primary.opacity(0.10)
                                } else {
                                    theme.foreground.opacity(0.055)
                                })
                                .text_size(px(9.8))
                                .font_family(theme.mono_font_family.clone())
                                .text_color(if is_selected {
                                    theme.primary
                                } else {
                                    theme.muted_foreground.opacity(0.58)
                                })
                                .child(format!("{}", display_index + 1)),
                        ),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_scope_node(
        &self,
        layout: &PaneLayoutState,
        panes: &HashMap<usize, PaneInfo>,
        display_indices: &HashMap<usize, usize>,
        selected_ids: &HashSet<usize>,
        focused_id: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            PaneLayoutState::Leaf { pane_id, .. } => panes
                .get(pane_id)
                .map(|pane| {
                    self.render_scope_leaf(
                        *pane_id,
                        pane,
                        display_indices,
                        selected_ids,
                        focused_id,
                        cx,
                    )
                })
                .unwrap_or_else(|| {
                    div()
                        .h_full()
                        .w_full()
                        .rounded(px(9.0))
                        .bg(cx.theme().muted.opacity(0.06))
                        .into_any_element()
                }),
            PaneLayoutState::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let theme = cx.theme();
                let make_pane = |child: AnyElement, basis: f32| {
                    div()
                        .flex_grow()
                        .flex_shrink()
                        .flex_basis(relative(basis.clamp(0.15, 0.85)))
                        .overflow_hidden()
                        .child(child)
                };
                let divider = div()
                    .bg(theme.title_bar_border.opacity(0.75))
                    .map(|divider| match direction {
                        PaneSplitDirection::Horizontal => divider.w(px(1.0)).h_full(),
                        PaneSplitDirection::Vertical => divider.h(px(1.0)).w_full(),
                    });
                let mut container = div().flex().size_full().gap(px(6.0));
                container = match direction {
                    PaneSplitDirection::Horizontal => container.flex_row(),
                    PaneSplitDirection::Vertical => container.flex_col(),
                };
                container
                    .child(make_pane(
                        self.render_scope_node(
                            first,
                            panes,
                            display_indices,
                            selected_ids,
                            focused_id,
                            cx,
                        ),
                        *ratio,
                    ))
                    .child(divider)
                    .child(make_pane(
                        self.render_scope_node(
                            second,
                            panes,
                            display_indices,
                            selected_ids,
                            focused_id,
                            cx,
                        ),
                        1.0 - *ratio,
                    ))
                    .into_any_element()
            }
        }
    }
}
