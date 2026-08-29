use super::*;

fn sanitize_tab_accent_alpha(alpha: f32) -> f32 {
    if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        crate::tab_colors::TAB_ACCENT_INACTIVE_ALPHA
    }
}

impl ConWorkspace {
    pub(super) fn on_sidebar_select(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarSelect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let closed_tools_panel = self.vertical_tabs_enabled() && self.sidebar_tools_open;
        if self.vertical_tabs_enabled() && self.sidebar_tools_open {
            self.sidebar_tools_open = false;
            self.activity_bar.update(cx, |bar, cx| {
                bar.left_panel_open = false;
                cx.notify();
            });
        }
        self.activate_tab(event.index, window, cx);
        if closed_tools_panel {
            self.save_session(cx);
            cx.notify();
        }
    }

    pub(super) fn on_sidebar_new_session(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        _event: &NewSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_tab(&NewTab, window, cx);
    }

    pub(super) fn on_sidebar_close_tab(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarCloseTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == event.session_id)
        else {
            return;
        };
        self.close_tab_by_index(index, window, cx);
    }

    pub(super) fn on_sidebar_rename(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarRename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let new_label = event.label.as_deref().and_then(normalize_tab_user_label);
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == event.session_id)
        else {
            return;
        };
        if !event.changed_by_user {
            self.refocus_active_terminal(window, cx);
            return;
        }
        if self.tabs[index].user_label == new_label {
            self.refocus_active_terminal(window, cx);
            return;
        }
        self.tabs[index].user_label = new_label;
        self.sync_sidebar(cx);
        self.save_session(cx);
        self.refocus_active_terminal(window, cx);
        cx.notify();
    }

    pub(super) fn on_sidebar_duplicate(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarDuplicate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == event.session_id)
        else {
            return;
        };
        self.duplicate_tab(index, window, cx);
    }

    pub(super) fn on_sidebar_reorder(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarReorder,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(from) = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == event.session_id)
        else {
            return;
        };
        // Sidebar emits `to` as a *slot* in `0..=tabs.len()`:
        //   slot K with K < tabs.len() == "insert before row K"
        //   slot tabs.len()             == "after the last row"
        // After `Vec::remove(from)` shifts every subsequent index
        // down by one, the resulting insert index is:
        //   from < to → to - 1 (the slot moved down with the rest)
        //   from > to → to     (the slot was above the source)
        //   from == to or from + 1 == to → no-op (drop on the same
        //     row's top half, or the slot just below — same place).
        let to = match event.move_delta {
            Some(delta) if delta < 0 => from.saturating_sub(1),
            Some(delta) if delta > 0 => (from + 2).min(self.tabs.len()),
            _ => event.to,
        };
        if from >= self.tabs.len() || to > self.tabs.len() {
            return;
        }
        if from == to || from + 1 == to {
            return;
        }
        let old_order: Vec<u64> = self.tabs.iter().map(|tab| tab.summary_id).collect();
        let active_id = self.tabs[self.active_tab].summary_id;
        let insert_at = if from < to { to - 1 } else { to };
        let tab = self.tabs.remove(from);
        // Vec::insert clamps via assert; insert_at is guaranteed
        // ≤ tabs.len() (post-remove) by construction above.
        self.tabs.insert(insert_at, tab);

        let new_positions: HashMap<u64, usize> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(idx, tab)| (tab.summary_id, idx))
            .collect();
        let mut remapped_pending = HashMap::new();
        for (old_idx, pending) in std::mem::take(&mut self.pending_control_agent_requests) {
            if let Some(summary_id) = old_order.get(old_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                remapped_pending.insert(new_idx, pending);
            }
        }
        self.pending_control_agent_requests = remapped_pending;

        for pending in &mut self.pending_create_pane_requests {
            if let Some(summary_id) = old_order.get(pending.tab_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                pending.tab_idx = new_idx;
            }
        }

        for pending in &mut self.pending_window_control_requests {
            if let PendingWindowControlRequest::TabsClose { tab_idx, .. } = pending
                && let Some(summary_id) = old_order.get(*tab_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                *tab_idx = new_idx;
            }
        }

        for pending in &mut self.pending_surface_control_requests {
            let tab_idx = match pending {
                PendingSurfaceControlRequest::Create { tab_idx, .. }
                | PendingSurfaceControlRequest::Split { tab_idx, .. }
                | PendingSurfaceControlRequest::Close { tab_idx, .. } => tab_idx,
            };
            if let Some(summary_id) = old_order.get(*tab_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                *tab_idx = new_idx;
            }
        }

        self.remap_tab_rename_state_after_reorder(&old_order);

        // Re-locate the active tab by stable summary_id rather than
        // index arithmetic (which had its own off-by-one in the
        // previous version).
        if let Some(new_active) = self.tabs.iter().position(|t| t.summary_id == active_id) {
            self.active_tab = new_active;
        }

        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn on_sidebar_pane_to_tab(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarPaneToTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        clear_pane_tab_promotion_drag_state(
            &mut self.pane_title_drag,
            &mut self.tab_strip_drop_slot,
            &mut self.tab_drag_target,
        );
        self.detach_pane_to_new_tab_at_slot(event.pane_id, event.to, window, cx);
    }

    /// Reorder a tab identified by `session_id` to drop slot `to`
    /// (0..=tabs.len()). Called from the horizontal tab strip drag/drop.
    pub(super) fn reorder_tab_by_id(&mut self, session_id: u64, to: usize, cx: &mut Context<Self>) {
        let Some(from) = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == session_id)
        else {
            return;
        };
        if from >= self.tabs.len() || to > self.tabs.len() {
            return;
        }
        if from == to || from + 1 == to {
            return;
        }
        let old_order: Vec<u64> = self.tabs.iter().map(|tab| tab.summary_id).collect();
        let active_id = self.tabs[self.active_tab].summary_id;
        let insert_at = if from < to { to - 1 } else { to };
        let tab = self.tabs.remove(from);
        self.tabs.insert(insert_at, tab);

        let new_positions: HashMap<u64, usize> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(idx, tab)| (tab.summary_id, idx))
            .collect();
        let mut remapped_pending = HashMap::new();
        for (old_idx, pending) in std::mem::take(&mut self.pending_control_agent_requests) {
            if let Some(summary_id) = old_order.get(old_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                remapped_pending.insert(new_idx, pending);
            }
        }
        self.pending_control_agent_requests = remapped_pending;

        for pending in &mut self.pending_create_pane_requests {
            if let Some(summary_id) = old_order.get(pending.tab_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                pending.tab_idx = new_idx;
            }
        }

        for pending in &mut self.pending_window_control_requests {
            if let PendingWindowControlRequest::TabsClose { tab_idx, .. } = pending
                && let Some(summary_id) = old_order.get(*tab_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                *tab_idx = new_idx;
            }
        }

        for pending in &mut self.pending_surface_control_requests {
            let tab_idx = match pending {
                PendingSurfaceControlRequest::Create { tab_idx, .. }
                | PendingSurfaceControlRequest::Split { tab_idx, .. }
                | PendingSurfaceControlRequest::Close { tab_idx, .. } => tab_idx,
            };
            if let Some(summary_id) = old_order.get(*tab_idx)
                && let Some(&new_idx) = new_positions.get(summary_id)
            {
                *tab_idx = new_idx;
            }
        }

        self.remap_tab_rename_state_after_reorder(&old_order);

        if let Some(new_active) = self.tabs.iter().position(|t| t.summary_id == active_id) {
            self.active_tab = new_active;
        }

        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    /// Begin an inline rename for the tab at `index` in the horizontal strip.
    /// Creates a local InputState that replaces the tab title in the strip.
    pub(super) fn begin_tab_rename(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let tab_id = tab.summary_id;
        let generation = self.tab_rename_generation;
        self.tab_rename_generation += 1;
        // Seed from the rendered tab label so focus→blur without edits
        // preserves AI/SSH/CWD-derived naming instead of materializing the
        // raw terminal title as a new explicit label.
        let is_editor_only = tab.pane_tree.pane_terminals().is_empty();
        let (hostname, title, current_dir) =
            if let Some(terminal) = tab.pane_tree.try_focused_terminal() {
                (
                    self.effective_remote_host_for_tab(index, terminal, cx),
                    terminal.title(cx),
                    terminal.current_dir(cx),
                )
            } else {
                (None, Some(tab.title.clone()), None)
            };
        let initial = tab_rename_initial_label(
            tab.user_label.as_deref(),
            tab.ai_label.as_deref(),
            tab.ai_icon.map(|kind| kind.svg_path()),
            hostname.as_deref(),
            title.as_deref(),
            current_dir.as_deref(),
            index,
            is_editor_only,
        );

        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_value(&initial, window, cx);
            state.set_placeholder("Tab name", window, cx);
            state
        });

        let select_all_on_focus = Rc::new(Cell::new(true));
        let changed_by_user = Rc::new(Cell::new(false));
        cx.subscribe_in(&input, window, {
            let select_all_on_focus = select_all_on_focus.clone();
            let changed_by_user = changed_by_user.clone();
            move |this, input_entity, event: &InputEvent, window, cx| match event {
                InputEvent::Focus if select_all_on_focus.replace(false) => {
                    window.dispatch_action(Box::new(gpui_component::input::SelectAll), cx);
                }
                InputEvent::Change => {
                    changed_by_user.set(true);
                }
                InputEvent::PressEnter { .. } | InputEvent::Blur => {
                    if this.tab_rename_cancelled_generation == Some(generation)
                        || this.tab_rename.as_ref().map(|rename| rename.generation)
                            != Some(generation)
                    {
                        if this.tab_rename_cancelled_generation == Some(generation) {
                            this.tab_rename_cancelled_generation = None;
                        }
                        return;
                    }
                    let value = input_entity.read(cx).value().to_string();
                    this.commit_tab_rename(tab_id, value, changed_by_user.get(), window, cx);
                }
                _ => {}
            }
        })
        .detach();

        self.tab_rename = Some(TabRenameEditor {
            tab_id,
            tab_index: index,
            generation,
            input: input.clone(),
        });
        self.tab_rename_cancelled_generation = None;
        input.update(cx, |state, cx| state.focus(window, cx));
        cx.notify();
    }

    pub(super) fn begin_tab_rename_by_id(
        &mut self,
        tab_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = self.tab_index_for_summary_id(tab_id) {
            self.begin_tab_rename(index, window, cx);
        }
    }

    pub(super) fn commit_tab_rename(
        &mut self,
        tab_id: u64,
        value: String,
        changed_by_user: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tab_index_for_summary_id(tab_id) else {
            self.tab_rename = None;
            self.tab_rename_cancelled_generation = None;
            cx.notify();
            return;
        };

        let Some(tab) = self.tabs.get_mut(index) else {
            self.tab_rename = None;
            self.tab_rename_cancelled_generation = None;
            cx.notify();
            return;
        };

        let Some(label) = tab_rename_commit_label(&value, changed_by_user) else {
            self.tab_rename = None;
            self.tab_rename_cancelled_generation = None;
            self.refocus_active_terminal(window, cx);
            cx.notify();
            return;
        };
        let changed = tab.user_label != label;
        tab.user_label = label;
        self.tab_rename = None;
        self.tab_rename_cancelled_generation = None;

        if changed {
            self.sync_sidebar(cx);
            self.save_session(cx);
        }
        self.refocus_active_terminal(window, cx);
        cx.notify();
    }

    pub(super) fn remap_tab_rename_state_after_close(&mut self, closed_index: usize) {
        self.tab_rename =
            self.tab_rename
                .take()
                .and_then(|state| match state.tab_index.cmp(&closed_index) {
                    std::cmp::Ordering::Less => Some(state),
                    std::cmp::Ordering::Equal => None,
                    std::cmp::Ordering::Greater => Some(TabRenameEditor {
                        tab_id: state.tab_id,
                        tab_index: state.tab_index - 1,
                        generation: state.generation,
                        input: state.input,
                    }),
                });
    }

    pub(super) fn remap_tab_rename_state_after_reorder(&mut self, old_order: &[u64]) {
        let rename_summary_id = self
            .tab_rename
            .as_ref()
            .and_then(|state| old_order.get(state.tab_index))
            .copied();
        if let Some(summary_id) = rename_summary_id
            && let Some(new_index) = self
                .tabs
                .iter()
                .position(|tab| tab.summary_id == summary_id)
            && let Some(state) = self.tab_rename.as_mut()
        {
            state.tab_index = new_index;
        }
    }

    pub(super) fn on_sidebar_close_others(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarCloseOthers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(keep) = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == event.session_id)
        else {
            return;
        };
        // Iterate from the end so indices stay stable as we close
        // tabs to the right of `keep`. After that, close everything
        // left of `keep` from the highest index downwards.
        let mut i = self.tabs.len();
        while i > keep + 1 {
            i -= 1;
            self.close_tab_by_index(i, window, cx);
        }
        let mut j = keep;
        while j > 0 {
            j -= 1;
            self.close_tab_by_index(j, window, cx);
        }
    }

    pub(super) fn on_sidebar_set_color(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarSetColor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == event.session_id)
        else {
            return;
        };
        self.set_tab_color(index, event.color, cx);
    }

    pub(super) fn on_sidebar_open_tool_slot(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        event: &SidebarOpenToolSlot,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activity_slot = event.slot;
        self.left_panel_open = true;
        self.sidebar_tools_open = true;
        self.activity_bar.update(cx, |bar, cx| {
            bar.active_slot = event.slot;
            bar.left_panel_open = true;
            cx.notify();
        });
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn focus_files_panel(
        &mut self,
        _: &FocusFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_activity_slot(ActivitySlot::Files, false, window, cx);
    }

    pub(super) fn search_files_panel(
        &mut self,
        _: &SearchFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_activity_slot(ActivitySlot::Search, true, window, cx);
    }

    fn show_activity_slot(
        &mut self,
        slot: ActivitySlot,
        focus_search: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activity_slot = slot;
        self.left_panel_open = true;
        self.sidebar_tools_open = true;
        self.activity_bar.update(cx, |bar, cx| {
            bar.active_slot = slot;
            bar.left_panel_open = true;
            cx.notify();
        });
        if focus_search {
            let search_view = self.search_view.clone();
            cx.on_next_frame(window, move |_workspace, window, cx| {
                search_view.update(cx, |search, cx| search.focus_query(window, cx));
            });
        } else {
            self.workspace_focus.clone().focus(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn on_sidebar_show_sessions(
        &mut self,
        _sidebar: &Entity<SessionSidebar>,
        _event: &SidebarShowSessions,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_tools_open = false;
        self.activity_bar.update(cx, |bar, cx| {
            bar.left_panel_open = false;
            cx.notify();
        });
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn sync_sidebar(&self, cx: &mut Context<Self>) {
        let sessions: Vec<SessionEntry> = self
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let is_editor_only = tab.pane_tree.pane_terminals().is_empty();
                let (hostname, title, current_dir) =
                    if let Some(terminal) = tab.pane_tree.try_focused_terminal() {
                        (
                            self.effective_remote_host_for_tab(i, terminal, cx),
                            terminal.title(cx),
                            terminal.current_dir(cx),
                        )
                    } else {
                        (None, Some(tab.title.clone()), None)
                    };
                let presentation = smart_tab_presentation(
                    tab.user_label.as_deref(),
                    tab.ai_label.as_deref(),
                    tab.ai_icon.map(|k| k.svg_path()),
                    hostname.as_deref(),
                    title.as_deref(),
                    current_dir.as_deref(),
                    i,
                    is_editor_only,
                );
                let pane_count = tab.pane_tree.pane_terminals().len();
                SessionEntry {
                    id: tab.summary_id,
                    name: presentation.name,
                    subtitle: presentation.subtitle,
                    is_ssh: presentation.is_ssh,
                    needs_attention: tab.needs_attention,
                    icon: presentation.icon,
                    has_user_label: tab.user_label.is_some(),
                    pane_count,
                    color: tab.color,
                }
            })
            .collect();
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.sync_sessions(sessions, self.active_tab, cx);
        });
    }

    pub(super) fn on_palette_select(
        &mut self,
        _palette: &Entity<CommandPalette>,
        event: &PaletteSelect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.action_id.as_str() {
            "new-window" => {
                cx.dispatch_action(&crate::NewWindow);
            }
            #[cfg(target_os = "macos")]
            "minimize-window" => {
                cx.dispatch_action(&crate::Minimize);
            }
            #[cfg(target_os = "macos")]
            "quick-terminal" => {
                cx.dispatch_action(&crate::ToggleQuickTerminal);
            }
            "toggle-agent" => {
                self.toggle_agent_panel(&ToggleAgentPanel, window, cx);
            }
            "settings" => {
                self.toggle_settings(&settings_panel::ToggleSettings, window, cx);
            }
            "new-tab" => {
                self.new_tab(&NewTab, window, cx);
            }
            "export-workspace-layout" => {
                self.export_workspace_layout(&ExportWorkspaceLayout, window, cx);
            }
            "add-workspace-layout-tabs" => {
                self.add_workspace_layout_tabs(&AddWorkspaceLayoutTabs, window, cx);
            }
            "open-workspace-layout-window" => {
                self.open_workspace_layout_window(&OpenWorkspaceLayoutWindow, window, cx);
            }
            "next-tab" => {
                self.next_tab(&NextTab, window, cx);
            }
            "previous-tab" => {
                self.previous_tab(&PreviousTab, window, cx);
            }
            "close-tab" => {
                self.close_tab(&CloseTab, window, cx);
            }
            "split-right" => {
                self.split_pane(
                    SplitDirection::Horizontal,
                    SplitPlacement::After,
                    window,
                    cx,
                );
            }
            "split-down" => {
                self.split_pane(SplitDirection::Vertical, SplitPlacement::After, window, cx);
            }
            "split-left" => {
                self.split_pane(
                    SplitDirection::Horizontal,
                    SplitPlacement::Before,
                    window,
                    cx,
                );
            }
            "split-up" => {
                self.split_pane(SplitDirection::Vertical, SplitPlacement::Before, window, cx);
            }
            "toggle-pane-zoom" => {
                self.toggle_pane_zoom(&TogglePaneZoom, window, cx);
            }
            "new-surface" => {
                self.create_surface_in_focused_pane(window, cx);
            }
            "new-surface-split-right" => {
                self.create_surface_split_from_focused_pane(
                    SplitDirection::Horizontal,
                    SplitPlacement::After,
                    window,
                    cx,
                );
            }
            "new-surface-split-down" => {
                self.create_surface_split_from_focused_pane(
                    SplitDirection::Vertical,
                    SplitPlacement::After,
                    window,
                    cx,
                );
            }
            "next-surface" => {
                self.cycle_surface_in_focused_pane(1, window, cx);
            }
            "previous-surface" => {
                self.cycle_surface_in_focused_pane(-1, window, cx);
            }
            "rename-surface" => {
                self.rename_current_surface(&RenameSurface, window, cx);
            }
            "close-surface" => {
                self.close_current_surface_in_focused_pane(window, cx);
            }
            "clear-terminal" => {
                if self.has_active_tab() {
                    if let Some(t) = self.try_active_terminal() {
                        t.clear_scrollback(cx);
                    }
                }
            }
            #[cfg(target_os = "macos")]
            "find-in-terminal" => {
                self.find_in_terminal(&FindInTerminal, window, cx);
            }
            "clear-restored-terminal-history" => {
                self.clear_restored_terminal_history(window, cx);
            }
            "focus-terminal" => {
                if self.has_active_tab() {
                    if let Some(t) = self.try_active_terminal() {
                        t.focus(window, cx);
                    }
                }
            }
            "toggle-input-bar" => {
                self.toggle_input_bar(&crate::ToggleInputBar, window, cx);
            }
            "toggle-left-sidebar" => {
                self.toggle_left_panel(&ToggleLeftPanel, window, cx);
            }
            "focus-files" => {
                self.show_activity_slot(ActivitySlot::Files, false, window, cx);
            }
            "search-files" => {
                self.show_activity_slot(ActivitySlot::Search, true, window, cx);
            }
            "collapse-sidebar" => {
                self.collapse_sidebar(&CollapseSidebar, window, cx);
            }
            "cycle-input-mode" => {
                self.input_bar.update(cx, |bar, cx| {
                    bar.cycle_mode(window, cx);
                });
                self.sync_active_terminal_focus_states(cx);
            }
            "check-for-updates" => {
                cx.dispatch_action(&crate::CheckForUpdates);
            }
            "quit" => {
                self.quit(&Quit, window, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn on_palette_dismissed(
        &mut self,
        _palette: &Entity<CommandPalette>,
        _event: &PaletteDismissed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restore_terminal_focus_after_modal(window, cx);
    }

    pub(super) fn on_settings_saved(
        &mut self,
        settings: &Entity<SettingsPanel>,
        _event: &SaveSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_settings_from_panel(settings, window, cx, true);
    }

    pub(super) fn apply_settings_from_panel(
        &mut self,
        settings: &Entity<SettingsPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
        restore_focus: bool,
    ) {
        let full_config = settings.read(cx).config().clone();
        let restore_terminal_text_was_enabled = self.config.appearance.restore_terminal_text;
        let old_keybindings = self.config.keybindings.clone();
        self.config = full_config.clone();
        if !self.vertical_tabs_enabled() {
            self.sidebar_tools_open = self.left_panel_open;
        }
        let new_agent_config = full_config.agent.clone();
        let auto_approve = new_agent_config.auto_approve_tools;
        self.harness.update_config(new_agent_config);
        self.shell_suggestion_engine = self.harness.suggestion_engine(180);
        self.shell_suggestion_engine.clear_cache();
        // Same suggestion model drives the tab summarizer; rebuild
        // it so the new credentials / model override take effect.
        self.tab_summary_engine = self
            .harness
            .tab_summary_engine()
            .with_state_from(&self.tab_summary_engine);
        self.tab_summary_generation = self.tab_summary_generation.wrapping_add(1);
        self.tab_summary_engine.clear_success_cache();
        for tab in &mut self.tabs {
            tab.ai_label = None;
            tab.ai_icon = None;
        }
        self.sync_sidebar(cx);
        if self.harness.config().suggestion_model.enabled {
            // Re-ask for fresh summaries with the new model.
            self.request_tab_summaries(cx);
        } else {
            for tab in &mut self.tabs {
                tab.ai_label = None;
                tab.ai_icon = None;
            }
            self.sync_sidebar(cx);
        }
        let active_agent_config = self.active_tab_agent_config();
        let active_agent_models = self.provider_models_for_config(&active_agent_config);

        // Sync auto-approve to agent panel UI
        self.agent_panel.update(cx, |panel, cx| {
            panel.set_auto_approve(auto_approve);
            panel.set_session_provider_options(
                AgentPanel::configured_session_providers(&active_agent_config),
                window,
                cx,
            );
            panel.set_provider_name(active_agent_config.provider.clone(), window, cx);
            panel.set_model_name(AgentHarness::active_model_name_for(&active_agent_config));
            panel.set_session_model_options(active_agent_models, window, cx);
        });

        // Apply updated skills paths (forces rescan on next cwd check)
        let skills_config = full_config.skills.clone();
        self.harness.update_skills_config(skills_config);
        if self.has_active_tab() {
            if let Some(cwd) = self.try_active_terminal().and_then(|t| t.current_dir(cx)) {
                self.request_skill_scan_for_cwd(&cwd);
            }
        }

        // Note: network/proxy config changes take effect on next app restart.
        // apply_to_env() is unsafe (requires single-threaded startup context)
        // and must not be called here while background threads are active.

        let term_config = full_config.terminal.clone();
        let appearance_config = full_config.appearance.clone();
        self.apply_terminal_and_ui_appearance(&term_config, &appearance_config, window, cx);
        self.sync_tab_strip_motion();
        if restore_terminal_text_was_enabled && !appearance_config.restore_terminal_text {
            self.save_session(cx);
        }

        // Re-apply keybindings at runtime so changes take effect immediately
        let kb = full_config.keybindings.clone();
        crate::rebind_app_keybindings(cx, &old_keybindings, &kb);
        #[cfg(target_os = "macos")]
        crate::global_hotkey::update_from_keybindings(&kb);

        if restore_focus {
            self.focus_terminal(window, cx);
        }
    }

    pub(super) fn on_appearance_preview(
        &mut self,
        settings: &Entity<SettingsPanel>,
        _event: &AppearancePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_appearance_preview_from_panel(settings, window, cx);
    }

    pub(super) fn apply_appearance_preview_from_panel(
        &mut self,
        settings: &Entity<SettingsPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let term_config = settings.read(cx).terminal_config().clone();
        let appearance_config = settings.read(cx).appearance_config().clone();
        self.apply_terminal_and_ui_appearance(&term_config, &appearance_config, window, cx);
        cx.notify();
    }

    pub(super) fn on_theme_preview(
        &mut self,
        _settings: &Entity<SettingsPanel>,
        event: &ThemePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_theme_preview(&event.0, window, cx);
    }

    pub(super) fn apply_theme_preview(
        &mut self,
        theme_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(new_theme) = TerminalTheme::by_name(theme_name) {
            if new_theme.name != self.terminal_theme.name {
                self.apply_terminal_theme(new_theme, window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn apply_terminal_and_ui_appearance(
        &mut self,
        term_config: &TerminalConfig,
        appearance_config: &AppearanceConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next_terminal_font_family = sanitize_terminal_font_family(&term_config.font_family);
        let next_ui_font_family = appearance_config.ui_font_family.clone();
        let next_ui_font_size = appearance_config.ui_font_size;
        let next_font_size = term_config.font_size;
        let next_terminal_cursor_style = term_config.cursor_style.clone();
        let next_terminal_opacity =
            Self::effective_terminal_opacity(appearance_config.terminal_opacity);
        let next_terminal_blur = Self::effective_terminal_blur(appearance_config.terminal_blur);
        let next_background_image = appearance_config.background_image.clone();
        let next_background_image_opacity =
            Self::clamp_background_image_opacity(appearance_config.background_image_opacity);
        let next_background_image_position = appearance_config.background_image_position.clone();
        let next_background_image_fit = appearance_config.background_image_fit.clone();
        let next_background_image_repeat = appearance_config.background_image_repeat;
        let next_tabs_orientation = appearance_config.tabs_orientation;

        let font_changed = self.terminal_font_family != next_terminal_font_family
            || (self.font_size - next_font_size).abs() > f32::EPSILON;
        let terminal_appearance_changed = font_changed
            || self.terminal_cursor_style != next_terminal_cursor_style
            || (self.terminal_opacity - next_terminal_opacity).abs() > f32::EPSILON
            || self.terminal_blur != next_terminal_blur
            || self.background_image != next_background_image
            || (self.background_image_opacity - next_background_image_opacity).abs() > f32::EPSILON
            || self.background_image_position != next_background_image_position
            || self.background_image_fit != next_background_image_fit
            || self.background_image_repeat != next_background_image_repeat;
        let ui_theme_changed = font_changed
            || self.ui_font_family != next_ui_font_family
            || (self.ui_font_size - next_ui_font_size).abs() > f32::EPSILON;

        self.terminal_font_family = next_terminal_font_family;
        self.ui_font_family = next_ui_font_family;
        self.ui_font_size = next_ui_font_size;
        self.font_size = next_font_size;
        self.config.appearance.tabs_orientation = next_tabs_orientation;
        if !self.vertical_tabs_enabled() {
            self.sidebar_tools_open = self.left_panel_open;
        }
        self.activity_bar.update(cx, |bar, cx| {
            bar.left_panel_open = if self.vertical_tabs_enabled() {
                self.sidebar_tools_open
            } else {
                self.left_panel_open
            };
            cx.notify();
        });
        self.sync_tab_strip_motion();
        if font_changed {
            let editor_views = self
                .tabs
                .iter()
                .flat_map(|tab| tab.pane_tree.editor_views())
                .collect::<Vec<_>>();
            for editor_view in editor_views {
                editor_view.update(cx, |editor, cx| editor.set_font_size(next_font_size, cx));
            }
        }
        self.terminal_cursor_style = next_terminal_cursor_style;
        self.terminal_opacity = next_terminal_opacity;
        self.terminal_blur = next_terminal_blur;
        self.ui_opacity = Self::clamp_ui_opacity(appearance_config.ui_opacity);
        self.tab_accent_inactive_alpha =
            sanitize_tab_accent_alpha(appearance_config.tab_accent_inactive_alpha);
        self.tab_accent_inactive_hover_alpha =
            sanitize_tab_accent_alpha(appearance_config.tab_accent_inactive_hover_alpha)
                .max(self.tab_accent_inactive_alpha);
        self.background_image = next_background_image;
        self.background_image_opacity = next_background_image_opacity;
        self.background_image_position = next_background_image_position;
        self.background_image_fit = next_background_image_fit;
        self.background_image_repeat = next_background_image_repeat;

        let effective_ui_opacity = Self::effective_ui_opacity(self.ui_opacity);
        self.agent_panel
            .update(cx, |panel, _cx| panel.set_ui_opacity(effective_ui_opacity));
        self.input_bar
            .update(cx, |bar, _cx| bar.set_ui_opacity(effective_ui_opacity));
        self.sidebar
            .update(cx, |s, cx| s.set_ui_opacity(effective_ui_opacity, cx));
        self.sidebar.update(cx, |s, cx| {
            s.set_tab_accent_alphas(
                self.tab_accent_inactive_alpha,
                self.tab_accent_inactive_hover_alpha,
                cx,
            )
        });
        self.command_palette.update(cx, |palette, _cx| {
            palette.set_ui_opacity(effective_ui_opacity)
        });

        if let Some(new_theme) = TerminalTheme::by_name(&term_config.theme) {
            let theme_changed = new_theme.name != self.terminal_theme.name;
            if theme_changed {
                self.terminal_theme = new_theme.clone();
            }
            if theme_changed || terminal_appearance_changed {
                self.sync_terminal_surface_appearance(&new_theme, window, cx);
            }
            if theme_changed || ui_theme_changed {
                self.sync_gpui_theme_appearance(&new_theme, window, cx);
            }
        } else {
            log::warn!(
                "Skipping terminal theme sync; theme {:?} was not found",
                term_config.theme
            );
        }

        let next_hide_pane_title_bar = appearance_config.hide_pane_title_bar;
        if self.hide_pane_title_bar != next_hide_pane_title_bar {
            self.hide_pane_title_bar = next_hide_pane_title_bar;
            cx.notify();
        }

        self.app_icon = sanitize_app_icon(&appearance_config.app_icon);
    }

    /// Apply a new terminal theme to all panes and sync UI mode.
    pub(super) fn apply_terminal_theme(
        &mut self,
        theme: TerminalTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_theme = theme.clone();
        self.sync_terminal_surface_appearance(&theme, window, cx);
        self.sync_gpui_theme_appearance(&theme, window, cx);
    }

    pub(super) fn sync_terminal_surface_appearance(
        &self,
        theme: &TerminalTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let colors = theme_to_ghostty_colors(theme);
        // Update all terminal panes (legacy gets full theme, ghostty gets color scheme)
        for tab in &self.tabs {
            for terminal in tab.pane_tree.all_surface_terminals() {
                terminal.set_theme(
                    theme,
                    &colors,
                    &self.terminal_font_family,
                    self.font_size,
                    self.terminal_opacity,
                    self.terminal_blur,
                    &self.terminal_cursor_style,
                    self.background_image.as_deref(),
                    self.background_image_opacity,
                    Some(&self.background_image_position),
                    Some(&self.background_image_fit),
                    self.background_image_repeat,
                    cx,
                );
            }
        }
        if let Err(e) = self.ghostty_app.update_appearance(
            &colors,
            &self.terminal_font_family,
            self.font_size,
            self.terminal_opacity,
            self.terminal_blur,
            &self.terminal_cursor_style,
            self.background_image.as_deref(),
            self.background_image_opacity,
            Some(&self.background_image_position),
            Some(&self.background_image_fit),
            self.background_image_repeat,
        ) {
            log::error!("Failed to update Ghostty appearance: {}", e);
        }
        for tab in &self.tabs {
            for terminal in tab.pane_tree.all_surface_terminals() {
                terminal.sync_window_background_blur(cx);
            }
        }
        #[cfg(target_os = "windows")]
        crate::set_windows_backdrop_blur(_window, self.terminal_blur);
        #[cfg(target_os = "macos")]
        crate::set_macos_window_glass_backdrop(_window, self.terminal_blur, self.terminal_opacity);
        #[cfg(target_os = "linux")]
        crate::set_linux_window_blur(_window, self.terminal_blur);
    }

    pub(super) fn sync_gpui_theme_appearance(
        &self,
        theme: &TerminalTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Sync GPUI UI theme colors with terminal theme
        crate::theme::sync_gpui_theme(
            theme,
            &self.terminal_font_family,
            &self.ui_font_family,
            self.ui_font_size,
            window,
            cx,
        );
    }
}
