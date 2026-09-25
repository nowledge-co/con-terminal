use super::*;

impl ConWorkspace {
    pub(super) fn spawn_control_terminal_exec(
        &mut self,
        tab_idx: usize,
        command: String,
        target: con_core::PaneTarget,
        response_tx: tokio::sync::oneshot::Sender<ControlResult>,
        cx: &mut Context<Self>,
    ) {
        let (exec_response_tx, exec_response_rx) = crossbeam_channel::bounded(1);
        self.handle_terminal_exec_request_for_tab(
            tab_idx,
            TerminalExecRequest {
                command,
                working_dir: None,
                target: Self::pane_selector_from_target(target),
                response_tx: exec_response_tx,
            },
            cx,
        );

        self.harness.spawn_detached(async move {
            let result = match tokio::task::spawn_blocking(move || {
                exec_response_rx.recv_timeout(std::time::Duration::from_secs(240))
            })
            .await
            {
                Ok(Ok(response)) => Self::terminal_exec_response_to_control_result(response),
                Ok(Err(_)) => Err(ControlError::internal(
                    "Timed out waiting for the visible shell command to finish",
                )),
                Err(err) => Err(ControlError::internal(format!(
                    "Visible shell join failed: {err}"
                ))),
            };
            Self::send_control_result(response_tx, result);
        });
    }

    pub(super) fn handle_pending_control_agent_event(
        &mut self,
        tab_idx: usize,
        event: &HarnessEvent,
    ) -> bool {
        let auto_approve = self
            .pending_control_agent_requests
            .get(&tab_idx)
            .map(|pending| pending.auto_approve_tools)
            .unwrap_or(false);

        if auto_approve
            && let HarnessEvent::ToolApprovalNeeded {
                call_id,
                approval_tx,
                ..
            } = event
        {
            let _ = approval_tx.send(con_agent::ToolApprovalDecision {
                call_id: call_id.clone(),
                allowed: true,
                reason: Some("auto-approved by con-cli".to_string()),
            });
            return true;
        }

        match event {
            HarnessEvent::ResponseComplete(message) => {
                if let Some(pending) = self.pending_control_agent_requests.remove(&tab_idx) {
                    let result = serde_json::to_value(AgentAskResult {
                        tab_index: tab_idx + 1,
                        conversation_id: self.tabs[tab_idx].session.conversation_id(),
                        prompt: pending.prompt,
                        message: message.clone(),
                    })
                    .map_err(|err| {
                        ControlError::internal(format!(
                            "Failed to serialize agent response for control output: {err}"
                        ))
                    });
                    Self::send_control_result(pending.response_tx, result);
                }
            }
            HarnessEvent::Error(err) => {
                if err.starts_with("Retrying (") {
                    return false;
                }
                if let Some(pending) = self.pending_control_agent_requests.remove(&tab_idx) {
                    Self::send_control_result(
                        pending.response_tx,
                        Err(ControlError::internal(err.clone())),
                    );
                }
            }
            _ => {}
        }

        false
    }

    pub(super) fn handle_control_request(
        &mut self,
        request: ControlRequestEnvelope,
        cx: &mut Context<Self>,
    ) {
        let ControlRequestEnvelope {
            command,
            response_tx,
        } = request;

        match command {
            ControlCommand::Handoff(command) => {
                self.handle_handoff_command(command, response_tx, cx)
            }
            ControlCommand::SystemIdentify => {
                let result = serde_json::to_value(SystemIdentifyResult {
                    app: "con".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    socket_path: self
                        .control_socket
                        .as_ref()
                        .map(|handle| handle.path().display().to_string())
                        .unwrap_or_else(|| con_core::control_socket_path().display().to_string()),
                    active_tab_index: self.active_tab + 1,
                    tab_count: self.tabs.len(),
                    methods: con_core::control_methods(),
                })
                .map_err(|err| ControlError::internal(err.to_string()));
                Self::send_control_result(response_tx, result);
            }
            ControlCommand::SystemCapabilities => {
                Self::send_control_result(
                    response_tx,
                    Ok(json!({ "methods": con_core::control_methods() })),
                );
            }
            ControlCommand::TabsList => {
                let tabs = self
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(idx, tab)| TabInfo {
                        index: idx + 1,
                        title: tab
                            .pane_tree
                            .try_focused_terminal()
                            .and_then(|t| t.title(cx))
                            .unwrap_or_else(|| tab.title.clone()),
                        is_active: idx == self.active_tab,
                        pane_count: tab.pane_tree.pane_count(),
                        focused_pane_id: tab.pane_tree.focused_pane_id(),
                        needs_attention: tab.needs_attention,
                        conversation_id: tab.session.conversation_id(),
                    })
                    .collect::<Vec<_>>();
                Self::send_control_result(
                    response_tx,
                    Ok(json!({
                        "active_tab_index": self.active_tab + 1,
                        "tabs": tabs,
                    })),
                );
            }
            ControlCommand::TabsNew => {
                self.pending_window_control_requests
                    .push(PendingWindowControlRequest::TabsNew { response_tx });
                self.schedule_pending_create_pane_flush(cx);
            }
            ControlCommand::TabsClose { tab_index } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => {
                        self.pending_window_control_requests.push(
                            PendingWindowControlRequest::TabsClose {
                                tab_idx,
                                response_tx,
                            },
                        );
                        self.schedule_pending_create_pane_flush(cx);
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::PanesList { tab_index } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => {
                        self.spawn_control_pane_query(
                            tab_idx,
                            con_agent::PaneQuery::List,
                            response_tx,
                            cx,
                        );
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::PanesRead {
                tab_index,
                target,
                lines,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => self.spawn_control_pane_query(
                    tab_idx,
                    con_agent::PaneQuery::ReadContent {
                        target: Self::pane_selector_from_target(target),
                        lines,
                    },
                    response_tx,
                    cx,
                ),
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::PanesExec {
                tab_index,
                target,
                command,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => {
                    self.spawn_control_terminal_exec(tab_idx, command, target, response_tx, cx)
                }
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::PanesSendKeys {
                tab_index,
                target,
                keys,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => self.spawn_control_pane_query(
                    tab_idx,
                    con_agent::PaneQuery::SendKeys {
                        target: Self::pane_selector_from_target(target),
                        keys,
                    },
                    response_tx,
                    cx,
                ),
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::PanesCreate {
                tab_index,
                location,
                command,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => self.spawn_control_pane_query(
                    tab_idx,
                    con_agent::PaneQuery::CreatePane { command, location },
                    response_tx,
                    cx,
                ),
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::PanesWait {
                tab_index,
                target,
                timeout_secs,
                pattern,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => self.spawn_control_pane_query(
                    tab_idx,
                    con_agent::PaneQuery::WaitFor {
                        target: Self::pane_selector_from_target(target),
                        timeout_secs,
                        pattern,
                    },
                    response_tx,
                    cx,
                ),
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::PanesProbeShell { tab_index, target } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => self.spawn_control_pane_query(
                        tab_idx,
                        con_agent::PaneQuery::ProbeShellContext {
                            target: Self::pane_selector_from_target(target),
                        },
                        response_tx,
                        cx,
                    ),
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::TreeGet { tab_index } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => {
                        let tabs = self
                        .tabs
                        .iter()
                        .enumerate()
                        .filter(|(idx, _)| tab_index.is_none() || *idx == tab_idx)
                        .map(|(idx, tab)| {
                            let panes = tab
                                .pane_tree
                                .pane_terminals()
                                .into_iter()
                                .enumerate()
                                .map(|(pane_index, (pane_id, terminal))| {
                                    let surfaces = tab
                                        .pane_tree
                                        .surface_infos(Some(pane_id))
                                        .into_iter()
                                        .map(|surface| self.surface_info_value(idx, surface, cx))
                                        .collect::<Vec<_>>();
                                    json!({
                                        "pane_index": pane_index + 1,
                                        "pane_id": pane_id,
                                        "pane_ref": format!("pane:{}", pane_index + 1),
                                        "title": terminal.title(cx).unwrap_or_else(|| format!("Pane {}", pane_index + 1)),
                                        "is_focused": pane_id == tab.pane_tree.focused_pane_id(),
                                        "active_surface_id": tab.pane_tree.active_surface_id_for_pane(pane_id),
                                        "surfaces": surfaces,
                                    })
                                })
                                .collect::<Vec<_>>();
                            json!({
                                "tab_index": idx + 1,
                                "is_active": idx == self.active_tab,
                                "title": tab.title,
                                "focused_pane_id": tab.pane_tree.focused_pane_id(),
                                "panes": panes,
                            })
                        })
                        .collect::<Vec<_>>();
                        Self::send_control_result(
                            response_tx,
                            Ok(json!({
                                "active_tab_index": self.active_tab + 1,
                                "tabs": tabs,
                            })),
                        );
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::SurfacesList { tab_index, pane } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => {
                        let target_pane_id = if pane.pane_index.is_some() || pane.pane_id.is_some()
                        {
                            match self.resolve_pane_target_for_tab(
                                tab_idx,
                                Self::pane_selector_from_target(pane),
                            ) {
                                Ok(resolved) => Some(resolved.pane_id),
                                Err(err) => {
                                    Self::send_control_result(
                                        response_tx,
                                        Err(ControlError::invalid_params(err)),
                                    );
                                    return;
                                }
                            }
                        } else {
                            None
                        };
                        let surfaces = self.tabs[tab_idx]
                            .pane_tree
                            .surface_infos(target_pane_id)
                            .into_iter()
                            .map(|surface| self.surface_info_value(tab_idx, surface, cx))
                            .collect::<Vec<_>>();
                        Self::send_control_result(
                            response_tx,
                            Ok(json!({
                                "tab_index": tab_idx + 1,
                                "surfaces": surfaces,
                            })),
                        );
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::SurfacesCreate {
                tab_index,
                pane,
                title,
                command,
                owner,
                close_pane_when_last,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => {
                    self.pending_surface_control_requests.push(
                        PendingSurfaceControlRequest::Create {
                            tab_idx,
                            pane,
                            title,
                            command,
                            owner,
                            close_pane_when_last,
                            response_tx,
                        },
                    );
                    self.schedule_pending_create_pane_flush(cx);
                }
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::SurfacesSplit {
                tab_index,
                source,
                location,
                title,
                command,
                owner,
                close_pane_when_last,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => {
                    self.pending_surface_control_requests.push(
                        PendingSurfaceControlRequest::Split {
                            tab_idx,
                            source,
                            location,
                            title,
                            command,
                            owner,
                            close_pane_when_last,
                            response_tx,
                        },
                    );
                    self.schedule_pending_create_pane_flush(cx);
                }
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::SurfacesFocus { tab_index, target } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => match self.resolve_surface_target_for_tab(tab_idx, target) {
                        Ok(resolved) => {
                            let surface_id = resolved.surface_id;
                            let was_zoomed =
                                self.tabs[tab_idx].pane_tree.zoomed_pane_id().is_some();
                            let changed = self.tabs[tab_idx].pane_tree.focus_surface(surface_id);
                            let resolved = match self.resolve_surface_target_for_tab(
                                tab_idx,
                                con_core::SurfaceTarget::new(None, None, Some(surface_id)),
                            ) {
                                Ok(resolved) => resolved,
                                Err(err) => {
                                    Self::send_control_result(response_tx, Err(err));
                                    return;
                                }
                            };
                            #[cfg(target_os = "macos")]
                            let active_tab_needs_focus_sync = tab_idx == self.active_tab;
                            #[cfg(not(target_os = "macos"))]
                            let active_tab_needs_focus_sync = changed && tab_idx == self.active_tab;

                            if active_tab_needs_focus_sync {
                                #[cfg(target_os = "macos")]
                                self.mark_tab_terminal_native_layout_pending(tab_idx, cx);
                                #[cfg(target_os = "macos")]
                                self.notify_tab_terminal_views(tab_idx, cx);
                                self.sync_active_terminal_focus_states(cx);
                                self.schedule_active_terminal_focus(was_zoomed, cx);
                            }
                            if changed {
                                self.sync_sidebar(cx);
                                self.save_session(cx);
                            }
                            if changed || active_tab_needs_focus_sync {
                                cx.notify();
                            }
                            Self::send_control_result(
                                response_tx,
                                Ok(json!({
                                    "status": if changed { "focused" } else { "unchanged" },
                                    "tab_index": tab_idx + 1,
                                    "pane_index": resolved.pane_index,
                                    "pane_id": resolved.pane_id,
                                    "surface_index": resolved.surface_index,
                                    "surface_id": resolved.surface_id,
                                })),
                            );
                        }
                        Err(err) => Self::send_control_result(response_tx, Err(err)),
                    },
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::SurfacesRename {
                tab_index,
                target,
                title,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => match self.resolve_surface_target_for_tab(tab_idx, target) {
                    Ok(resolved) => {
                        self.tabs[tab_idx]
                            .pane_tree
                            .rename_surface(resolved.surface_id, Some(title.clone()));
                        self.sync_sidebar(cx);
                        self.save_session(cx);
                        Self::send_control_result(
                            response_tx,
                            Ok(json!({
                                "status": "renamed",
                                "tab_index": tab_idx + 1,
                                "pane_id": resolved.pane_id,
                                "surface_id": resolved.surface_id,
                                "title": title,
                            })),
                        );
                        cx.notify();
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                },
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::SurfacesClose {
                tab_index,
                target,
                close_empty_owned_pane,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => {
                    self.pending_surface_control_requests.push(
                        PendingSurfaceControlRequest::Close {
                            tab_idx,
                            target,
                            close_empty_owned_pane,
                            response_tx,
                        },
                    );
                    self.schedule_pending_create_pane_flush(cx);
                }
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::SurfacesRead {
                tab_index,
                target,
                lines,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => match self.resolve_surface_target_for_tab(tab_idx, target) {
                    Ok(resolved) => Self::send_control_result(
                        response_tx,
                        Ok(json!({
                            "tab_index": tab_idx + 1,
                            "pane_id": resolved.pane_id,
                            "surface_id": resolved.surface_id,
                            "content": resolved.terminal.recent_lines(lines, cx).join("\n"),
                        })),
                    ),
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                },
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::SurfacesSendText {
                tab_index,
                target,
                text,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => match self.resolve_surface_target_for_tab(tab_idx, target) {
                    Ok(resolved) => {
                        resolved.terminal.write(text.as_bytes(), cx);
                        self.record_runtime_event_for_terminal(
                            tab_idx,
                            &resolved.terminal,
                            con_agent::context::PaneRuntimeEvent::RawInput {
                                keys: text,
                                input_generation: resolved.terminal.input_generation(cx),
                            },
                        );
                        Self::send_control_result(
                            response_tx,
                            Ok(json!({
                                "status": "sent",
                                "tab_index": tab_idx + 1,
                                "pane_id": resolved.pane_id,
                                "surface_id": resolved.surface_id,
                            })),
                        );
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                },
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::SurfacesSendKey {
                tab_index,
                target,
                key,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => match self.resolve_surface_target_for_tab(tab_idx, target) {
                    Ok(resolved) => match Self::surface_key_bytes(&key) {
                        Ok(bytes) => {
                            resolved.terminal.write(&bytes, cx);
                            self.record_runtime_event_for_terminal(
                                tab_idx,
                                &resolved.terminal,
                                con_agent::context::PaneRuntimeEvent::RawInput {
                                    keys: key,
                                    input_generation: resolved.terminal.input_generation(cx),
                                },
                            );
                            Self::send_control_result(
                                response_tx,
                                Ok(json!({
                                    "status": "sent",
                                    "tab_index": tab_idx + 1,
                                    "pane_id": resolved.pane_id,
                                    "surface_id": resolved.surface_id,
                                })),
                            );
                        }
                        Err(err) => Self::send_control_result(response_tx, Err(err)),
                    },
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                },
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::SurfacesWaitReady {
                tab_index,
                target,
                timeout_secs,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => {
                    let requested_tab_index = tab_idx + 1;
                    let tab_id = self.tabs[tab_idx].summary_id;
                    let surface_id = match self.resolve_surface_target_for_tab(tab_idx, target) {
                        Ok(resolved) => resolved.surface_id,
                        Err(err) => {
                            Self::send_control_result(response_tx, Err(err));
                            return;
                        }
                    };
                    let timeout = Duration::from_secs(timeout_secs.unwrap_or(10).clamp(1, 300));
                    let started = std::time::Instant::now();
                    cx.spawn(async move |this, cx| {
                        loop {
                            let result = this.update(cx, |workspace, cx| {
                                let Some(current_tab_idx) =
                                    workspace.tab_index_for_summary_id(tab_id)
                                else {
                                    return Some(Err(ControlError::invalid_params(format!(
                                        "Tab {} is no longer available.",
                                        requested_tab_index
                                    ))));
                                };
                                let resolved = match workspace.resolve_surface_target_for_tab(
                                    current_tab_idx,
                                    con_core::SurfaceTarget::new(None, None, Some(surface_id)),
                                ) {
                                    Ok(resolved) => resolved,
                                    Err(err) => return Some(Err(err)),
                                };
                                let is_ready = resolved.terminal.surface_ready(cx)
                                    && resolved.terminal.is_alive(cx)
                                    && resolved.terminal.has_shell_integration(cx);
                                let timed_out = started.elapsed() >= timeout;
                                if is_ready || timed_out {
                                    let status = if is_ready { "ready" } else { "timeout" };
                                    Some(Ok(workspace.surface_wait_ready_result(
                                        current_tab_idx,
                                        &resolved,
                                        status,
                                        cx,
                                    )))
                                } else {
                                    None
                                }
                            });

                            match result {
                                Ok(Some(result)) => {
                                    Self::send_control_result(response_tx, result);
                                    return;
                                }
                                Ok(None) => {}
                                Err(err) => {
                                    Self::send_control_result(
                                        response_tx,
                                        Err(ControlError::internal(format!(
                                            "Failed to wait for surface readiness: {err}"
                                        ))),
                                    );
                                    return;
                                }
                            }

                            cx.background_executor()
                                .timer(Duration::from_millis(50))
                                .await;
                        }
                    })
                    .detach();
                }
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::TmuxInspect { tab_index, target } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => self.spawn_control_pane_query(
                        tab_idx,
                        con_agent::PaneQuery::InspectTmux {
                            target: Self::pane_selector_from_target(target),
                        },
                        response_tx,
                        cx,
                    ),
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::TmuxList { tab_index, target } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => self.spawn_control_pane_query(
                        tab_idx,
                        con_agent::PaneQuery::TmuxList {
                            pane: Self::pane_selector_from_target(target),
                        },
                        response_tx,
                        cx,
                    ),
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::TmuxCapture {
                tab_index,
                pane,
                target,
                lines,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => self.spawn_control_pane_query(
                    tab_idx,
                    con_agent::PaneQuery::TmuxCapture {
                        pane: Self::pane_selector_from_target(pane),
                        target,
                        lines,
                    },
                    response_tx,
                    cx,
                ),
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::TmuxSendKeys {
                tab_index,
                pane,
                target,
                literal_text,
                key_names,
                append_enter,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => self.spawn_control_pane_query(
                    tab_idx,
                    con_agent::PaneQuery::TmuxSendKeys {
                        pane: Self::pane_selector_from_target(pane),
                        target,
                        literal_text,
                        key_names,
                        append_enter,
                    },
                    response_tx,
                    cx,
                ),
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::TmuxRun {
                tab_index,
                pane,
                target,
                location,
                command,
                window_name,
                cwd,
                detached,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => self.spawn_control_pane_query(
                    tab_idx,
                    con_agent::PaneQuery::TmuxRunCommand {
                        pane: Self::pane_selector_from_target(pane),
                        target,
                        location,
                        command,
                        window_name,
                        cwd,
                        detached,
                    },
                    response_tx,
                    cx,
                ),
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::AgentNewConversation { tab_index } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => {
                        self.tabs[tab_idx].session.new_conversation();
                        if tab_idx == self.active_tab {
                            self.agent_panel.update(cx, |panel, cx| {
                                panel.clear_messages(cx);
                            });
                        } else {
                            self.tabs[tab_idx].panel_state.clear();
                        }
                        self.save_session(cx);
                        Self::send_control_result(
                            response_tx,
                            Ok(json!({
                                "tab_index": tab_idx + 1,
                                "conversation_id": self.tabs[tab_idx].session.conversation_id(),
                            })),
                        );
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::AgentAsk {
                tab_index,
                prompt,
                auto_approve_tools,
                timeout_secs,
            } => match self.resolve_control_tab_index(tab_index) {
                Ok(tab_idx) => {
                    if prompt.trim().is_empty() {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(
                                "agent.ask requires a non-empty prompt",
                            )),
                        );
                        return;
                    }
                    if self.pending_control_agent_requests.contains_key(&tab_idx) {
                        Self::send_control_result(
                            response_tx,
                            Err(ControlError::invalid_params(format!(
                                "Tab {} already has a pending con-cli agent request",
                                tab_idx + 1
                            ))),
                        );
                        return;
                    }

                    if tab_idx == self.active_tab {
                        self.agent_panel.update(cx, |panel, cx| {
                            panel.add_message("user", &prompt, cx);
                        });
                    } else {
                        self.tabs[tab_idx].panel_state.add_message("user", &prompt);
                    }

                    let context = self.build_agent_context_for_tab(tab_idx, cx);
                    let session = &self.tabs[tab_idx].session;
                    let agent_config = self.tab_agent_config(tab_idx);
                    let request_id = self.next_control_agent_request_id;
                    self.next_control_agent_request_id =
                        self.next_control_agent_request_id.wrapping_add(1);
                    self.pending_control_agent_requests.insert(
                        tab_idx,
                        PendingControlAgentRequest {
                            request_id,
                            prompt: prompt.clone(),
                            auto_approve_tools,
                            response_tx,
                        },
                    );
                    if let Some(timeout_secs) = timeout_secs.map(|secs| secs.clamp(5, 600)) {
                        self.spawn_control_agent_request_timeout(
                            tab_idx,
                            request_id,
                            timeout_secs,
                            cx,
                        );
                    }
                    self.harness
                        .send_message(session, agent_config, prompt, context);
                }
                Err(err) => Self::send_control_result(response_tx, Err(err)),
            },
            ControlCommand::AgentOpenPanelForRequest { tab_index } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => {
                        if tab_idx == self.active_tab
                            && let Some(target) =
                                super::chrome::agent_panel_motion_target_for_agent_request(
                                    self.agent_panel_open,
                                )
                        {
                            self.agent_panel_open = true;
                            let duration = Self::terminal_adjacent_chrome_duration(true, 290, 220);
                            self.agent_panel_motion.set_target(target, duration);
                            cx.notify();
                        }
                        let open = if tab_idx == self.active_tab {
                            self.agent_panel_open
                        } else {
                            false
                        };
                        Self::send_control_result(
                            response_tx,
                            Ok(json!({
                                "agent_panel": {
                                    "open": open,
                                    "content_visible": open,
                                }
                            })),
                        );
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
            ControlCommand::AgentPanelState { tab_index } => {
                match self.resolve_control_tab_index(tab_index) {
                    Ok(tab_idx) => {
                        let open = if tab_idx == self.active_tab {
                            self.agent_panel_open
                        } else {
                            false
                        };
                        Self::send_control_result(
                            response_tx,
                            Ok(json!({
                                "agent_panel": {
                                    "open": open,
                                    "content_visible": open,
                                }
                            })),
                        );
                    }
                    Err(err) => Self::send_control_result(response_tx, Err(err)),
                }
            }
        }
    }

    pub(super) fn spawn_control_agent_request_timeout(
        &self,
        _tab_idx: usize,
        request_id: u64,
        timeout_secs: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(timeout_secs))
                .await;

            let _ = this.update(cx, |workspace, _| {
                let current_tab_idx = workspace
                    .pending_control_agent_requests
                    .iter()
                    .find_map(|(idx, pending)| (pending.request_id == request_id).then_some(*idx));
                if let Some(current_tab_idx) = current_tab_idx {
                    let pending = workspace
                        .pending_control_agent_requests
                        .remove(&current_tab_idx)
                        .expect("pending request must exist");
                    Self::send_control_result(
                        pending.response_tx,
                        Err(ControlError::internal(format!(
                            "agent.ask timed out after {timeout_secs}s"
                        ))),
                    );
                }
            });
        })
        .detach();
    }
}
