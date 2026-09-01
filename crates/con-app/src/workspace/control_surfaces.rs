use super::*;

impl ConWorkspace {
    pub(super) fn flush_pending_window_control_requests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = std::mem::take(&mut self.pending_window_control_requests);
        if pending.is_empty() {
            return;
        }

        for request in pending {
            match request {
                PendingWindowControlRequest::TabsNew { response_tx } => {
                    self.new_tab(&NewTab, window, cx);
                    let result = Ok(json!({
                        "active_tab_index": self.active_tab + 1,
                        "tab_count": self.tabs.len(),
                        "focused_pane_id": self.tabs[self.active_tab].pane_tree.focused_pane_id(),
                    }));
                    Self::send_control_result(response_tx, result);
                }
                PendingWindowControlRequest::TabsClose {
                    tab_idx,
                    response_tx,
                } => {
                    if self.tabs.len() <= 1 {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(
                                "refusing to close the last tab over the control plane",
                            )),
                        );
                        continue;
                    }
                    if tab_idx >= self.tabs.len() {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(format!(
                                "Tab index {} is out of range. Valid tabs are 1..={}.",
                                tab_idx + 1,
                                self.tabs.len()
                            ))),
                        );
                        continue;
                    }
                    self.close_tab_by_index(tab_idx, window, cx);
                    let result = Ok(json!({
                        "active_tab_index": self.active_tab + 1,
                        "tab_count": self.tabs.len(),
                        "closed_tab_index": tab_idx + 1,
                    }));
                    Self::send_control_result(response_tx, result);
                }
            }
        }
    }

    pub(super) fn flush_pending_create_pane_requests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = std::mem::take(&mut self.pending_create_pane_requests);
        if pending.is_empty() {
            return;
        }

        let mut created_any = false;
        for req in pending {
            if req.tab_idx >= self.tabs.len() {
                let _ = req.response_tx.send(con_agent::PaneResponse::Error(format!(
                    "Tab {} is no longer available.",
                    req.tab_idx + 1
                )));
                continue;
            }

            let terminal = self.create_terminal(req.cwd.as_deref(), window, cx);
            let direction = match req.location {
                con_agent::tools::PaneCreateLocation::Right => SplitDirection::Horizontal,
                con_agent::tools::PaneCreateLocation::Down => SplitDirection::Vertical,
            };
            let was_zoomed = self.tabs[req.tab_idx].pane_tree.zoomed_pane_id().is_some();
            self.tabs[req.tab_idx]
                .pane_tree
                .split(direction, terminal.clone());
            let should_focus = req.tab_idx == self.active_tab;
            if should_focus {
                #[cfg(target_os = "macos")]
                self.mark_tab_terminal_native_layout_pending(req.tab_idx, cx);
                self.notify_tab_terminal_views(req.tab_idx, cx);
                terminal.focus(window, cx);
                self.sync_active_terminal_focus_states(cx);
                self.sync_active_tab_native_view_visibility_now_or_after_layout(
                    was_zoomed, window, cx,
                );
            } else {
                terminal.set_focus_state(false, cx);
            }
            Self::schedule_terminal_bootstrap_reassert(
                &terminal,
                should_focus,
                self.window_handle,
                self.workspace_handle.clone(),
                cx,
            );

            if let Some(cmd) = &req.command {
                let cmd_with_newline = format!("{}\n", cmd);
                terminal.write(cmd_with_newline.as_bytes(), cx);
            }
            self.record_runtime_event_for_terminal(
                req.tab_idx,
                &terminal,
                con_agent::context::PaneRuntimeEvent::PaneCreated {
                    startup_command: req.command.clone(),
                },
            );

            let pane_tree = &self.tabs[req.tab_idx].pane_tree;
            let pane_index = pane_tree
                .all_terminals()
                .iter()
                .enumerate()
                .find(|(_, pane)| pane.entity_id() == terminal.entity_id())
                .map(|(idx, _)| idx + 1)
                .unwrap_or_else(|| pane_tree.pane_count());
            let pane_id = pane_tree
                .pane_id_for_terminal(&terminal)
                .unwrap_or(pane_index);
            let _ = req.response_tx.send(con_agent::PaneResponse::PaneCreated {
                pane_index,
                pane_id,
                surface_ready: terminal.surface_ready(cx),
                is_alive: terminal.is_alive(cx),
                has_shell_integration: terminal.has_shell_integration(cx),
            });
            created_any = true;
        }

        if created_any {
            self.save_session(cx);
        }
        cx.notify();
        window.refresh();
    }

    pub(super) fn flush_pending_surface_control_requests(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pending = std::mem::take(&mut self.pending_surface_control_requests);
        if pending.is_empty() {
            return;
        }

        for request in pending {
            match request {
                PendingSurfaceControlRequest::Create {
                    tab_idx,
                    pane,
                    title,
                    command,
                    owner,
                    close_pane_when_last,
                    response_tx,
                } => {
                    if tab_idx >= self.tabs.len() {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(format!(
                                "Tab {} is no longer available.",
                                tab_idx + 1
                            ))),
                        );
                        continue;
                    }
                    let resolved = match self
                        .resolve_pane_target_for_tab(tab_idx, Self::pane_selector_from_target(pane))
                    {
                        Ok(resolved) => resolved,
                        Err(err) => {
                            Self::send_control_result(
                                response_tx,
                                Err(ControlError::invalid_params(err)),
                            );
                            continue;
                        }
                    };
                    let cwd = resolved.pane.current_dir(cx);
                    let terminal = self.create_terminal(cwd.as_deref(), window, cx);
                    let options = SurfaceCreateOptions {
                        title,
                        owner,
                        close_pane_when_last,
                    };
                    let Some(surface_id) = self.tabs[tab_idx].pane_tree.create_surface_in_pane(
                        resolved.pane_id,
                        terminal.clone(),
                        options,
                    ) else {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(format!(
                                "Pane id {} is no longer available in tab {}.",
                                resolved.pane_id,
                                tab_idx + 1
                            ))),
                        );
                        continue;
                    };
                    if tab_idx == self.active_tab {
                        #[cfg(target_os = "macos")]
                        self.mark_tab_terminal_native_layout_pending(tab_idx, cx);
                        self.notify_tab_terminal_views(tab_idx, cx);
                        self.sync_active_tab_native_view_visibility_now_or_after_layout(
                            false, window, cx,
                        );
                    }
                    self.finish_created_surface(
                        tab_idx,
                        resolved.pane_id,
                        surface_id,
                        terminal,
                        command,
                        false,
                        response_tx,
                        window,
                        cx,
                    );
                }
                PendingSurfaceControlRequest::Split {
                    tab_idx,
                    source,
                    location,
                    title,
                    command,
                    owner,
                    close_pane_when_last,
                    response_tx,
                } => {
                    if tab_idx >= self.tabs.len() {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(format!(
                                "Tab {} is no longer available.",
                                tab_idx + 1
                            ))),
                        );
                        continue;
                    }
                    let resolved = match self.resolve_surface_target_for_tab(tab_idx, source) {
                        Ok(resolved) => resolved,
                        Err(err) => {
                            Self::send_control_result(response_tx, Err(err));
                            continue;
                        }
                    };
                    let cwd = resolved.terminal.current_dir(cx);
                    let terminal = self.create_terminal(cwd.as_deref(), window, cx);
                    let direction = match location {
                        con_agent::tools::PaneCreateLocation::Right => SplitDirection::Horizontal,
                        con_agent::tools::PaneCreateLocation::Down => SplitDirection::Vertical,
                    };
                    let was_zoomed = self.tabs[tab_idx].pane_tree.zoomed_pane_id().is_some();
                    let options = SurfaceCreateOptions {
                        title,
                        owner: Some(owner.unwrap_or_else(|| "con-cli".to_string())),
                        close_pane_when_last,
                    };
                    let Some((pane_id, surface_id)) = self.tabs[tab_idx]
                        .pane_tree
                        .split_pane_with_surface_options(
                            resolved.pane_id,
                            direction,
                            SplitPlacement::After,
                            terminal.clone(),
                            options,
                        )
                    else {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(format!(
                                "{} is no longer available in tab {}.",
                                source.describe(),
                                tab_idx + 1
                            ))),
                        );
                        continue;
                    };
                    if tab_idx == self.active_tab {
                        #[cfg(target_os = "macos")]
                        self.mark_tab_terminal_native_layout_pending(tab_idx, cx);
                        self.notify_tab_terminal_views(tab_idx, cx);
                        self.sync_active_tab_native_view_visibility_now_or_after_layout(
                            was_zoomed, window, cx,
                        );
                    }
                    self.finish_created_surface(
                        tab_idx,
                        pane_id,
                        surface_id,
                        terminal,
                        command,
                        true,
                        response_tx,
                        window,
                        cx,
                    );
                }
                PendingSurfaceControlRequest::Close {
                    tab_idx,
                    target,
                    close_empty_owned_pane,
                    response_tx,
                } => {
                    if tab_idx >= self.tabs.len() {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(format!(
                                "Tab {} is no longer available.",
                                tab_idx + 1
                            ))),
                        );
                        continue;
                    }
                    let resolved = match self.resolve_surface_target_for_tab(tab_idx, target) {
                        Ok(resolved) => resolved,
                        Err(err) => {
                            Self::send_control_result(response_tx, Err(err));
                            continue;
                        }
                    };
                    let surfaces = self.tabs[tab_idx]
                        .pane_tree
                        .surface_infos(Some(resolved.pane_id));
                    let current = surfaces
                        .iter()
                        .find(|surface| surface.surface_id == resolved.surface_id)
                        .cloned();
                    let closing_was_focused = current
                        .as_ref()
                        .is_some_and(|surface| surface.is_active && surface.is_focused_pane);
                    let Some(close_outcome) = self.tabs[tab_idx]
                        .pane_tree
                        .close_surface(resolved.surface_id, close_empty_owned_pane)
                    else {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(
                                "Refusing to close the last surface in a pane unless it is an owned ephemeral pane and close_empty_owned_pane=true.",
                            )),
                        );
                        continue;
                    };
                    let closing = close_outcome.terminal.clone();
                    closing.set_focus_state(false, cx);
                    closing.set_native_view_visible(false, cx);
                    closing.shutdown_surface(Some(window), cx);
                    if tab_idx == self.active_tab {
                        if close_outcome.closed_pane {
                            #[cfg(target_os = "macos")]
                            self.mark_active_tab_terminal_native_layout_pending(cx);
                            for terminal in self.tabs[tab_idx]
                                .pane_tree
                                .all_terminals()
                                .into_iter()
                                .cloned()
                                .collect::<Vec<_>>()
                            {
                                terminal.ensure_surface(window, cx);
                                terminal.notify(cx);
                            }
                        }
                        self.sync_active_tab_native_view_visibility(cx);
                        if closing_was_focused || close_outcome.closed_pane {
                            if let Some(replacement) =
                                self.tabs[tab_idx].pane_tree.try_focused_terminal().cloned()
                            {
                                replacement.ensure_surface(window, cx);
                                replacement.focus(window, cx);
                            }
                        }
                        self.sync_active_terminal_focus_states(cx);
                    }
                    if close_outcome.closed_pane {
                        self.reconcile_runtime_trackers_for_tab(tab_idx);
                    }
                    self.save_session(cx);
                    Self::send_control_result(
                        response_tx,
                        Ok(json!({
                            "status": "closed",
                            "closed_pane": close_outcome.closed_pane,
                            "tab_index": tab_idx + 1,
                            "pane_id": close_outcome.pane_id,
                            "surface_id": resolved.surface_id,
                        })),
                    );
                }
            }
        }

        cx.notify();
        window.refresh();
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_created_surface(
        &mut self,
        tab_idx: usize,
        pane_id: usize,
        surface_id: usize,
        terminal: TerminalPane,
        command: Option<String>,
        created_pane: bool,
        response_tx: oneshot::Sender<ControlResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab_is_active = tab_idx == self.active_tab;
        if tab_is_active {
            terminal.focus(window, cx);
            self.sync_active_terminal_focus_states(cx);
            #[cfg(not(target_os = "macos"))]
            self.sync_active_tab_native_view_visibility(cx);
        } else {
            terminal.set_focus_state(false, cx);
            terminal.set_native_view_visible(false, cx);
        }
        Self::schedule_terminal_bootstrap_reassert(
            &terminal,
            tab_is_active,
            self.window_handle,
            self.workspace_handle.clone(),
            cx,
        );
        if let Some(cmd) = &command {
            terminal.write(format!("{cmd}\n").as_bytes(), cx);
        }
        self.record_runtime_event_for_terminal(
            tab_idx,
            &terminal,
            con_agent::context::PaneRuntimeEvent::PaneCreated {
                startup_command: command.clone(),
            },
        );
        let result =
            self.surface_created_result(tab_idx, pane_id, surface_id, created_pane, &terminal, cx);
        Self::send_control_result(response_tx, Ok(result));
        self.save_session(cx);
    }

    pub(super) fn reconcile_runtime_trackers_for_tab(&self, tab_idx: usize) {
        let pane_ids: HashSet<usize> = self.tabs[tab_idx]
            .pane_tree
            .pane_terminals()
            .into_iter()
            .map(|(pane_id, _)| pane_id)
            .collect();
        self.tabs[tab_idx]
            .runtime_trackers
            .borrow_mut()
            .retain(|pane_id, _| pane_ids.contains(pane_id));
        self.tabs[tab_idx]
            .runtime_cache
            .borrow_mut()
            .retain(|pane_id, _| pane_ids.contains(pane_id));
    }

    pub(super) fn observe_terminal_runtime_for_tab(
        &self,
        tab_idx: usize,
        terminal: &TerminalPane,
        recent_output_lines: usize,
        cx: &App,
    ) -> (
        con_agent::context::PaneObservationFrame,
        con_agent::context::PaneRuntimeState,
    ) {
        let pane_tree = &self.tabs[tab_idx].pane_tree;
        let pane_id = pane_tree
            .pane_id_for_terminal(terminal)
            .unwrap_or(usize::MAX);
        let observation = terminal.observation_frame(recent_output_lines, cx);
        let runtime = {
            let mut trackers = self.tabs[tab_idx].runtime_trackers.borrow_mut();
            let tracker = trackers.entry(pane_id).or_default();
            tracker.observe(observation.clone())
        };
        self.tabs[tab_idx]
            .runtime_cache
            .borrow_mut()
            .insert(pane_id, runtime.clone());
        (observation, runtime)
    }

    pub(super) fn cached_runtime_for_tab(
        &self,
        tab_idx: usize,
        terminal: &TerminalPane,
    ) -> Option<con_agent::context::PaneRuntimeState> {
        let pane_id = self.tabs[tab_idx]
            .pane_tree
            .pane_id_for_terminal(terminal)?;
        self.tabs[tab_idx]
            .runtime_cache
            .borrow()
            .get(&pane_id)
            .cloned()
    }

    pub(super) fn record_runtime_event_for_terminal(
        &self,
        tab_idx: usize,
        terminal: &TerminalPane,
        event: con_agent::context::PaneRuntimeEvent,
    ) {
        let pane_tree = &self.tabs[tab_idx].pane_tree;
        let pane_id = pane_tree
            .pane_id_for_terminal(terminal)
            .unwrap_or(usize::MAX);
        let mut trackers = self.tabs[tab_idx].runtime_trackers.borrow_mut();
        let tracker = trackers.entry(pane_id).or_default();
        tracker.record_action(event);
    }

    pub(super) fn resolve_pane_target_for_tab(
        &self,
        tab_idx: usize,
        selector: con_agent::tools::PaneSelector,
    ) -> Result<ResolvedPaneTarget, String> {
        let pane_tree = &self.tabs[tab_idx].pane_tree;
        let all_terminals = pane_tree.all_terminals();
        let focused_terminal = pane_tree.try_visible_focus_terminal();
        let focused_pane_id = focused_terminal
            .map(|(pane_id, _)| pane_id)
            .unwrap_or_else(|| pane_tree.focused_pane_id());
        let focused_pane = focused_terminal.map(|(_, terminal)| terminal.clone());
        let focused_pane_index = all_terminals
            .iter()
            .enumerate()
            .find(|(_, terminal)| pane_tree.pane_id_for_terminal(terminal) == Some(focused_pane_id))
            .map(|(idx, _)| idx + 1)
            .unwrap_or(1);

        let by_id = match selector.pane_id {
            Some(pane_id) => {
                let pane = all_terminals
                    .iter()
                    .enumerate()
                    .find_map(|(idx, terminal)| {
                        (pane_tree.pane_id_for_terminal(terminal) == Some(pane_id)).then(|| {
                            ResolvedPaneTarget {
                                pane: (*terminal).clone(),
                                pane_index: idx + 1,
                                pane_id,
                            }
                        })
                    })
                    .ok_or_else(|| {
                        format!(
                            "Pane id {} is no longer available in this tab. That pane was likely closed. Re-run list_panes to choose a new target.",
                            pane_id
                        )
                    })?;
                Some(pane)
            }
            None => None,
        };

        let by_index = match selector.pane_index {
            Some(index) => {
                if index == 0 || index > all_terminals.len() {
                    if let Some(from_id) = by_id.clone() {
                        log::warn!(
                            "Pane selector index {} is stale; continuing with pane_id {}",
                            index,
                            from_id.pane_id
                        );
                        None
                    } else {
                        return Err(format!(
                            "Pane index {} is no longer valid in the current layout. The pane layout changed or that pane was closed. Re-run list_panes and prefer pane_id for follow-up targeting.",
                            index,
                        ));
                    }
                } else {
                    let pane = all_terminals[index - 1].clone();
                    let pane_id = match pane_tree.pane_id_for_terminal(&pane) {
                        Some(pane_id) => pane_id,
                        None if by_id.is_some() => {
                            let from_id = by_id.as_ref().expect("checked is_some");
                            log::warn!(
                                "Pane selector index {} no longer resolves to a live pane; continuing with pane_id {}",
                                index,
                                from_id.pane_id
                            );
                            return Ok(from_id.clone());
                        }
                        None => {
                            return Err(format!(
                                "Pane index {} no longer resolves to a live pane. Re-run list_panes and prefer pane_id for follow-up targeting.",
                                index
                            ));
                        }
                    };
                    Some(ResolvedPaneTarget {
                        pane,
                        pane_index: index,
                        pane_id,
                    })
                }
            }
            None => None,
        };

        match (by_index, by_id) {
            (Some(from_index), Some(from_id)) => {
                if from_index.pane_id != from_id.pane_id {
                    log::warn!(
                        "Pane selector mismatch: pane_index {} resolved to pane_id {}, but caller also supplied pane_id {}; continuing with pane_id",
                        from_index.pane_index,
                        from_index.pane_id,
                        from_id.pane_id
                    );
                    return Ok(from_id);
                }
                Ok(from_id)
            }
            (Some(target), None) => Ok(target),
            (None, Some(target)) => Ok(target),
            (None, None) => {
                let pane = focused_pane.ok_or_else(|| {
                    "No terminal pane is focused in this tab. Use list_panes to select a target.".to_string()
                })?;
                Ok(ResolvedPaneTarget {
                    pane,
                    pane_index: focused_pane_index,
                    pane_id: focused_pane_id,
                })
            }
        }
    }

    pub(super) fn resolve_surface_target_for_tab(
        &self,
        tab_idx: usize,
        target: con_core::SurfaceTarget,
    ) -> Result<ResolvedSurfaceTarget, ControlError> {
        let pane_tree = &self.tabs[tab_idx].pane_tree;
        let surfaces = pane_tree.surface_infos(None);
        if let Some(surface_id) = target.surface_id {
            let surface = surfaces
                .into_iter()
                .find(|surface| surface.surface_id == surface_id)
                .ok_or_else(|| {
                    ControlError::invalid_params(format!(
                        "Surface id {} is no longer available in tab {}.",
                        surface_id,
                        tab_idx + 1
                    ))
                })?;
            if let Some(pane_id) = target.pane_id
                && pane_id != surface.pane_id
            {
                return Err(ControlError::invalid_params(format!(
                    "Surface id {} belongs to pane id {}, not pane id {}.",
                    surface_id, surface.pane_id, pane_id
                )));
            }
            if let Some(pane_index) = target.pane_index
                && pane_index != surface.pane_index
            {
                return Err(ControlError::invalid_params(format!(
                    "Surface id {} belongs to pane {}, not pane {}.",
                    surface_id, surface.pane_index, pane_index
                )));
            }
            return Ok(ResolvedSurfaceTarget {
                terminal: surface.terminal,
                pane_index: surface.pane_index,
                pane_id: surface.pane_id,
                surface_index: surface.surface_index,
                surface_id,
            });
        }

        let resolved_pane = self
            .resolve_pane_target_for_tab(
                tab_idx,
                Self::pane_selector_from_target(target.pane_target()),
            )
            .map_err(ControlError::invalid_params)?;
        let surface_id = pane_tree
            .active_surface_id_for_pane(resolved_pane.pane_id)
            .ok_or_else(|| {
                ControlError::invalid_params(format!(
                    "Pane id {} has no active surface in tab {}.",
                    resolved_pane.pane_id,
                    tab_idx + 1
                ))
            })?;
        let surface = pane_tree
            .surface_infos(Some(resolved_pane.pane_id))
            .into_iter()
            .find(|surface| surface.surface_id == surface_id)
            .ok_or_else(|| {
                ControlError::invalid_params(format!(
                    "Surface id {} is no longer available in tab {}.",
                    surface_id,
                    tab_idx + 1
                ))
            })?;
        Ok(ResolvedSurfaceTarget {
            terminal: surface.terminal,
            pane_index: surface.pane_index,
            pane_id: surface.pane_id,
            surface_index: surface.surface_index,
            surface_id,
        })
    }

    pub(super) fn surface_created_result(
        &self,
        tab_idx: usize,
        pane_id: usize,
        surface_id: usize,
        created_pane: bool,
        terminal: &TerminalPane,
        cx: &App,
    ) -> serde_json::Value {
        let pane_tree = &self.tabs[tab_idx].pane_tree;
        let pane_index = pane_tree
            .pane_terminals()
            .into_iter()
            .enumerate()
            .find_map(|(index, (candidate, _))| (candidate == pane_id).then_some(index + 1))
            .unwrap_or(1);
        let surface_index = pane_tree
            .surface_infos(Some(pane_id))
            .into_iter()
            .find_map(|surface| (surface.surface_id == surface_id).then_some(surface.surface_index))
            .unwrap_or(1);
        json!({
            "tab_index": tab_idx + 1,
            "created_pane": created_pane,
            "pane_index": pane_index,
            "pane_id": pane_id,
            "pane_ref": format!("pane:{pane_index}"),
            "surface_index": surface_index,
            "surface_id": surface_id,
            "surface_ref": format!("surface:{surface_index}"),
            "surface_ready": terminal.surface_ready(cx),
            "is_alive": terminal.is_alive(cx),
            "has_shell_integration": terminal.has_shell_integration(cx),
            "foreground_process_group_id": terminal.foreground_process_group_id(cx),
            "tty_name": terminal.tty_name(cx),
        })
    }

    pub(super) fn surface_wait_ready_result(
        &self,
        tab_idx: usize,
        resolved: &ResolvedSurfaceTarget,
        status: &str,
        cx: &App,
    ) -> serde_json::Value {
        json!({
            "status": status,
            "tab_index": tab_idx + 1,
            "pane_index": resolved.pane_index,
            "pane_id": resolved.pane_id,
            "surface_index": resolved.surface_index,
            "surface_id": resolved.surface_id,
            "surface_ready": resolved.terminal.surface_ready(cx),
            "is_alive": resolved.terminal.is_alive(cx),
            "has_shell_integration": resolved.terminal.has_shell_integration(cx),
            "is_busy": resolved.terminal.is_busy(cx),
            "foreground_process_group_id": resolved.terminal.foreground_process_group_id(cx),
            "tty_name": resolved.terminal.tty_name(cx),
        })
    }

    pub(super) fn surface_info_value(
        &self,
        tab_idx: usize,
        surface: crate::pane_tree::PaneSurfaceInfo,
        cx: &App,
    ) -> serde_json::Value {
        let title = surface
            .title
            .clone()
            .or_else(|| surface.terminal.title(cx))
            .unwrap_or_else(|| format!("Surface {}", surface.surface_index));
        let (cols, rows) = surface.terminal.grid_size(cx);
        json!({
            "tab_index": tab_idx + 1,
            "pane_index": surface.pane_index,
            "pane_id": surface.pane_id,
            "pane_ref": format!("pane:{}", surface.pane_index),
            "surface_index": surface.surface_index,
            "surface_id": surface.surface_id,
            "surface_ref": format!("surface:{}", surface.surface_index),
            "title": title,
            "cwd": surface.terminal.current_dir(cx),
            "is_active": surface.is_active,
            "is_focused_pane": surface.is_focused_pane,
            "surface_ready": surface.terminal.surface_ready(cx),
            "is_alive": surface.terminal.is_alive(cx),
            "has_shell_integration": surface.terminal.has_shell_integration(cx),
            "is_busy": surface.terminal.is_busy(cx),
            "foreground_process_group_id": surface.terminal.foreground_process_group_id(cx),
            "tty_name": surface.terminal.tty_name(cx),
            "rows": rows,
            "cols": cols,
            "owner": surface.owner,
            "close_pane_when_last": surface.close_pane_when_last,
        })
    }

    pub(super) fn surface_key_bytes(key: &str) -> Result<Vec<u8>, ControlError> {
        let key = key.trim();
        match key.to_ascii_lowercase().as_str() {
            "escape" | "esc" => Ok(vec![0x1b]),
            "enter" | "return" => Ok(b"\n".to_vec()),
            "tab" => Ok(b"\t".to_vec()),
            "backspace" => Ok(vec![0x7f]),
            _ => {
                if let Some(code) = crate::terminal_keys::ctrl_chord_to_c0(key) {
                    return Ok(vec![code]);
                }
                if key.chars().count() == 1 {
                    return Ok(key.as_bytes().to_vec());
                }
                return Err(ControlError::invalid_params(format!(
                    "Unsupported surface key `{key}`. Supported keys: escape, enter, tab, backspace, ctrl-<letter>, ctrl-space/ctrl-@, ctrl-[, ctrl-\\, ctrl-], ctrl-^, ctrl-_, ctrl-/, ctrl-?, ctrl-~, ctrl-2..8."
                )));
            }
        }
    }

    pub(super) fn spawn_shell_anchor_command<F>(
        &self,
        _tab_idx: usize,
        pane: TerminalPane,
        pane_index: usize,
        command: String,
        timeout_secs: u64,
        parse_response: F,
        response_tx: crossbeam_channel::Sender<con_agent::PaneResponse>,
        cx: &mut Context<Self>,
    ) where
        F: Fn(Vec<String>) -> Result<con_agent::PaneResponse, String> + Send + 'static,
    {
        let _ = pane.take_command_finished(cx);
        pane.write(format!("{command}\n").as_bytes(), cx);

        cx.spawn(async move |this, cx| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;

                let lines = this
                    .update(cx, |_, cx| pane.recent_lines(400, cx))
                    .unwrap_or_default();

                match parse_response(lines.clone()) {
                    Ok(response) => {
                        let _ = this.update(cx, |_, cx| {
                            pane.recover_shell_prompt_state(cx);
                        });
                        let _ = response_tx.send(response);
                        return;
                    }
                    Err(_) => {}
                }

                if std::time::Instant::now() >= deadline {
                    let excerpt = this
                        .update(cx, |_, cx| pane.recent_lines(120, cx).join("\n"))
                        .unwrap_or_default();
                    let _ = response_tx.send(con_agent::PaneResponse::Error(format!(
                        "Shell-anchor command timed out in pane {} after {}s.\nRecent output:\n{}",
                        pane_index, timeout_secs, excerpt
                    )));
                    return;
                }

                let finished = this
                    .update(cx, |_, cx| {
                        pane.take_command_finished(cx).is_some() || !pane.is_busy(cx)
                    })
                    .unwrap_or(false);
                if !finished {
                    continue;
                }

                let excerpt = this
                    .update(cx, |_, cx| pane.recent_lines(120, cx).join("\n"))
                    .unwrap_or_default();
                let parse_err = parse_response(lines).err().unwrap_or_else(|| {
                    "shell-anchor markers were not observed in pane output".to_string()
                });
                let _ = response_tx.send(con_agent::PaneResponse::Error(format!(
                    "Shell-anchor command in pane {} could not be parsed: {}\nRecent output:\n{}",
                    pane_index, parse_err, excerpt
                )));
                return;
            }
        })
        .detach();
    }

    pub(super) fn pane_blocks_shell_anchor_control(pane: &TerminalPane, cx: &App) -> bool {
        if !pane.is_busy(cx) {
            return false;
        }

        let observation = pane.observation_frame(40, cx);
        let prompt_like = observation
            .screen_hints
            .iter()
            .any(|hint| hint.kind == con_agent::context::PaneObservationHintKind::PromptLikeInput);

        if prompt_like {
            pane.recover_shell_prompt_state(cx);
            return false;
        }

        true
    }

    pub(super) fn effective_remote_host_for_tab(
        &self,
        tab_idx: usize,
        terminal: &TerminalPane,
        cx: &App,
    ) -> Option<String> {
        self.cached_runtime_for_tab(tab_idx, terminal)
            .map(|runtime| runtime.remote_host)
            .unwrap_or_else(|| {
                self.observe_terminal_runtime_for_tab(tab_idx, terminal, 12, cx)
                    .1
                    .remote_host
            })
    }

    /// Build agent context from a tab's focused pane, including summaries of peer panes.
    pub(super) fn build_agent_context_for_tab(
        &self,
        tab_idx: usize,
        cx: &App,
    ) -> con_agent::TerminalContext {
        self.reconcile_runtime_trackers_for_tab(tab_idx);
        let pane_tree = &self.tabs[tab_idx].pane_tree;

        // Determine focused pane's 1-based index and hostname
        let all_terminals = pane_tree.all_terminals();
        let focused_terminal = pane_tree.try_visible_focus_terminal();
        let focused_pid = focused_terminal
            .map(|(pane_id, _)| pane_id)
            .unwrap_or_else(|| pane_tree.focused_pane_id());
        let focused_pane_index = all_terminals
            .iter()
            .enumerate()
            .find(|(_, t)| pane_tree.pane_id_for_terminal(t) == Some(focused_pid))
            .map(|(i, _)| i + 1)
            .unwrap_or(1);

        // If there's no terminal pane (editor-only tab), return an empty context.
        let Some((_, focused)) = focused_terminal else {
            return self.harness.build_context_from_snapshot(
                focused_pane_index,
                focused_pid,
                &Default::default(),
                &Default::default(),
                vec![],
            );
        };

        let (focused_observation, focused_runtime) =
            self.observe_terminal_runtime_for_tab(tab_idx, focused, 50, cx);

        let mut other_pane_summaries = Vec::new();
        if pane_tree.pane_count() > 1 {
            for (idx, terminal) in all_terminals.iter().enumerate() {
                if let Some(pid) = pane_tree.pane_id_for_terminal(terminal) {
                    if pid == focused_pid {
                        continue;
                    }
                    let (observation, runtime) = self.observe_terminal_runtime_for_tab(
                        tab_idx,
                        terminal,
                        Self::SECONDARY_PANE_OBSERVATION_LINES,
                        cx,
                    );
                    let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                    let remote_workspace =
                        con_agent::context::remote_workspace_anchor(&runtime, &observation);
                    let workspace_cwd_hint = con_agent::context::workspace_cwd_hint(
                        observation.cwd.as_deref(),
                        &runtime.recent_actions,
                    );
                    other_pane_summaries.push(con_agent::context::PaneSummary {
                        pane_index: idx + 1,
                        pane_id: pid,
                        hostname: runtime.remote_host.clone(),
                        hostname_confidence: runtime.remote_host_confidence,
                        hostname_source: runtime.remote_host_source,
                        remote_workspace,
                        title: observation.title.clone(),
                        foreground_process_group_id: observation.foreground_process_group_id,
                        tty_name: observation.tty_name.clone(),
                        front_state: runtime.front_state,
                        mode: runtime.mode,
                        has_shell_integration: observation.has_shell_integration,
                        shell_metadata_fresh: runtime.shell_metadata_fresh,
                        observation_support: observation.support.clone(),
                        control,
                        agent_cli: runtime.agent_cli.clone(),
                        active_scope: runtime.active_scope.clone(),
                        evidence: runtime.evidence.clone(),
                        runtime_stack: runtime.scope_stack,
                        last_verified_runtime_stack: runtime.last_verified_scope_stack,
                        runtime_warnings: runtime.warnings,
                        tmux_session: runtime.tmux_session,
                        cwd: observation.cwd,
                        workspace_cwd_hint,
                        workspace_agent_cli_hint: con_agent::context::workspace_agent_cli_hint(
                            runtime.agent_cli.as_deref(),
                            &runtime.recent_actions,
                        ),
                        screen_hints: observation.screen_hints,
                        last_command: observation.last_command,
                        last_exit_code: observation.last_exit_code,
                        is_busy: observation.is_busy,
                        recent_output: observation.recent_output,
                    });
                }
            }
        }

        self.harness.build_context_from_snapshot(
            focused_pane_index,
            focused_pid,
            &focused_observation,
            &focused_runtime,
            other_pane_summaries,
        )
    }

    /// Build agent context from the active tab.
    pub(super) fn build_agent_context(&self, cx: &App) -> con_agent::TerminalContext {
        self.build_agent_context_for_tab(self.active_tab, cx)
    }

    pub(super) fn resolve_control_tab_index(
        &self,
        tab_index: Option<usize>,
    ) -> Result<usize, ControlError> {
        match tab_index {
            Some(index) if index == 0 || index > self.tabs.len() => {
                Err(ControlError::invalid_params(format!(
                    "Tab index {} is out of range. Valid tabs are 1..={}.",
                    index,
                    self.tabs.len()
                )))
            }
            Some(index) => Ok(index - 1),
            None => Ok(self.active_tab),
        }
    }

    pub(super) fn tab_index_for_summary_id(&self, tab_id: u64) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.summary_id == tab_id)
    }

    pub(super) fn pane_selector_from_target(
        target: con_core::PaneTarget,
    ) -> con_agent::tools::PaneSelector {
        con_agent::tools::PaneSelector::new(target.pane_index, target.pane_id)
    }

    pub(super) fn send_control_result(
        response_tx: tokio::sync::oneshot::Sender<ControlResult>,
        result: ControlResult,
    ) {
        let _ = response_tx.send(result);
    }

    pub(super) fn pane_response_to_control_result(
        tab_idx: usize,
        response: con_agent::PaneResponse,
    ) -> ControlResult {
        match response {
            con_agent::PaneResponse::PaneList(panes) => Ok(json!({
                "tab_index": tab_idx + 1,
                "panes": panes,
            })),
            con_agent::PaneResponse::Content(content) => Ok(json!({ "content": content })),
            con_agent::PaneResponse::KeysSent => Ok(json!({ "status": "sent" })),
            con_agent::PaneResponse::TmuxInfo(tmux) => Ok(json!({
                "tab_index": tab_idx + 1,
                "tmux": tmux,
            })),
            con_agent::PaneResponse::TmuxList(snapshot) => Ok(json!({
                "tab_index": tab_idx + 1,
                "snapshot": snapshot,
            })),
            con_agent::PaneResponse::TmuxCapture(capture) => Ok(json!({
                "tab_index": tab_idx + 1,
                "capture": capture,
            })),
            con_agent::PaneResponse::TmuxExec(exec) => Ok(json!({
                "tab_index": tab_idx + 1,
                "exec": exec,
            })),
            con_agent::PaneResponse::ShellProbe(shell) => Ok(json!({
                "tab_index": tab_idx + 1,
                "shell": shell,
            })),
            con_agent::PaneResponse::SearchResults(matches) => Ok(json!({
                "matches": matches,
            })),
            con_agent::PaneResponse::BusyStatus {
                surface_ready,
                is_alive,
                is_busy,
                has_shell_integration,
            } => Ok(json!({
                "surface_ready": surface_ready,
                "is_alive": is_alive,
                "is_busy": is_busy,
                "has_shell_integration": has_shell_integration,
            })),
            con_agent::PaneResponse::WaitComplete { status, output } => Ok(json!({
                "status": status,
                "output": output,
            })),
            con_agent::PaneResponse::PaneCreated {
                pane_index,
                pane_id,
                surface_ready,
                is_alive,
                has_shell_integration,
            } => Ok(json!({
                "tab_index": tab_idx + 1,
                "pane_index": pane_index,
                "pane_id": pane_id,
                "surface_ready": surface_ready,
                "is_alive": is_alive,
                "has_shell_integration": has_shell_integration,
            })),
            con_agent::PaneResponse::Error(err) => Err(ControlError::invalid_params(err)),
        }
    }

    pub(super) fn terminal_exec_response_to_control_result(
        response: TerminalExecResponse,
    ) -> ControlResult {
        Ok(json!({
            "output": response.output,
            "exit_code": response.exit_code,
        }))
    }

    pub(super) fn spawn_control_pane_query(
        &mut self,
        tab_idx: usize,
        query: con_agent::PaneQuery,
        response_tx: tokio::sync::oneshot::Sender<ControlResult>,
        cx: &mut Context<Self>,
    ) {
        let (pane_response_tx, pane_response_rx) = crossbeam_channel::bounded(1);
        self.handle_pane_request_for_tab(
            tab_idx,
            con_agent::PaneRequest {
                query,
                response_tx: pane_response_tx,
            },
            cx,
        );

        self.harness.spawn_detached(async move {
            let result = match tokio::task::spawn_blocking(move || {
                pane_response_rx.recv_timeout(std::time::Duration::from_secs(240))
            })
            .await
            {
                Ok(Ok(response)) => Self::pane_response_to_control_result(tab_idx, response),
                Ok(Err(_)) => Err(ControlError::internal(
                    "Timed out waiting for the pane operation to finish",
                )),
                Err(err) => Err(ControlError::internal(format!(
                    "Pane operation join failed: {err}"
                ))),
            };
            Self::send_control_result(response_tx, result);
        });
    }
}
