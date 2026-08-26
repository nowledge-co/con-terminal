use super::*;

impl ConWorkspace {
    pub(super) fn on_window_activation_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !window.is_window_active() || self.is_modal_open(cx) {
            return;
        }

        self.reassert_keyboard_focus_after_activation(window, cx);
    }

    fn reassert_keyboard_focus_after_activation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input_bar_focused =
            self.input_bar_visible && self.input_bar.focus_handle(cx).is_focused(window);
        let agent_inline_refocused = if !input_bar_focused && self.agent_panel_open {
            let inline_focused = self
                .agent_panel
                .read(cx)
                .inline_input_is_focused(window, cx);
            inline_focused
                && self
                    .agent_panel
                    .update(cx, |panel, cx| panel.focus_inline_input(window, cx))
        } else {
            false
        };

        let check_editor_or_terminal = !input_bar_focused && !agent_inline_refocused;
        let editor_has_keyboard_focus =
            check_editor_or_terminal && self.editor_has_keyboard_focus(window, cx);
        let active_pane_is_editor = check_editor_or_terminal && self.has_active_tab() && {
            let pane_tree = &self.tabs[self.active_tab].pane_tree;
            pane_tree
                .editor_view_for_pane(pane_tree.focused_pane_id())
                .is_some()
        };

        match activation_focus_target_for_reassertion(
            input_bar_focused,
            agent_inline_refocused,
            editor_has_keyboard_focus,
            active_pane_is_editor,
        ) {
            ActivationFocusTarget::InputBar => {
                self.input_bar.focus_handle(cx).focus(window, cx);
            }
            ActivationFocusTarget::AgentInlineInput => {}
            ActivationFocusTarget::ActiveEditor => {
                self.focus_active_editor_or_workspace(window, cx);
            }
            ActivationFocusTarget::Terminal => {
                self.focus_first_terminal(window, cx);
            }
        }
    }

    pub(super) fn minimize(&mut self, _: &Minimize, window: &mut Window, _cx: &mut Context<Self>) {
        window.minimize_window();
    }

    pub(super) fn quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_all_sessions();
        self.flush_session_save(cx);
        // Tear down ghostty surfaces before app exit to avoid Metal/NSView crashes.
        // Hide views and unfocus first, then clear the tabs vector so GhosttyTerminal
        // Drop runs (calling ghostty_surface_free) before cx.quit() exits the process.
        for tab in &self.tabs {
            for t in tab.pane_tree.all_surface_terminals() {
                t.set_focus_state(false, cx);
                t.set_native_view_visible(false, cx);
            }
        }
        self.tabs.clear();
        cx.quit();
    }

    pub(super) fn focus_input(
        &mut self,
        _: &FocusInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_modal_open(cx) {
            return;
        }

        if self.is_input_surface_focused(window, cx) {
            self.focus_first_terminal(window, cx);
            return;
        }

        self.focus_preferred_input_surface(window, cx);
    }

    /// Send the focused terminal's selection to the agent input.
    ///
    /// Grabs the current selection (if any), routes it to the preferred
    /// input surface (input bar in Agent mode, or the agent panel's inline
    /// input when the bar is hidden), and focuses that input so the user
    /// can type their question immediately. Without a selection this still
    /// focuses the agent input, so the shortcut doubles as "ask AI".
    pub(super) fn ask_ai(&mut self, _: &AskAi, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_modal_open(cx) {
            return;
        }

        let selection = self
            .has_active_tab()
            .then(|| {
                self.tabs[self.active_tab]
                    .pane_tree
                    .try_visible_focus_terminal()
                    .and_then(|(_, terminal)| terminal.selection_text(cx))
            })
            .flatten()
            .map(|text| text.trim_end().to_string())
            .filter(|text| !text.is_empty());

        if !self.input_bar_visible && self.agent_panel_open {
            let inserted = self.agent_panel.update(cx, |panel, cx| {
                panel.ask_ai_with_context(selection.as_deref(), window, cx)
            });
            if inserted {
                return;
            }
        }

        self.focus_input_bar_surface(window, cx);
        self.input_bar.update(cx, |bar, cx| {
            bar.ask_ai_with_context(selection.as_deref(), window, cx);
        });
    }

    pub(super) fn is_input_surface_focused(&self, window: &Window, cx: &App) -> bool {
        let input_bar_focused =
            self.input_bar_visible && self.input_bar.focus_handle(cx).is_focused(window);
        let agent_inline_focused = self.agent_panel_open
            && self
                .agent_panel
                .read(cx)
                .inline_input_is_focused(window, cx);

        input_bar_focused || agent_inline_focused
    }

    pub(super) fn focus_preferred_input_surface(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.input_bar_visible {
            if self.agent_panel_open {
                let focused_inline = self
                    .agent_panel
                    .update(cx, |panel, cx| panel.focus_inline_input(window, cx));
                if !focused_inline {
                    self.focus_agent_inline_input_next_frame(window, cx);
                }
                return;
            }
        }
        self.focus_input_bar_surface(window, cx);
    }

    pub(super) fn focus_input_bar_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input_bar_visible {
            self.input_bar_visible = true;
            self.save_session(cx);
        }
        let duration = Self::terminal_adjacent_chrome_duration(true, 180, 180);
        #[cfg(target_os = "macos")]
        if duration.is_zero() {
            self.arm_input_bar_snap_guard(cx);
        }
        self.input_bar_motion.set_target(1.0, duration);
        self.input_bar.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(super) fn focus_first_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_active_tab() {
            return;
        }

        self.pane_scope_picker_open = false;
        let (pane_id, terminal) = {
            let pane_tree = &self.tabs[self.active_tab].pane_tree;
            let Some((pane_id, terminal)) = pane_tree.try_visible_focus_terminal() else {
                self.focus_active_editor_or_workspace(window, cx);
                return;
            };
            (pane_id, terminal.clone())
        };
        self.tabs[self.active_tab].pane_tree.focus(pane_id);
        terminal.focus(window, cx);
        self.sync_active_terminal_focus_states(cx);
        cx.notify();
    }

    pub(super) fn cycle_input_mode(
        &mut self,
        _: &CycleInputMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.input_bar.update(cx, |bar, cx| {
            bar.cycle_mode(window, cx);
        });
        if self.input_bar.read(cx).mode() == InputMode::Agent {
            self.pane_scope_picker_open = false;
        }
        self.sync_active_terminal_focus_states(cx);
        cx.notify();
    }

    pub(super) fn toggle_command_palette(
        &mut self,
        _: &ToggleCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Close settings if open (mutually exclusive)
        if self.settings_panel.read(cx).is_overlay_visible() {
            self.settings_panel.update(cx, |panel, cx| {
                panel.toggle(window, cx);
            });
        }
        // The action keeps its historic "toggle" name for keymap
        // compatibility, but keyboard invocation is intentionally idempotent:
        // repeated key-down events while modifiers are held should focus the
        // palette, not close it and strand focus.
        self.command_palette.update(cx, |palette, cx| {
            palette.show(window, cx);
        });
        cx.notify();
    }

    pub(super) fn open_settings_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.settings_window {
            if handle
                .update(cx, |_, settings_window, _| {
                    settings_window.activate_window();
                })
                .is_ok()
            {
                return;
            }
            self.settings_window = None;
            self.settings_window_panel = None;
        }

        let config = self.config.clone();
        let registry = self.model_registry.clone();
        let runtime = self.harness.runtime_handle();
        let workspace = cx.weak_entity();
        let main_window = window.window_handle();
        let opened_panel: Rc<RefCell<Option<Entity<SettingsPanel>>>> = Rc::new(RefCell::new(None));
        let opened_panel_for_window = opened_panel.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(920.0), px(680.0)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Settings".into()),
                // Match the main macOS window: the system owns the traffic
                // lights, while Con paints the titlebar surface itself.
                appears_transparent: cfg!(target_os = "macos"),
                ..Default::default()
            }),
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        };

        match cx.open_window(options, move |settings_window, cx| {
            let panel = cx.new(|cx| {
                let mut panel =
                    SettingsPanel::new(&config, registry.clone(), runtime, settings_window, cx);
                panel.open_standalone(settings_window, cx);
                panel
            });
            *opened_panel_for_window.borrow_mut() = Some(panel.clone());

            let workspace_for_save = workspace.clone();
            let main_window_for_save = main_window;
            cx.subscribe(&panel, move |settings, _: &SaveSettings, cx| {
                let _ = main_window_for_save.update(cx, |_, window, cx| {
                    let _ = workspace_for_save.update(cx, |workspace, cx| {
                        workspace.apply_settings_from_panel(&settings, window, cx, false);
                    });
                });
            })
            .detach();

            let workspace_for_theme = workspace.clone();
            let main_window_for_theme = main_window;
            cx.subscribe(&panel, move |_settings, event: &ThemePreview, cx| {
                let theme_name = event.0.clone();
                let _ = main_window_for_theme.update(cx, |_, window, cx| {
                    let _ = workspace_for_theme.update(cx, |workspace, cx| {
                        workspace.apply_theme_preview(&theme_name, window, cx);
                    });
                });
            })
            .detach();

            let workspace_for_appearance = workspace.clone();
            let main_window_for_appearance = main_window;
            cx.subscribe(&panel, move |settings, _: &AppearancePreview, cx| {
                let _ = main_window_for_appearance.update(cx, |_, window, cx| {
                    let _ = workspace_for_appearance.update(cx, |workspace, cx| {
                        workspace.apply_appearance_preview_from_panel(&settings, window, cx);
                    });
                });
            })
            .detach();

            let panel_for_close = panel.clone();
            let workspace_for_close = workspace.clone();
            let main_window_for_close = main_window;
            settings_window.on_window_should_close(cx, move |window, cx| {
                let should_close = panel_for_close
                    .update(cx, |panel, cx| panel.request_standalone_close(window, cx));

                if should_close {
                    let _ = main_window_for_close.update(cx, |_, _window, cx| {
                        let _ = workspace_for_close.update(cx, |workspace, _cx| {
                            workspace.settings_window = None;
                            workspace.settings_window_panel = None;
                        });
                    });
                }

                should_close
            });

            let view = cx.new(|_| SettingsWindowView {
                panel: panel.clone(),
            });
            cx.new(|cx| {
                gpui_component::Root::new(view, settings_window, cx).bg(cx.theme().background)
            })
        }) {
            Ok(handle) => {
                self.settings_window = Some(handle.into());
                self.settings_window_panel = opened_panel.borrow().clone();
            }
            Err(err) => {
                log::error!("Failed to open settings window: {err}");
            }
        }
    }

    pub(super) fn toggle_settings(
        &mut self,
        _: &settings_panel::ToggleSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Close command palette if open (mutually exclusive)
        if self.command_palette.read(cx).is_visible() {
            self.command_palette.update(cx, |palette, cx| {
                palette.toggle(window, cx);
            });
        }
        self.open_settings_window(window, cx);
        cx.notify();
    }

    pub(super) fn is_modal_open(&self, cx: &App) -> bool {
        self.settings_panel.read(cx).is_overlay_visible()
            || self.command_palette.read(cx).is_visible()
    }

    pub(super) fn new_window(
        &mut self,
        _: &crate::NewWindow,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cwd = self
            .has_active_tab()
            .then(|| {
                self.try_active_terminal()
                    .and_then(|terminal| terminal.current_dir(cx))
            })
            .flatten();
        let config = con_core::Config::load().unwrap_or_default();
        let session = crate::fresh_window_session_with_history_for_cwd(
            cwd.as_deref().map(std::path::PathBuf::from),
        );
        crate::open_con_window(config, session, false, cx);
    }

    pub(super) fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_vertical_tab_rail_for_new_tab_if_needed(cx);
        let cwd = self
            .has_active_tab()
            .then(|| self.try_active_terminal().and_then(|t| t.current_dir(cx)))
            .flatten();
        let terminal = self.create_terminal(cwd.as_deref(), window, cx);
        let tab_number = self.tabs.len() + 1;
        let summary_id = self.next_tab_summary_id;
        self.next_tab_summary_id += 1;

        self.tabs.push(Tab {
            pane_tree: PaneTree::new(terminal),
            title: format!("Terminal {}", tab_number),
            user_label: None,
            ai_label: None,
            ai_icon: None,
            color: None,
            summary_id,
            summary_epoch: 0,
            needs_attention: false,
            session: AgentSession::new(),
            agent_routing: Self::default_agent_routing(self.harness.config()),
            panel_state: PanelState::new(),
            runtime_trackers: RefCell::new(HashMap::new()),
            runtime_cache: RefCell::new(HashMap::new()),
            shell_history: HashMap::new(),
        });

        if self.sync_tab_strip_motion() {
            #[cfg(target_os = "macos")]
            self.arm_top_chrome_snap_guard(cx);
            if Self::should_defer_top_chrome_refresh_when_tab_strip_appears() {
                cx.on_next_frame(window, |_, _, cx| {
                    cx.notify();
                });
            }
        }

        let new_index = self.tabs.len() - 1;
        self.activate_tab(new_index, window, cx);
    }

    pub(super) fn focus_agent_inline_input_next_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.on_next_frame(window, |workspace, window, cx| {
            if !workspace.agent_panel_open || workspace.input_bar_visible {
                return;
            }
            let focused = workspace
                .agent_panel
                .update(cx, |panel, cx| panel.focus_inline_input(window, cx));
            if !focused {
                workspace.focus_input_bar_surface(window, cx);
            }
        });
    }

    #[cfg(target_os = "macos")]
    pub fn mark_as_quick_terminal(&mut self) {
        self.is_quick_terminal = true;
    }

    pub(super) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        // Cmd+W maps to CloseTab, but for a focused EditorPane it should close
        // the active editor file first. Only empty editors fall through to pane/tab close.
        let active_tree = &self.tabs[self.active_tab].pane_tree;
        let keyboard_focused_editor_tabs = active_tree.focused_editor_tab_count(window, cx);
        let focused_pane_id = active_tree.focused_pane_id();
        let focused_pane_editor_tabs = active_tree
            .editor_view_for_pane(focused_pane_id)
            .map(|view| view.read(cx).tab_count());
        let close_intent = workspace_close_intent_for_close_tab(
            active_tree.pane_count(),
            keyboard_focused_editor_tabs,
            focused_pane_editor_tabs,
            self.tabs.len(),
        );
        log::debug!(
            "[editor-close] CloseTab: active_tab={} focused_pane={} pane_count={} keyboard_focused_editor_tabs={:?} focused_pane_editor_tabs={:?} intent={:?}",
            self.active_tab,
            focused_pane_id,
            active_tree.pane_count(),
            keyboard_focused_editor_tabs,
            focused_pane_editor_tabs,
            close_intent
        );
        if close_intent == WorkspaceCloseIntent::CloseEditorFile {
            let pane_id = active_tree
                .active_editor_pane_id(window, cx)
                .unwrap_or(focused_pane_id);
            if !self.close_pane_in_tab(self.active_tab, pane_id, window, cx) {
                self.close_tab_by_index(self.active_tab, window, cx);
            }
            return;
        }

        // If the active tab has multiple panes, close the focused pane first.
        // Only close the entire tab when it's down to a single pane.
        if self.tabs[self.active_tab].pane_tree.pane_count() > 1 {
            log::debug!(
                "[editor-close] CloseTab fallback closing pane: active_tab={} focused_pane={}",
                self.active_tab,
                self.tabs[self.active_tab].pane_tree.focused_pane_id()
            );
            let tab = &mut self.tabs[self.active_tab];
            let pane_id = tab.pane_tree.focused_pane_id();
            let closing_terminals = tab
                .pane_tree
                .surface_infos(Some(pane_id))
                .into_iter()
                .map(|surface| surface.terminal)
                .collect::<Vec<_>>();
            tab.pane_tree.close_focused();
            let surviving_terminals: Vec<TerminalPane> =
                tab.pane_tree.all_terminals().into_iter().cloned().collect();
            // Safe: may be None if only editor panes remain after close.
            let new_focus = tab.pane_tree.try_focused_terminal().cloned();

            #[cfg(target_os = "macos")]
            self.mark_active_tab_terminal_native_layout_pending(cx);

            for terminal in &surviving_terminals {
                terminal.ensure_surface(window, cx);
                terminal.notify(cx);
            }
            self.sync_active_tab_native_view_visibility(cx);

            if let Some(focused) = new_focus {
                focused.focus(window, cx);
            } else {
                // No terminal survived — focus workspace so keyboard shortcuts still work.
                self.workspace_focus.clone().focus(window, cx);
            }
            self.sync_active_terminal_focus_states(cx);
            cx.on_next_frame(window, move |_workspace, window, cx| {
                for terminal in &closing_terminals {
                    terminal.shutdown_surface(Some(&mut *window), cx);
                }
                for terminal in &surviving_terminals {
                    terminal.notify(cx);
                }
            });
            self.save_session(cx);
            cx.notify();
            return;
        }
        self.close_tab_by_index(self.active_tab, window, cx);
    }

    pub(super) fn close_tab_by_index(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() <= 1 {
            if self.is_quick_terminal {
                self.destroy_quick_terminal_window(window, cx);
                return;
            }

            if !self.config.appearance.close_to_quit {
                // Keep the window open: create a fresh tab, then close the
                // old one. Creating the new tab first lets it inherit the
                // current working directory before the old terminal is torn
                // down. After new_tab we have two tabs (old at 0, new at 1),
                // so the recursive close takes the normal multi-tab path.
                self.new_tab(&NewTab, window, cx);
                self.close_tab_by_index(0, window, cx);
                return;
            }

            self.close_window_from_last_tab(window, cx);
            return;
        }
        let closing_terminals: Vec<TerminalPane> = self.tabs[index]
            .pane_tree
            .all_surface_terminals()
            .into_iter()
            .cloned()
            .collect();
        // Save the closing tab's conversation
        {
            let conv = self.tabs[index].session.conversation();
            let _ = conv.lock().save();
        }
        let was_active = index == self.active_tab;
        self.reindex_pending_control_agent_requests_after_tab_close(index);
        self.reindex_pending_surface_control_requests_after_tab_close(index);
        self.remap_tab_rename_state_after_close(index);
        let removed = self.tabs.remove(index);
        // Drop the closed tab's cached AI summary so a future tab
        // assigned the same summary_id (which won't happen, since
        // ids are monotonic) doesn't inherit stale state.
        self.tab_summary_engine.forget(removed.summary_id);
        if self.sync_tab_strip_motion() {
            #[cfg(target_os = "macos")]
            self.arm_top_chrome_snap_guard(cx);
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        } else if self.active_tab > index {
            self.active_tab -= 1;
        }
        // Swap new active tab's panel state into the panel if needed
        if was_active {
            let incoming = std::mem::replace(
                &mut self.tabs[self.active_tab].panel_state,
                PanelState::new(),
            );
            self.agent_panel.update(cx, |panel, cx| {
                panel.swap_state(incoming, cx);
            });
        }
        // Show and focus new active tab's ghostty views
        for terminal in self.tabs[self.active_tab].pane_tree.all_terminals() {
            terminal.ensure_surface(window, cx);
        }
        self.sync_active_tab_native_view_visibility(cx);
        cx.on_next_frame(window, move |_workspace, window, cx| {
            for terminal in &closing_terminals {
                terminal.shutdown_surface(Some(&mut *window), cx);
            }
        });
        let focused = self.tabs[self.active_tab].pane_tree.try_focused_terminal();
        if let Some(focused) = focused {
            focused.focus(window, cx);
            self.sync_active_terminal_focus_states(cx);
            Self::schedule_terminal_bootstrap_reassert(
                focused,
                true,
                self.window_handle,
                self.workspace_handle.clone(),
                cx,
            );
        } else {
            self.sync_active_terminal_focus_states(cx);
        }
        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn close_tab_by_id(
        &mut self,
        tab_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.tab_index_for_summary_id(tab_id) {
            self.close_tab_by_index(index, window, cx);
        }
    }

    pub(super) fn destroy_quick_terminal_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(target_os = "macos")]
        crate::quick_terminal::force_hide();
        #[cfg(target_os = "macos")]
        crate::quick_terminal::reset_destroyed_window();
        self.close_window_from_last_tab(window, cx);
    }

    pub(super) fn close_window_from_last_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.defer_in(window, |workspace, window, cx| {
            workspace.prepare_window_close(cx);
            let should_quit = cfg!(not(target_os = "macos")) && cx.windows().len() <= 1;
            window.remove_window();
            if should_quit {
                cx.quit();
            }
        });
    }

    pub(super) fn prepare_window_close(&mut self, cx: &mut Context<Self>) {
        if self.window_close_prepared {
            return;
        }
        self.window_close_prepared = true;

        self.cancel_all_sessions();
        self.flush_session_save(cx);

        if let Some(settings_window) = self.settings_window.take() {
            self.settings_window_panel = None;
            let _ = settings_window.update(cx, |_, window, _| {
                window.remove_window();
            });
        }

        for request in std::mem::take(&mut self.pending_window_control_requests) {
            match request {
                PendingWindowControlRequest::TabsNew { response_tx } => {
                    Self::send_control_result(
                        response_tx,
                        Err(ControlError::internal(
                            "window closed while tabs.new was pending".to_string(),
                        )),
                    );
                }
                PendingWindowControlRequest::TabsClose { response_tx, .. } => {
                    Self::send_control_result(
                        response_tx,
                        Err(ControlError::internal(
                            "window closed while tabs.close was pending".to_string(),
                        )),
                    );
                }
            }
        }

        for (tab_idx, pending) in std::mem::take(&mut self.pending_control_agent_requests) {
            Self::send_control_result(
                pending.response_tx,
                Err(ControlError::internal(format!(
                    "window closed while agent.ask was still pending for tab {}",
                    tab_idx + 1
                ))),
            );
        }

        for request in std::mem::take(&mut self.pending_surface_control_requests) {
            let response_tx = match request {
                PendingSurfaceControlRequest::Create { response_tx, .. }
                | PendingSurfaceControlRequest::Split { response_tx, .. }
                | PendingSurfaceControlRequest::Close { response_tx, .. } => response_tx,
            };
            Self::send_control_result(
                response_tx,
                Err(ControlError::internal(
                    "window closed while a surface control request was pending".to_string(),
                )),
            );
        }

        for tab in &self.tabs {
            let conv = tab.session.conversation();
            let _ = conv.lock().save();

            for terminal in tab.pane_tree.all_surface_terminals() {
                // The platform window is already closing and will release its
                // sprite atlas; no current Window handle is available here.
                terminal.shutdown_surface(None, cx);
            }
        }
    }

    pub(super) fn reindex_pending_control_agent_requests_after_tab_close(
        &mut self,
        closed_tab_idx: usize,
    ) {
        let mut shifted = HashMap::new();
        for (tab_idx, pending) in std::mem::take(&mut self.pending_control_agent_requests) {
            if tab_idx == closed_tab_idx {
                Self::send_control_result(
                    pending.response_tx,
                    Err(ControlError::internal(format!(
                        "Tab {} was closed while agent.ask was still pending",
                        closed_tab_idx + 1
                    ))),
                );
                continue;
            }

            let next_idx = if tab_idx > closed_tab_idx {
                tab_idx - 1
            } else {
                tab_idx
            };
            shifted.insert(next_idx, pending);
        }
        self.pending_control_agent_requests = shifted;
    }

    pub(super) fn reindex_pending_surface_control_requests_after_tab_close(
        &mut self,
        closed_tab_idx: usize,
    ) {
        let mut shifted = Vec::new();
        for mut request in std::mem::take(&mut self.pending_surface_control_requests) {
            let tab_idx = match &mut request {
                PendingSurfaceControlRequest::Create { tab_idx, .. }
                | PendingSurfaceControlRequest::Split { tab_idx, .. }
                | PendingSurfaceControlRequest::Close { tab_idx, .. } => tab_idx,
            };

            if *tab_idx == closed_tab_idx {
                let (method, response_tx) = match request {
                    PendingSurfaceControlRequest::Create { response_tx, .. } => {
                        ("surfaces.create", response_tx)
                    }
                    PendingSurfaceControlRequest::Split { response_tx, .. } => {
                        ("surfaces.split", response_tx)
                    }
                    PendingSurfaceControlRequest::Close { response_tx, .. } => {
                        ("surfaces.close", response_tx)
                    }
                };
                Self::send_control_result(
                    response_tx,
                    Err(ControlError::internal(format!(
                        "Tab {} was closed while {method} was pending",
                        closed_tab_idx + 1
                    ))),
                );
                continue;
            }

            if *tab_idx > closed_tab_idx {
                *tab_idx -= 1;
            }
            shifted.push(request);
        }
        self.pending_surface_control_requests = shifted;
    }
}
