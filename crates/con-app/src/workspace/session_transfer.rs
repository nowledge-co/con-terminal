//! Dragging changes layout ownership, never terminal/process ownership.
use super::*;

/// Native shell integration retains its original socket address and terminal
/// context across a move. Route that exact context to its current owner.
#[derive(Default)]
pub(super) struct TransferredTerminalOwners {
    pub terminals: HashMap<uuid::Uuid, WeakEntity<ConWorkspace>>,
    pub workspaces: Vec<WeakEntity<ConWorkspace>>,
}
impl Global for TransferredTerminalOwners {}

impl ConWorkspace {
    pub(super) fn handoff_transfer_control_socket(&mut self, cx: &mut Context<Self>) {
        if self.control_socket.is_none() {
            return;
        }
        let owners = cx
            .try_global::<TransferredTerminalOwners>()
            .map(|r| r.workspaces.clone())
            .unwrap_or_default();
        for owner in owners {
            if owner.entity_id() == cx.entity_id() {
                continue;
            }
            let handed = owner
                .update(cx, |destination, _| {
                    if destination.window_close_prepared || destination.control_socket.is_some() {
                        return false;
                    }
                    destination.control_socket = self.control_socket.take();
                    destination.control_request_rx =
                        std::mem::replace(&mut self.control_request_rx, crossbeam_channel::never());
                    true
                })
                .unwrap_or(false);
            if handed {
                // The closing source flushes its own snapshot synchronously.
                // Persist the surviving destination afterwards, not an empty
                // replacement tab from the now-closed source window.
                cx.defer(move |cx| {
                    let _ = owner.update(cx, |destination, cx| destination.flush_session_save(cx));
                });
                break;
            }
        }
    }

    pub(super) fn clear_session_drag(&mut self, cx: &mut Context<Self>) {
        self.session_drag = None;
        self.session_drag_hover = None;
        if cx
            .try_global::<crate::sidebar::ActiveSessionDrag>()
            .and_then(|d| d.0.as_ref())
            .is_some_and(|d| d.source_window.get() == Some(self.window_handle.window_id()))
        {
            cx.set_global(crate::sidebar::ActiveSessionDrag(None));
        }
        self.pane_title_drag = None;
        self.tab_strip_drop_slot = None;
        self.tab_drag_target = None;
        if let Ok(mut id) = self.active_dragged_tab_session_id.lock() {
            *id = None;
        }
        if let Ok(mut preview) = self.tab_drag_preview.lock() {
            *preview = None;
        }
        cx.notify();
    }

    pub(super) fn hover_drag_tab(
        &mut self,
        id: u64,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.session_drag.as_ref() else {
            return;
        };
        if drag.origin == DraggedTabOrigin::Pane
            || drag.session_id == id
            || self.tabs[self.active_tab].summary_id == id
            || self.session_drag_hover == Some(id)
        {
            return;
        }
        self.session_drag_hover = Some(id);
        cx.spawn_in(window, async move |workspace, window| {
            window
                .background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = window.update(|window, cx| {
                let _ = workspace.update(cx, |workspace, cx| {
                    if workspace.session_drag_hover == Some(id)
                        && cx.has_active_drag()
                        && bounds.contains(&window.mouse_position())
                    {
                        workspace.activate_tab_by_id(id, window, cx);
                    }
                });
            });
        })
        .detach();
    }

    fn transfer_is_ready(&self, index: usize, cx: &App) -> bool {
        if self.close_confirmation.is_some() || self.is_quick_terminal {
            return false;
        }
        let activity = if index == self.active_tab {
            self.agent_panel.read(cx).state().activity()
        } else {
            self.tabs[index].panel_state.activity()
        };
        // Built-in requests carry window-bound approval/execution callbacks.
        // Never silently rehome those callbacks while they are running.
        activity <= con_core::terminal_status::Activity::Idle
            && self.pending_create_pane_requests.is_empty()
            && self.pending_surface_control_requests.is_empty()
            && self.pending_window_control_requests.is_empty()
            && !self.pending_control_agent_requests.contains_key(&index)
    }

    fn transfer_notice(window: &mut Window, cx: &mut App, message: &'static str) {
        use gpui_component::{WindowExt, notification::Notification};
        window.push_notification(
            Notification::new()
                .title("Keep this session here")
                .message(message),
            cx,
        );
    }

    /// Take a tab without calling any close/termination path. Switch away first
    /// so the live built-in panel state travels with its conversation.
    fn take_transfer_tab(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Tab {
        if index == self.active_tab {
            if self.tabs.len() == 1 {
                self.new_tab(&NewTab, window, cx);
            } else {
                self.activate_tab(if index == 0 { 1 } else { index - 1 }, window, cx);
            }
        }
        self.reindex_pending_control_agent_requests_after_tab_close(index);
        self.reindex_pending_surface_control_requests_after_tab_close(index);
        self.remap_tab_rename_state_after_close(index);
        let tab = self.tabs.remove(index);
        self.tab_summary_engine.forget(tab.summary_id);
        if self.active_tab > index {
            self.active_tab -= 1;
        }
        self.sync_tab_strip_motion();
        tab
    }

    pub(super) fn tab_from_pane_tree(&mut self, tree: PaneTree) -> Tab {
        let summary_id = self.next_tab_summary_id;
        self.next_tab_summary_id += 1;
        Tab {
            pane_tree: tree,
            title: "Terminal".into(),
            user_label: None,
            ai_label: None,
            ai_icon: None,
            agent_cli: None,
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
        }
    }

    pub(super) fn take_pane_tab(&mut self, index: usize, pane: usize) -> Option<Tab> {
        let tree = self.tabs[index].pane_tree.take_pane(pane)?;
        let mut tab = self.tab_from_pane_tree(tree);
        if let Some(tracker) = self.tabs[index].runtime_trackers.borrow_mut().remove(&pane) {
            tab.runtime_trackers.get_mut().insert(pane, tracker);
        }
        if let Some(state) = self.tabs[index].runtime_cache.borrow_mut().remove(&pane) {
            tab.runtime_cache.get_mut().insert(pane, state);
        }
        if let Some(history) = self.tabs[index].shell_history.remove(&pane) {
            tab.shell_history.insert(pane, history);
        }
        Some(tab)
    }

    pub(super) fn finish_session_transfer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        self.mark_active_tab_terminal_native_layout_pending(cx);
        self.sync_tab_strip_motion();
        self.sync_active_tab_native_view_visibility(cx);
        self.sync_active_terminal_focus_states(cx);
        self.refocus_active_terminal(window, cx);
        self.sync_sidebar(cx);
        self.sync_file_tree_from_active_focus(cx);
        self.save_session(cx);
        self.clear_session_drag(cx);
    }

    pub(super) fn split_target_for_drag(
        &self,
        dragged: &DraggedTab,
        position: Point<Pixels>,
    ) -> Option<PaneSplitDropTarget> {
        let pane = if dragged.origin == DraggedTabOrigin::Pane {
            if self.tabs[self.active_tab].summary_id != dragged.session_id {
                return None;
            }
            dragged.pane_id?
        } else {
            if self.tabs[self.active_tab].summary_id == dragged.session_id {
                return None;
            }
            usize::MAX
        };
        let bounds = self.pane_content_bounds.lock().ok().and_then(|b| *b)?;
        let tree = &self.tabs[self.active_tab].pane_tree;
        pane_split_drop_target_from_position(pane, position, &tree.pane_bounds(bounds)).filter(
            |target| {
                dragged.origin != DraggedTabOrigin::Pane
                    || !tree.is_noop_pane_move(
                        pane,
                        target.target_pane_id,
                        target.direction,
                        target.placement,
                    )
            },
        )
    }

    pub(super) fn drop_tab_as_split(
        &mut self,
        dragged: &DraggedTab,
        target: PaneSplitDropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.tab_index_for_summary_id(dragged.session_id) else {
            return;
        };
        if source == self.active_tab || !self.transfer_is_ready(source, cx) {
            return;
        }
        // A pane has no separate built-in conversation panel. Keep a tab with
        // chat history intact rather than silently discard its identity.
        if !self.tabs[source]
            .session
            .conversation()
            .lock()
            .messages
            .is_empty()
        {
            Self::transfer_notice(
                window,
                cx,
                "This tab has a con conversation. Move it to a new window to keep the conversation intact.",
            );
            return;
        }
        if !self.tabs[self.active_tab]
            .pane_tree
            .pane_ids()
            .contains(&target.target_pane_id)
        {
            return;
        }
        let destination_id = self.tabs[self.active_tab].summary_id;
        let incoming = self.take_transfer_tab(source, window, cx);
        let destination = self
            .tab_index_for_summary_id(destination_id)
            .expect("destination remains");
        self.active_tab = destination;
        let Tab {
            pane_tree,
            runtime_trackers,
            runtime_cache,
            shell_history,
            ..
        } = incoming;
        let map = self.tabs[destination].pane_tree.insert_tree(
            target.target_pane_id,
            pane_tree,
            target.direction,
            target.placement,
        );
        let mut trackers = runtime_trackers.into_inner();
        let mut cache = runtime_cache.into_inner();
        let mut history = shell_history;
        for (old, new) in map {
            if let Some(v) = trackers.remove(&old) {
                self.tabs[destination]
                    .runtime_trackers
                    .borrow_mut()
                    .insert(new, v);
            }
            if let Some(v) = cache.remove(&old) {
                self.tabs[destination]
                    .runtime_cache
                    .borrow_mut()
                    .insert(new, v);
            }
            if let Some(v) = history.remove(&old) {
                self.tabs[destination].shell_history.insert(new, v);
            }
        }
        self.finish_session_transfer(window, cx);
    }

    pub(super) fn move_drag_to_window(
        &mut self,
        dragged: &DraggedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if dragged.source_window.get() != Some(window.window_handle().window_id()) {
            return;
        }
        let Some(index) = self.tab_index_for_summary_id(dragged.session_id) else {
            return;
        };
        if !self.transfer_is_ready(index, cx) {
            Self::transfer_notice(
                window,
                cx,
                "Wait for the con agent request or pending terminal operation to finish, then drag again.",
            );
            self.clear_session_drag(cx);
            return;
        }
        let pane = dragged
            .pane_id
            .filter(|_| dragged.origin == DraggedTabOrigin::Pane);
        let whole_tab = pane.is_none() || self.tabs[index].pane_tree.pane_count() == 1;
        if let Some(pane) = pane
            && !self.tabs[index].pane_tree.pane_ids().contains(&pane)
        {
            return;
        }
        let Ok((handle, destination)) = crate::open_con_transfer_window(self.config.clone(), cx)
        else {
            Self::transfer_notice(
                window,
                cx,
                "The new window could not open. Your session is still here.",
            );
            return;
        };
        let close_source = whole_tab && self.tabs.len() == 1;
        let incoming = if whole_tab {
            self.take_transfer_tab(index, window, cx)
        } else {
            self.take_pane_tab(index, pane.expect("pane transfer"))
                .expect("validated pane")
        };
        if close_source {
            self.flush_session_save(cx);
        }
        let origins = incoming
            .pane_tree
            .all_surface_terminals()
            .into_iter()
            .map(|terminal| terminal.native_agent.borrow().origin)
            .collect::<Vec<_>>();
        let claims = self
            .native_session_claims
            .iter()
            .filter(|(_, origin)| origins.contains(origin))
            .map(|(key, origin)| (key.clone(), *origin))
            .collect::<Vec<_>>();
        let mut pending = Some(incoming);
        let result = handle.update(cx, |_, destination_window, cx| {
            let incoming = pending.take().expect("transfer is consumed once");
            destination.update(cx, |workspace, cx| {
                // Initial terminal is still lazy; dispose it without spawning a shell.
                for tab in &workspace.tabs {
                    for terminal in tab.pane_tree.all_surface_terminals() {
                        terminal.shutdown_surface(Some(&mut *destination_window), cx);
                    }
                }
                let mut incoming = incoming;
                incoming.summary_id = workspace.next_tab_summary_id;
                workspace.next_tab_summary_id += 1;
                for terminal in incoming.pane_tree.all_surface_terminals() {
                    terminal.move_to_window(destination_window, cx);
                    subscribe_terminal_pane(terminal, destination_window, cx);
                }
                for editor in incoming.pane_tree.editor_views() {
                    workspace.subscribe_editor_view(&editor, destination_window, cx);
                }
                let panel = std::mem::replace(&mut incoming.panel_state, PanelState::new());
                workspace.agent_panel.update(cx, |panel_view, cx| {
                    panel_view.swap_state(panel, cx);
                });
                if !cx.has_global::<TransferredTerminalOwners>() {
                    cx.set_global(TransferredTerminalOwners::default());
                }
                let owner = cx.weak_entity();
                cx.global_mut::<TransferredTerminalOwners>()
                    .workspaces
                    .push(owner.clone());
                for origin in &origins {
                    cx.global_mut::<TransferredTerminalOwners>()
                        .terminals
                        .insert(*origin, owner.clone());
                }
                for (key, origin) in &claims {
                    workspace.native_session_claims.insert(key.clone(), *origin);
                }
                workspace.tabs = vec![incoming];
                workspace.active_tab = 0;
                workspace.finish_session_transfer(destination_window, cx);
                destination_window.activate_window();
            });
        });
        if result.is_err() {
            if let Some(tab) = pending {
                let index = self.tabs.len();
                self.tabs.push(tab);
                self.activate_tab(index, window, cx);
            }
            Self::transfer_notice(
                window,
                cx,
                "The new window became unavailable. Your session is still here.",
            );
            self.finish_session_transfer(window, cx);
            return;
        }
        for (key, _) in claims {
            self.native_session_claims.remove(&key);
        }
        if close_source {
            self.clear_session_drag(cx);
        } else {
            self.finish_session_transfer(window, cx);
        }
        if close_source {
            self.handoff_transfer_control_socket(cx);
            self.window_close_prepared = true;
            // Only the fresh replacement remains; migrated terminals are no
            // longer reachable from the closing workspace's shutdown path.
            for tab in &self.tabs {
                for terminal in tab.pane_tree.all_surface_terminals() {
                    terminal.shutdown_surface(Some(&mut *window), cx);
                }
            }
            window.remove_window();
        }
    }
}
