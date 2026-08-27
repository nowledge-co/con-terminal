use super::*;

impl ConWorkspace {
    pub(super) fn execute_shell(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        let target_ids = self.input_bar.read(cx).target_pane_ids();
        let pane_tree = &self.tabs[self.active_tab].pane_tree;
        let all_terminals = pane_tree.all_terminals();
        let mut history_records = Vec::new();

        for terminal in &all_terminals {
            if all_terminals.len() == 1
                || target_ids
                    .iter()
                    .any(|&tid| pane_tree.terminal_has_pane_id(terminal, tid))
            {
                terminal.write(format!("{}\n", cmd).as_bytes(), cx);
                if let Some(pane_id) = pane_tree.pane_id_for_terminal(terminal) {
                    history_records.push((pane_id, terminal.current_dir(cx)));
                }
            }
        }

        for (pane_id, cwd) in history_records {
            self.record_shell_command(self.active_tab, pane_id, cmd, cwd);
        }
        self.after_shell_command_recorded(cx);

        self.input_bar.update(cx, |bar, _cx| {
            bar.clear_completion_ui();
        });
        self.save_session(cx);

        // Always keep focus on input bar after sending a command —
        // the terminal output is visible, and the user can click to focus it.
        self.input_bar.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn send_to_agent(&mut self, content: &str, cx: &mut Context<Self>) {
        if !self.has_active_tab() {
            return;
        }
        self.record_input_history(content);
        if let Some(target) =
            super::chrome::agent_panel_motion_target_for_agent_request(self.agent_panel_open)
        {
            self.agent_panel_open = true;
            let duration = Self::terminal_adjacent_chrome_duration(true, 290, 220);
            #[cfg(target_os = "macos")]
            if !duration.is_zero() {
                self.arm_chrome_transition_underlay(duration + Duration::from_millis(80));
            }
            #[cfg(target_os = "macos")]
            if duration.is_zero() {
                self.arm_agent_panel_snap_guard(cx);
            }
            self.agent_panel_motion.set_target(target, duration);
        }
        self.agent_panel.update(cx, |panel, cx| {
            panel.add_message("user", content, cx);
        });
        let context = self.build_agent_context(cx);
        let session = &self.tabs[self.active_tab].session;
        let agent_config = self.active_tab_agent_config();
        self.harness
            .send_message(session, agent_config, content.to_string(), context);
        self.save_session(cx);
    }

    pub(super) fn create_surface_in_focused_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }

        let tab_idx = self.active_tab;
        let focused_terminal = self.tabs[tab_idx].pane_tree.try_visible_focus_terminal();
        let pane_id = focused_terminal
            .map(|(pane_id, _)| pane_id)
            .unwrap_or_else(|| self.tabs[tab_idx].pane_tree.focused_pane_id());
        let cwd = focused_terminal.and_then(|(_, terminal)| terminal.current_dir(cx));
        let terminal = self.create_terminal(cwd.as_deref(), window, cx);
        let next_surface_index = self.tabs[tab_idx]
            .pane_tree
            .surface_infos(Some(pane_id))
            .len()
            .saturating_add(1);
        let options = SurfaceCreateOptions::plain(Some(format!("Surface {next_surface_index}")));
        let Some(_surface_id) =
            self.tabs[tab_idx]
                .pane_tree
                .create_surface_in_pane(pane_id, terminal.clone(), options)
        else {
            return;
        };

        #[cfg(target_os = "macos")]
        self.mark_tab_terminal_native_layout_pending(tab_idx, cx);
        self.notify_tab_terminal_views(tab_idx, cx);
        terminal.ensure_surface(window, cx);
        terminal.focus(window, cx);
        self.sync_active_terminal_focus_states(cx);
        self.sync_active_tab_native_view_visibility_now_or_after_layout(false, window, cx);
        Self::schedule_terminal_bootstrap_reassert(
            &terminal,
            true,
            self.window_handle,
            self.workspace_handle.clone(),
            cx,
        );
        self.record_runtime_event_for_terminal(
            tab_idx,
            &terminal,
            con_agent::context::PaneRuntimeEvent::PaneCreated {
                startup_command: None,
            },
        );
        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn create_surface_split_from_focused_pane(
        &mut self,
        direction: SplitDirection,
        placement: SplitPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }

        let tab_idx = self.active_tab;
        let source_pane_id = self.tabs[tab_idx].pane_tree.focused_pane_id();
        let cwd = self.tabs[tab_idx]
            .pane_tree
            .try_focused_terminal()
            .and_then(|t| t.current_dir(cx));
        let terminal = self.create_terminal(cwd.as_deref(), window, cx);
        let was_zoomed = self.tabs[tab_idx].pane_tree.zoomed_pane_id().is_some();
        let options = SurfaceCreateOptions {
            title: Some("Surface 1".to_string()),
            owner: Some("command-palette".to_string()),
            close_pane_when_last: true,
        };
        let Some((_pane_id, _surface_id)) = self.tabs[tab_idx]
            .pane_tree
            .split_pane_with_surface_options(
                source_pane_id,
                direction,
                placement,
                terminal.clone(),
                options,
            )
        else {
            return;
        };

        #[cfg(target_os = "macos")]
        self.mark_tab_terminal_native_layout_pending(tab_idx, cx);
        self.notify_tab_terminal_views(tab_idx, cx);
        terminal.ensure_surface(window, cx);
        terminal.focus(window, cx);
        self.sync_active_terminal_focus_states(cx);
        self.sync_active_tab_native_view_visibility_now_or_after_layout(was_zoomed, window, cx);
        Self::schedule_terminal_bootstrap_reassert(
            &terminal,
            true,
            self.window_handle,
            self.workspace_handle.clone(),
            cx,
        );
        self.record_runtime_event_for_terminal(
            tab_idx,
            &terminal,
            con_agent::context::PaneRuntimeEvent::PaneCreated {
                startup_command: None,
            },
        );
        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn cycle_surface_in_focused_pane(
        &mut self,
        offset: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }

        let pane_tree = &self.tabs[self.active_tab].pane_tree;
        let pane_id = pane_tree.focused_pane_id();
        let surfaces = pane_tree.surface_infos(Some(pane_id));
        if surfaces.len() <= 1 {
            return;
        }

        let active_index = surfaces
            .iter()
            .position(|surface| surface.is_active)
            .unwrap_or(0);
        let len = surfaces.len() as isize;
        let next_index = (active_index as isize + offset).rem_euclid(len) as usize;
        let next_surface_id = surfaces[next_index].surface_id;
        self.focus_surface_in_active_tab(next_surface_id, window, cx);
    }

    pub(super) fn focused_active_surface_for_rename(
        &self,
        cx: &App,
    ) -> Option<(usize, usize, String)> {
        let tab_idx = self.active_tab;
        let tab = self.tabs.get(tab_idx)?;
        let pane_id = tab.pane_tree.focused_pane_id();
        let surface = tab
            .pane_tree
            .surface_infos(Some(pane_id))
            .into_iter()
            .find(|surface| surface.is_active)?;
        let title = surface
            .title
            .clone()
            .or_else(|| surface.terminal.title(cx))
            .unwrap_or_else(|| format!("Surface {}", surface.surface_index + 1));

        Some((tab_idx, surface.surface_id, title))
    }

    pub(super) fn rename_surface_title(
        &mut self,
        tab_idx: usize,
        surface_id: usize,
        value: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(tab_idx) else {
            return false;
        };
        let value = value.trim();
        let title = if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        };

        if !tab.pane_tree.rename_surface(surface_id, title) {
            return false;
        }

        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
        true
    }

    pub(super) fn begin_surface_rename(
        &mut self,
        surface_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }

        let tab_idx = self.active_tab;
        let Some(surface) = self.tabs[tab_idx]
            .pane_tree
            .surface_infos(None)
            .into_iter()
            .find(|surface| surface.surface_id == surface_id)
        else {
            return;
        };
        let initial = surface
            .title
            .clone()
            .unwrap_or_else(|| format!("Surface {}", surface.surface_index + 1));

        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(&initial, window, cx);
            state.set_placeholder("Surface name", window, cx);
            state
        });

        cx.subscribe_in(&input, window, {
            move |this, input_entity, event: &InputEvent, _window, cx| {
                if !matches!(event, InputEvent::PressEnter { .. }) {
                    return;
                }
                let value = input_entity.read(cx).value().to_string();
                this.rename_surface_title(tab_idx, surface_id, value, cx);
                this.surface_rename = None;
                cx.notify();
            }
        })
        .detach();

        self.surface_rename = Some(SurfaceRenameEditor {
            surface_id,
            input: input.clone(),
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(super) fn close_surface_by_id_in_active_tab(
        &mut self,
        surface_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }

        let tab_idx = self.active_tab;
        if self
            .surface_rename
            .as_ref()
            .is_some_and(|editor| editor.surface_id == surface_id)
        {
            self.surface_rename = None;
        }
        let Some(close_outcome) = self.tabs[tab_idx].pane_tree.close_surface(surface_id, true)
        else {
            return;
        };
        let closing = close_outcome.terminal.clone();
        closing.set_focus_state(false, cx);
        closing.set_native_view_visible(false, cx);
        closing.shutdown_surface(Some(window), cx);

        #[cfg(target_os = "macos")]
        self.mark_tab_terminal_native_layout_pending(tab_idx, cx);
        self.notify_tab_terminal_views(tab_idx, cx);
        self.sync_active_tab_native_view_visibility(cx);
        if let Some(replacement) = self.tabs[tab_idx].pane_tree.try_focused_terminal().cloned() {
            replacement.ensure_surface(window, cx);
            replacement.focus(window, cx);
        }
        self.sync_active_terminal_focus_states(cx);
        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn close_current_surface_in_focused_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }

        let tab_idx = self.active_tab;
        let pane_id = self.tabs[tab_idx].pane_tree.focused_pane_id();
        let Some(surface_id) = self.tabs[tab_idx]
            .pane_tree
            .surface_infos(Some(pane_id))
            .into_iter()
            .find(|surface| surface.is_active)
            .map(|surface| surface.surface_id)
        else {
            return;
        };

        self.close_surface_by_id_in_active_tab(surface_id, window, cx);
    }

    pub(super) fn split_pane(
        &mut self,
        direction: SplitDirection,
        placement: SplitPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }
        let cwd = self.try_active_terminal().and_then(|t| t.current_dir(cx));
        let terminal = self.create_terminal(cwd.as_deref(), window, cx);
        let was_zoomed = self.tabs[self.active_tab]
            .pane_tree
            .zoomed_pane_id()
            .is_some();
        self.tabs[self.active_tab].pane_tree.split_with_placement(
            direction,
            placement,
            terminal.clone(),
        );
        #[cfg(target_os = "macos")]
        self.mark_active_tab_terminal_native_layout_pending(cx);
        self.record_runtime_event_for_terminal(
            self.active_tab,
            &terminal,
            con_agent::context::PaneRuntimeEvent::PaneCreated {
                startup_command: None,
            },
        );
        self.notify_active_tab_terminal_views(cx);
        terminal.focus(window, cx);
        self.sync_active_terminal_focus_states(cx);
        self.sync_active_tab_native_view_visibility_now_or_after_layout(was_zoomed, window, cx);
        Self::schedule_terminal_bootstrap_reassert(
            &terminal,
            true,
            self.window_handle,
            self.workspace_handle.clone(),
            cx,
        );
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn split_right(
        &mut self,
        _: &SplitRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_tab() {
            self.split_pane(
                SplitDirection::Horizontal,
                SplitPlacement::After,
                window,
                cx,
            );
        }
    }

    pub(super) fn split_down(
        &mut self,
        _: &SplitDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_tab() {
            self.split_pane(SplitDirection::Vertical, SplitPlacement::After, window, cx);
        }
    }

    pub(super) fn split_left(
        &mut self,
        _: &SplitLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_tab() {
            self.split_pane(
                SplitDirection::Horizontal,
                SplitPlacement::Before,
                window,
                cx,
            );
        }
    }

    pub(super) fn split_up(&mut self, _: &SplitUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_active_tab() {
            self.split_pane(SplitDirection::Vertical, SplitPlacement::Before, window, cx);
        }
    }

    pub(super) fn clear_terminal(
        &mut self,
        _: &ClearTerminal,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.has_active_tab() {
            if let Some(t) = self.try_active_terminal() {
                t.clear_scrollback(cx);
            }
        }
    }

    pub(super) fn find_in_terminal(
        &mut self,
        _: &FindInTerminal,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        #[cfg(target_os = "macos")]
        if self.has_active_tab()
            && let Some(terminal) = self.try_active_terminal()
        {
            terminal.show_terminal_find(_window, _cx);
        }
    }

    pub(super) fn clear_restored_terminal_history_action(
        &mut self,
        _: &ClearRestoredTerminalHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_restored_terminal_history(window, cx);
    }

    pub(super) fn new_surface(
        &mut self,
        _: &NewSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_surface_in_focused_pane(window, cx);
    }

    pub(super) fn new_surface_split_right(
        &mut self,
        _: &NewSurfaceSplitRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_surface_split_from_focused_pane(
            SplitDirection::Horizontal,
            SplitPlacement::After,
            window,
            cx,
        );
    }

    pub(super) fn new_surface_split_down(
        &mut self,
        _: &NewSurfaceSplitDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_surface_split_from_focused_pane(
            SplitDirection::Vertical,
            SplitPlacement::After,
            window,
            cx,
        );
    }

    pub(super) fn next_surface(
        &mut self,
        _: &NextSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_surface_in_focused_pane(1, window, cx);
    }

    pub(super) fn previous_surface(
        &mut self,
        _: &PreviousSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cycle_surface_in_focused_pane(-1, window, cx);
    }

    pub(super) fn rename_current_surface(
        &mut self,
        _: &RenameSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_tab_idx, surface_id, _title)) = self.focused_active_surface_for_rename(cx)
        else {
            return;
        };
        self.begin_surface_rename(surface_id, window, cx);
    }

    pub(super) fn close_surface(
        &mut self,
        _: &CloseSurface,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_current_surface_in_focused_pane(window, cx);
    }

    pub(super) fn close_pane(
        &mut self,
        _: &ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }

        let pane_count = self.tabs[self.active_tab].pane_tree.pane_count();
        let focused_pane_id = self.tabs[self.active_tab]
            .pane_tree
            .active_editor_pane_id(window, cx)
            .unwrap_or_else(|| self.tabs[self.active_tab].pane_tree.focused_pane_id());
        let contains = self.tabs[self.active_tab]
            .pane_tree
            .contains_pane(focused_pane_id);

        let valid_pane_id = if contains {
            focused_pane_id
        } else {
            self.tabs[self.active_tab].pane_tree.first_pane_id_pub()
        };

        if pane_count > 1 {
            let _ = self.close_pane_in_tab(self.active_tab, valid_pane_id, window, cx);
            return;
        }

        if self.tabs.len() > 1 {
            if self.close_pane_in_tab(self.active_tab, valid_pane_id, window, cx) {
                return;
            }
            log::debug!(
                "[close_pane] single pane, closing tab (tabs={})",
                self.tabs.len()
            );
            self.close_tab_by_index(self.active_tab, window, cx);
            return;
        }

        if self.is_quick_terminal {
            self.destroy_quick_terminal_window(window, cx);
            return;
        }

        log::debug!("[close_pane] single pane, single tab — closing window");
        self.close_window_from_last_tab(window, cx);
    }

    pub(super) fn toggle_pane_zoom(
        &mut self,
        _: &TogglePaneZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_focused_pane_zoom(window, cx);
    }

    pub(super) fn toggle_focused_pane_zoom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.has_active_tab() {
            return;
        }

        let was_zoomed = self.tabs[self.active_tab]
            .pane_tree
            .zoomed_pane_id()
            .is_some();
        if !self.tabs[self.active_tab].pane_tree.toggle_zoom_focused() {
            return;
        }

        #[cfg(target_os = "macos")]
        self.mark_active_tab_terminal_native_layout_pending(cx);
        self.notify_active_tab_terminal_views(cx);
        // Only focus the terminal if the focused pane is a terminal pane.
        // For editor panes, skip terminal focus so the correct pane stays zoomed.
        let focused_pane_id = self.tabs[self.active_tab].pane_tree.focused_pane_id();
        let focused_is_terminal = self.tabs[self.active_tab]
            .pane_tree
            .pane_terminals()
            .iter()
            .any(|(id, _)| *id == focused_pane_id);
        if focused_is_terminal {
            if let Some(t) = self.try_active_terminal() {
                t.focus(window, cx);
            }
        }
        self.sync_active_terminal_focus_states(cx);
        self.sync_active_tab_native_view_visibility_now_or_after_layout(was_zoomed, window, cx);
        cx.notify();
    }

    /// Toggle zoom for a specific pane (called from the pane title bar options menu).
    pub(super) fn toggle_pane_zoom_for_pane(
        &mut self,
        pane_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }
        // Focus the target pane first so toggle_zoom_focused acts on it.
        self.tabs[self.active_tab].pane_tree.focus_pane(pane_id);
        self.toggle_focused_pane_zoom(window, cx);
    }

    pub(super) fn pane_drag_title_for(&self, pane_id: usize, cx: &Context<Self>) -> SharedString {
        self.tabs[self.active_tab]
            .pane_tree
            .pane_title(pane_id, cx)
            .unwrap_or_else(|| "Terminal".to_string())
            .into()
    }

    pub(super) fn update_pane_title_drag_state(
        &mut self,
        pane_id: usize,
        position: Point<Pixels>,
        target: Option<PaneDropTarget>,
        cx: &mut Context<Self>,
    ) {
        if let Some(ref mut drag) = self.pane_title_drag {
            drag.current_pos = position;
            drag.target = target;
        } else {
            self.pane_title_drag = Some(PaneTitleDragState {
                title: self.pane_drag_title_for(pane_id, cx),
                current_pos: position,
                active: true,
                target,
            });
        }
        cx.notify();
    }

    /// Detach a pane from the active tab's split tree and promote it to a new tab.
    pub(super) fn detach_pane_to_new_tab_at_slot(
        &mut self,
        pane_id: usize,
        requested_slot: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            return;
        }
        let tab_idx = self.active_tab;
        if self.tabs[tab_idx].pane_tree.pane_count() <= 1 {
            // Can't detach the only pane — nothing to do.
            return;
        }

        // Collect all surfaces for this pane before closing it.
        let surfaces = self.tabs[tab_idx].pane_tree.surface_infos(Some(pane_id));
        if surfaces.is_empty() {
            return;
        }

        // Separate active surface (first) from the rest.
        let active_surface = surfaces
            .iter()
            .find(|s| s.is_active)
            .or_else(|| surfaces.first())
            .cloned();
        let Some(active_surface) = active_surface else {
            return;
        };
        let other_surfaces: Vec<_> = surfaces
            .iter()
            .filter(|s| s.surface_id != active_surface.surface_id)
            .cloned()
            .collect();

        // Remove the pane from the split tree.
        if !self.remove_pane_in_tab(tab_idx, pane_id, window, cx, false) {
            return;
        }

        // Build a new tab with the active terminal as root.
        let tab_number = self.tabs.len() + 1;
        let summary_id = self.next_tab_summary_id;
        self.next_tab_summary_id += 1;
        let active_options = SurfaceCreateOptions {
            title: active_surface.title.clone(),
            owner: active_surface.owner.clone(),
            close_pane_when_last: active_surface.close_pane_when_last,
        };
        let mut new_pane_tree =
            PaneTree::new_with_surface_options(active_surface.terminal.clone(), active_options);

        // Re-add any additional surfaces into the new pane tree.
        for surface in &other_surfaces {
            let opts = SurfaceCreateOptions {
                title: surface.title.clone(),
                owner: surface.owner.clone(),
                close_pane_when_last: surface.close_pane_when_last,
            };
            let root_pane_id = new_pane_tree.focused_pane_id();
            let _ =
                new_pane_tree.create_surface_in_pane(root_pane_id, surface.terminal.clone(), opts);
        }

        let new_index = requested_slot.min(self.tabs.len());
        self.active_tab = rebase_active_tab_for_insert(self.active_tab, new_index);
        self.tabs.insert(
            new_index,
            Tab {
                pane_tree: new_pane_tree,
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
            },
        );

        if self.sync_tab_strip_motion() {
            #[cfg(target_os = "macos")]
            self.arm_top_chrome_snap_guard(cx);
            if Self::should_defer_top_chrome_refresh_when_tab_strip_appears() {
                cx.on_next_frame(window, |_, _, cx| {
                    cx.notify();
                });
            }
        }

        self.activate_tab(new_index, window, cx);
    }

    pub(super) fn close_pane_in_tab(
        &mut self,
        tab_idx: usize,
        pane_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.remove_pane_in_tab(tab_idx, pane_id, window, cx, true)
    }

    pub(super) fn remove_pane_in_tab(
        &mut self,
        tab_idx: usize,
        pane_id: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        shutdown_closing_terminals: bool,
    ) -> bool {
        if tab_idx >= self.tabs.len() {
            return false;
        }

        let pane_count = self.tabs[tab_idx].pane_tree.pane_count();
        let pane_tree = &mut self.tabs[tab_idx].pane_tree;

        if let Some(editor_view) = pane_tree.editor_view_for_pane(pane_id) {
            let editor_pane_empty = editor_view.update(cx, |editor, cx| {
                let editor_pane_empty = editor.close_active_tab();
                if editor.active_path().is_some() {
                    cx.emit(ActiveFileChanged);
                }
                cx.notify();
                editor_pane_empty
            });

            match editor_file_close_outcome(pane_count, editor_pane_empty) {
                EditorFileCloseOutcome::KeepEditorPane => {
                    if tab_idx == self.active_tab {
                        let focus_handle = editor_view.read(cx).focus_handle(cx).clone();
                        focus_handle.focus(window, cx);
                    }
                    cx.notify();
                    return true;
                }
                EditorFileCloseOutcome::ReplaceEditorPaneWithTerminal => {
                    let cwd = self
                        .file_tree_view
                        .read(cx)
                        .root()
                        .and_then(|root| root.to_str())
                        .map(str::to_string);
                    let terminal = self.create_terminal(cwd.as_deref(), window, cx);
                    self.tabs[tab_idx].pane_tree = PaneTree::new(terminal.clone());
                    if tab_idx == self.active_tab {
                        terminal.ensure_surface(window, cx);
                        terminal.focus(window, cx);
                        self.sync_active_terminal_focus_states(cx);
                        #[cfg(target_os = "macos")]
                        self.mark_active_tab_terminal_native_layout_pending(cx);
                    }
                    self.sync_sidebar(cx);
                    self.save_session(cx);
                    cx.notify();
                    return true;
                }
                EditorFileCloseOutcome::CloseEditorPane => {}
            }
        }

        if pane_count <= 1 {
            return false;
        }

        // Check if this is an editor pane (no terminal surfaces).
        // Editor panes close directly without terminal shutdown.
        let closing_terminals = pane_tree
            .surface_infos(Some(pane_id))
            .into_iter()
            .map(|surface| surface.terminal)
            .collect::<Vec<_>>();

        let is_editor_pane = closing_terminals.is_empty();

        if !pane_tree.close_pane(pane_id) {
            return false;
        }

        // Editor pane: no terminal lifecycle to manage, just re-focus.
        if is_editor_pane {
            let tab_is_visible = tab_idx == self.active_tab;
            if tab_is_visible {
                let surviving_terminals: Vec<TerminalPane> =
                    pane_tree.all_terminals().into_iter().cloned().collect();
                if let Some(terminal) = surviving_terminals.first() {
                    terminal.ensure_surface(window, cx);
                    terminal.focus(window, cx);
                } else {
                    // No terminals left — keep keyboard focus on workspace
                    self.workspace_focus.clone().focus(window, cx);
                }
                for terminal in &surviving_terminals {
                    terminal.notify(cx);
                }
                self.sync_active_terminal_focus_states(cx);
            }
            cx.notify();
            return true;
        }

        // Terminal pane: normal shutdown flow.
        let mut focus_after_close = None;
        let mut sync_visibility_after_close = false;
        {
            let pane_tree = &mut self.tabs[tab_idx].pane_tree;
            let surviving_terminals: Vec<TerminalPane> =
                pane_tree.all_terminals().into_iter().cloned().collect();
            let tab_is_visible = tab_idx == self.active_tab;
            if tab_is_visible {
                for terminal in &surviving_terminals {
                    terminal.ensure_surface(window, cx);
                    terminal.notify(cx);
                }
                sync_visibility_after_close = true;
            }

            if shutdown_closing_terminals {
                cx.on_next_frame(window, move |_workspace, window, cx| {
                    for terminal in &closing_terminals {
                        terminal.shutdown_surface(Some(&mut *window), cx);
                    }
                    if tab_is_visible {
                        for terminal in &surviving_terminals {
                            terminal.notify(cx);
                        }
                    }
                });
            } else if tab_is_visible {
                for terminal in &surviving_terminals {
                    terminal.notify(cx);
                }
            }

            if tab_idx == self.active_tab {
                // Only try to focus a terminal if one still exists.
                // The tab may now contain only editor panes.
                focus_after_close = self.tabs[tab_idx].pane_tree.try_focused_terminal().cloned();
            }
        }
        if tab_idx == self.active_tab {
            #[cfg(target_os = "macos")]
            self.mark_active_tab_terminal_native_layout_pending(cx);
        }

        if let Some(focused) = focus_after_close {
            if sync_visibility_after_close {
                self.sync_active_tab_native_view_visibility(cx);
            }
            focused.focus(window, cx);
            self.sync_active_terminal_focus_states(cx);
        } else if tab_idx == self.active_tab {
            // No terminal survived (editor-only tab) — keep keyboard focus on workspace
            // so Cmd+T, Cmd+W etc. still work without requiring a click.
            self.workspace_focus.clone().focus(window, cx);
            self.sync_active_terminal_focus_states(cx);
        }

        self.save_session(cx);
        cx.notify();
        true
    }
}
