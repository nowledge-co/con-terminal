use super::*;

impl ConWorkspace {
    /// Handle a visible terminal execution request from the agent.
    ///
    /// Writes the command to the focused PTY so the user sees it execute.
    /// Uses Ghostty's COMMAND_FINISHED signal when available, with a bounded
    /// recent-output fallback when shell integration is unavailable.
    pub(super) fn handle_terminal_exec_request_for_tab(
        &mut self,
        tab_idx: usize,
        req: TerminalExecRequest,
        cx: &mut Context<Self>,
    ) {
        let resolved = match self.resolve_pane_target_for_tab(tab_idx, req.target) {
            Ok(target) => target,
            Err(err) => {
                let _ = req.response_tx.send(TerminalExecResponse {
                    output: err,
                    exit_code: Some(1),
                });
                return;
            }
        };
        let pane = resolved.pane;
        let target_pane_index = resolved.pane_index;

        // Safety: refuse to execute on a dead PTY.
        if !pane.is_alive(cx) {
            let _ = req.response_tx.send(TerminalExecResponse {
                output: "Pane PTY process has exited — cannot execute command.".to_string(),
                exit_code: Some(1),
            });
            return;
        }

        let (observation, runtime) = self.observe_terminal_runtime_for_tab(tab_idx, &pane, 20, cx);
        let control = con_agent::control::PaneControlState::from_runtime(&runtime);
        let managed_remote_workspace =
            con_agent::context::remote_workspace_anchor(&runtime, &observation);
        if !control.allows_visible_shell_exec() && managed_remote_workspace.is_none() {
            let active_scope = runtime
                .active_scope
                .as_ref()
                .map(con_agent::context::PaneRuntimeScope::summary)
                .unwrap_or_else(|| runtime.mode.as_str().to_string());
            let host = runtime.remote_host.unwrap_or_else(|| "unknown".to_string());
            let notes = if control.notes.is_empty() {
                String::new()
            } else {
                format!("\nnotes:\n- {}", control.notes.join("\n- "))
            };
            let suggestion = match control.visible_target.kind {
                con_agent::PaneVisibleTargetKind::TmuxSession => {
                    if control
                        .capabilities
                        .contains(&con_agent::PaneControlCapability::QueryTmux)
                    {
                        format!(
                            "\n\nSUGGESTED APPROACH: This pane exposes native tmux control. Prefer tmux-native tools over outer-pane send_keys.\n\
                             1. tmux_list_targets(pane_index={idx}) to discover tmux windows/panes\n\
                             2. tmux_capture_pane(pane_index={idx}, target=\"%<pane>\") to inspect the exact tmux pane\n\
                             3. tmux_run_command(pane_index={idx}, location=\"new_window\", command=\"bash\", window_name=\"scratch\") to create a fresh shell target when needed\n\
                             4. tmux_send_keys(pane_index={idx}, target=\"%<pane>\", literal_text=\"your_command\", append_enter=true) to act on an existing tmux pane",
                            idx = target_pane_index
                        )
                    } else {
                        format!(
                            "\n\nSUGGESTED APPROACH: tmux native control is not currently available here. Use outer-pane send_keys only as a fallback.\n\
                             1. read_pane(pane_index={idx}) to inspect the visible tmux screen\n\
                             2. send_keys(pane_index={idx}, keys=\"\\x02c\") to create a new tmux window with a shell, or use another tmux prefix sequence to reach a shell pane\n\
                             3. read_pane(pane_index={idx}) to verify you reached a shell prompt\n\
                             4. send_keys(pane_index={idx}, keys=\"your_command\\n\") to execute\n\
                             5. read_pane(pane_index={idx}) to see the output",
                            idx = target_pane_index
                        )
                    }
                }
                con_agent::PaneVisibleTargetKind::InteractiveApp => {
                    format!(
                        "\n\nSUGGESTED APPROACH: Use read_pane(pane_index={idx}) to inspect the current screen, then \
                         send_keys(pane_index={idx}, ...) for keystroke-level interaction. \
                         Use \\x1b (Escape) or \\x03 (Ctrl-C) to exit to a shell if needed. \
                         Always verify with read_pane after each send_keys.",
                        idx = target_pane_index
                    )
                }
                _ => {
                    format!(
                        "\n\nSUGGESTED APPROACH: Use read_pane(pane_index={idx}) to inspect the visible app, then \
                         send_keys(pane_index={idx}, ...) for interaction. \
                         Always verify with read_pane after sending keys.",
                        idx = target_pane_index
                    )
                }
            };
            let output = format!(
                "Refused to execute shell command in pane {} because the visible target is not a proven shell.\n\
                 mode: {}\nactive_scope: {}\nhost: {}\nvisible_target: {}\n\
                 control_channels: {}\ncontrol_capabilities: {}{}{}",
                target_pane_index,
                runtime.mode.as_str(),
                active_scope,
                host,
                control.visible_target.summary(),
                control
                    .channels
                    .iter()
                    .map(|channel| channel.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                control
                    .capabilities
                    .iter()
                    .map(|capability| capability.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                notes,
                suggestion,
            );
            let _ = req.response_tx.send(TerminalExecResponse {
                output,
                exit_code: Some(2),
            });
            return;
        }

        // Safety: warn if pane is busy (command in progress).
        if pane.is_busy(cx) {
            log::warn!(
                "[workspace] Executing on busy pane — a command is already in progress. \
                 Command completion tracking may produce unexpected results."
            );
        }

        // Write the command to the PTY — user sees it execute in real time
        let cmd_with_newline = format!("{}\n", req.command);
        pane.write(cmd_with_newline.as_bytes(), cx);
        if let Some(pane_id) = self.tabs[tab_idx].pane_tree.pane_id_for_terminal(&pane) {
            self.record_shell_command(tab_idx, pane_id, &req.command, pane.current_dir(cx));
            self.after_shell_command_recorded(cx);
        }
        self.record_runtime_event_for_terminal(
            tab_idx,
            &pane,
            con_agent::context::PaneRuntimeEvent::VisibleShellExec {
                command: req.command.clone(),
                input_generation: pane.input_generation(cx),
            },
        );

        let fallback_response_tx = req.response_tx;
        let pane_for_fallback = pane.clone();
        cx.spawn(async move |_this, cx| {
            enum VisibleExecPoll {
                Finished {
                    output: String,
                    exit_code: Option<i32>,
                },
                Observe {
                    output: String,
                    prompt_like: bool,
                },
            }

            const PROMPT_STABLE_POLLS: u32 = 2;
            let mut last_prompt_snapshot = String::new();
            let mut stable_prompt_polls = 0u32;

            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;

            for _ in 0..29 {
                let poll = _this
                    .update(cx, |_ws, cx| {
                        if let Some((exit_code, _duration)) =
                            pane_for_fallback.take_command_finished(cx)
                        {
                            return VisibleExecPoll::Finished {
                                output: pane_for_fallback.recent_lines(50, cx).join("\n"),
                                exit_code,
                            };
                        }

                        let observation = pane_for_fallback.observation_frame(50, cx);
                        let output = observation
                            .recent_output
                            .iter()
                            .map(|line| line.trim_end())
                            .collect::<Vec<_>>()
                            .join("\n");
                        let prompt_like = observation.screen_hints.iter().any(|hint| {
                            matches!(
                                hint.kind,
                                con_agent::context::PaneObservationHintKind::PromptLikeInput
                            )
                        });

                        VisibleExecPoll::Observe {
                            output,
                            prompt_like,
                        }
                    })
                    .ok();

                match poll {
                    Some(VisibleExecPoll::Finished { output, exit_code }) => {
                        let _ = fallback_response_tx
                            .try_send(TerminalExecResponse { output, exit_code });
                        return;
                    }
                    Some(VisibleExecPoll::Observe {
                        output,
                        prompt_like,
                    }) if prompt_like => {
                        if !output.is_empty() && output == last_prompt_snapshot {
                            stable_prompt_polls += 1;
                        } else {
                            last_prompt_snapshot = output;
                            stable_prompt_polls = 0;
                        }

                        if stable_prompt_polls >= PROMPT_STABLE_POLLS {
                            let output = _this
                                .update(cx, |_ws, cx| {
                                    pane_for_fallback.recover_shell_prompt_state(cx);
                                    pane_for_fallback.recent_lines(50, cx).join("\n")
                                })
                                .unwrap_or_else(|_| last_prompt_snapshot.clone());
                            let _ = fallback_response_tx.try_send(TerminalExecResponse {
                                output,
                                exit_code: None,
                            });
                            return;
                        }
                    }
                    Some(VisibleExecPoll::Observe { output, .. }) => {
                        last_prompt_snapshot = output;
                        stable_prompt_polls = 0;
                    }
                    None => return,
                }

                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
            }

            let output = _this
                .update(cx, |_ws, cx| {
                    pane_for_fallback.recent_lines(50, cx).join("\n")
                })
                .unwrap_or_default();
            let _ = fallback_response_tx.try_send(TerminalExecResponse {
                output,
                exit_code: None,
            });
        })
        .detach();
    }

    pub(super) fn handle_pane_request_for_tab(
        &mut self,
        tab_idx: usize,
        req: con_agent::PaneRequest,
        cx: &mut Context<Self>,
    ) {
        use con_agent::{PaneInfo, PaneQuery, PaneResponse};

        let pane_tree = &self.tabs[tab_idx].pane_tree;
        let focused_pid = pane_tree.focused_pane_id();
        let all_terminals = pane_tree.all_terminals();

        let response = match req.query {
            PaneQuery::List => {
                self.reconcile_runtime_trackers_for_tab(tab_idx);
                let panes: Vec<PaneInfo> = all_terminals
                    .iter()
                    .enumerate()
                    .map(|(idx, terminal)| {
                        let pid = pane_tree.pane_id_for_terminal(terminal).unwrap_or(idx);
                        let (observation, runtime) = self.observe_terminal_runtime_for_tab(
                            tab_idx,
                            terminal,
                            Self::SECONDARY_PANE_OBSERVATION_LINES,
                            cx,
                        );
                        let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                        let remote_workspace =
                            con_agent::context::remote_workspace_anchor(&runtime, &observation);
                        let title = observation
                            .title
                            .clone()
                            .unwrap_or_else(|| format!("Pane {}", idx + 1));
                        let (cols, rows) = terminal.grid_size(cx);
                        PaneInfo {
                            index: idx + 1,
                            pane_id: pid,
                            title,
                            cwd: observation.cwd.clone(),
                            is_focused: pid == focused_pid,
                            rows,
                            cols,
                            surface_ready: terminal.surface_ready(cx),
                            is_alive: terminal.is_alive(cx),
                            hostname: runtime.remote_host.clone(),
                            hostname_confidence: runtime.remote_host_confidence,
                            hostname_source: runtime.remote_host_source,
                            remote_workspace,
                            front_state: runtime.front_state,
                            mode: runtime.mode,
                            shell_metadata_fresh: runtime.shell_metadata_fresh,
                            shell_context_fresh: runtime.shell_context_fresh,
                            observation_support: observation.support.clone(),
                            address_space: control.address_space,
                            visible_target: control.visible_target.clone(),
                            target_stack: control.target_stack.clone(),
                            tmux_control: control.tmux.clone(),
                            control_attachments: control.attachments.clone(),
                            control_channels: control.channels.clone(),
                            control_capabilities: control.capabilities.clone(),
                            control_notes: control.notes.clone(),
                            active_scope: runtime.active_scope.clone(),
                            agent_cli: runtime.agent_cli.clone(),
                            evidence: runtime.evidence.clone(),
                            runtime_stack: runtime.scope_stack,
                            last_verified_runtime_stack: runtime.last_verified_scope_stack,
                            runtime_warnings: runtime.warnings,
                            shell_context: runtime.shell_context.clone(),
                            recent_actions: runtime.recent_actions.clone(),
                            screen_hints: observation.screen_hints,
                            tmux_session: runtime.tmux_session,
                            has_shell_integration: observation.has_shell_integration,
                            last_command: observation.last_command,
                            last_exit_code: observation.last_exit_code,
                            is_busy: observation.is_busy,
                        }
                    })
                    .collect();
                PaneResponse::PaneList(panes)
            }
            PaneQuery::ReadContent { target, lines } => {
                match self.resolve_pane_target_for_tab(tab_idx, target) {
                    Ok(resolved) => {
                        let content = resolved.pane.recent_lines(lines, cx).join("\n");
                        PaneResponse::Content(content)
                    }
                    Err(err) => PaneResponse::Error(err),
                }
            }
            PaneQuery::SendKeys { target, keys } => {
                match self.resolve_pane_target_for_tab(tab_idx, target) {
                    Ok(resolved) => {
                        resolved.pane.write(keys.as_bytes(), cx);
                        self.record_runtime_event_for_terminal(
                            tab_idx,
                            &resolved.pane,
                            con_agent::context::PaneRuntimeEvent::RawInput {
                                keys: keys.clone(),
                                input_generation: resolved.pane.input_generation(cx),
                            },
                        );
                        PaneResponse::KeysSent
                    }
                    Err(err) => PaneResponse::Error(err),
                }
            }
            PaneQuery::SearchText {
                target,
                pattern,
                max_matches,
            } => {
                let targets: Vec<(usize, TerminalPane)> =
                    if target.pane_index.is_none() && target.pane_id.is_none() {
                        all_terminals
                            .iter()
                            .enumerate()
                            .map(|(i, t)| (i + 1, (*t).clone()))
                            .collect()
                    } else {
                        match self.resolve_pane_target_for_tab(tab_idx, target) {
                            Ok(resolved) => vec![(resolved.pane_index, resolved.pane)],
                            Err(err) => {
                                return {
                                    let _ = req.response_tx.send(PaneResponse::Error(err));
                                };
                            }
                        }
                    };

                let mut results = Vec::new();
                let remaining = max_matches;
                for (idx, terminal) in &targets {
                    let per_pane = remaining.saturating_sub(results.len());
                    if per_pane == 0 {
                        break;
                    }
                    for (line_num, text) in terminal.search_text(&pattern, per_pane, cx) {
                        results.push((*idx, line_num, text));
                    }
                }
                PaneResponse::SearchResults(results)
            }
            PaneQuery::InspectTmux { target } => {
                match self.resolve_pane_target_for_tab(tab_idx, target) {
                    Ok(resolved) => {
                        let (_, runtime) =
                            self.observe_terminal_runtime_for_tab(tab_idx, &resolved.pane, 20, cx);
                        let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                        if let Some(tmux) = control.tmux {
                            PaneResponse::TmuxInfo(tmux)
                        } else {
                            PaneResponse::Error(format!(
                                "Pane {} (id {}) is not currently in a tmux scope.",
                                resolved.pane_index, resolved.pane_id
                            ))
                        }
                    }
                    Err(err) => PaneResponse::Error(err),
                }
            }
            PaneQuery::TmuxList { pane: target } => match self
                .resolve_pane_target_for_tab(tab_idx, target)
            {
                Ok(resolved) => {
                    let (_, runtime) =
                        self.observe_terminal_runtime_for_tab(tab_idx, &resolved.pane, 20, cx);
                    let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                    let tmux_mode = control.tmux.as_ref().map(|tmux| tmux.mode);
                    if !control
                        .capabilities
                        .contains(&con_agent::PaneControlCapability::QueryTmux)
                    {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) does not currently expose tmux native query capability.\nvisible_target: {}\ntmux_mode: {}\ncontrol_attachments: {}\ncontrol_capabilities: {}",
                            resolved.pane_index,
                            resolved.pane_id,
                            control.visible_target.summary(),
                            tmux_mode.map(|mode| mode.as_str()).unwrap_or("none"),
                            con_agent::control::format_control_attachments(&control.attachments),
                            control
                                .capabilities
                                .iter()
                                .map(|capability| capability.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ))
                    } else if Self::pane_blocks_shell_anchor_control(&resolved.pane, cx) {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) is busy. Wait for the current command to finish before running tmux-native queries from its shell anchor.",
                            resolved.pane_index, resolved.pane_id
                        ))
                    } else {
                        let response_tx = req.response_tx;
                        let nonce = format!(
                            "tmux-list-{}-{}",
                            resolved.pane_id,
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|duration| duration.as_nanos())
                                .unwrap_or_default()
                        );
                        let command = con_agent::tmux::build_tmux_list_command(&nonce);
                        self.spawn_shell_anchor_command(
                            tab_idx,
                            resolved.pane,
                            resolved.pane_index,
                            command,
                            10,
                            move |lines| {
                                con_agent::tmux::parse_tmux_list_lines(&lines, &nonce)
                                    .map(con_agent::PaneResponse::TmuxList)
                            },
                            response_tx,
                            cx,
                        );
                        return;
                    }
                }
                Err(err) => PaneResponse::Error(err),
            },
            PaneQuery::TmuxCapture {
                pane: pane_target,
                target,
                lines,
            } => match self.resolve_pane_target_for_tab(tab_idx, pane_target) {
                Ok(resolved) => {
                    let (_, runtime) =
                        self.observe_terminal_runtime_for_tab(tab_idx, &resolved.pane, 20, cx);
                    let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                    if !control
                        .capabilities
                        .contains(&con_agent::PaneControlCapability::QueryTmux)
                    {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) does not currently expose tmux native query capability for capture.\nvisible_target: {}\ncontrol_capabilities: {}",
                            resolved.pane_index,
                            resolved.pane_id,
                            control.visible_target.summary(),
                            control
                                .capabilities
                                .iter()
                                .map(|capability| capability.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ))
                    } else if Self::pane_blocks_shell_anchor_control(&resolved.pane, cx) {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) is busy. Wait for the current command to finish before running tmux capture from its shell anchor.",
                            resolved.pane_index, resolved.pane_id
                        ))
                    } else {
                        let response_tx = req.response_tx;
                        let nonce = format!(
                            "tmux-capture-{}-{}",
                            resolved.pane_id,
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|duration| duration.as_nanos())
                                .unwrap_or_default()
                        );
                        let command = con_agent::tmux::build_tmux_capture_command(
                            &nonce,
                            target.as_deref(),
                            lines,
                        );
                        self.spawn_shell_anchor_command(
                            tab_idx,
                            resolved.pane,
                            resolved.pane_index,
                            command,
                            10,
                            move |lines| {
                                con_agent::tmux::parse_tmux_capture_lines(
                                    &lines,
                                    &nonce,
                                    target.as_deref(),
                                )
                                .map(con_agent::PaneResponse::TmuxCapture)
                            },
                            response_tx,
                            cx,
                        );
                        return;
                    }
                }
                Err(err) => PaneResponse::Error(err),
            },
            PaneQuery::TmuxSendKeys {
                pane: pane_target,
                target,
                literal_text,
                key_names,
                append_enter,
            } => match self.resolve_pane_target_for_tab(tab_idx, pane_target) {
                Ok(resolved) => {
                    let (_, runtime) =
                        self.observe_terminal_runtime_for_tab(tab_idx, &resolved.pane, 20, cx);
                    let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                    if !control
                        .capabilities
                        .contains(&con_agent::PaneControlCapability::SendTmuxKeys)
                    {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) does not currently expose tmux native send-keys capability.\nvisible_target: {}\ncontrol_capabilities: {}",
                            resolved.pane_index,
                            resolved.pane_id,
                            control.visible_target.summary(),
                            control
                                .capabilities
                                .iter()
                                .map(|capability| capability.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ))
                    } else if Self::pane_blocks_shell_anchor_control(&resolved.pane, cx) {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) is busy. Wait for the current command to finish before sending tmux-native keys from its shell anchor.",
                            resolved.pane_index, resolved.pane_id
                        ))
                    } else {
                        match con_agent::tmux::build_tmux_send_keys_command(
                            &target,
                            literal_text.as_deref(),
                            &key_names,
                            append_enter,
                        ) {
                            Ok(command) => {
                                let response_tx = req.response_tx;
                                self.spawn_shell_anchor_command(
                                    tab_idx,
                                    resolved.pane,
                                    resolved.pane_index,
                                    command,
                                    10,
                                    move |_lines| {
                                        Ok(con_agent::PaneResponse::Content(format!(
                                            "tmux send-keys delivered to target {}",
                                            target
                                        )))
                                    },
                                    response_tx,
                                    cx,
                                );
                                return;
                            }
                            Err(err) => PaneResponse::Error(err),
                        }
                    }
                }
                Err(err) => PaneResponse::Error(err),
            },
            PaneQuery::TmuxRunCommand {
                pane: pane_target,
                target,
                location,
                command,
                window_name,
                cwd,
                detached,
            } => match self.resolve_pane_target_for_tab(tab_idx, pane_target) {
                Ok(resolved) => {
                    let (_, runtime) =
                        self.observe_terminal_runtime_for_tab(tab_idx, &resolved.pane, 20, cx);
                    let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                    if !control
                        .capabilities
                        .contains(&con_agent::PaneControlCapability::ExecTmuxCommand)
                    {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) does not currently expose tmux native run-command capability.\nvisible_target: {}\ncontrol_capabilities: {}",
                            resolved.pane_index,
                            resolved.pane_id,
                            control.visible_target.summary(),
                            control
                                .capabilities
                                .iter()
                                .map(|capability| capability.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ))
                    } else if Self::pane_blocks_shell_anchor_control(&resolved.pane, cx) {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) is busy. Wait for the current command to finish before launching tmux-native commands from its shell anchor.",
                            resolved.pane_index, resolved.pane_id
                        ))
                    } else {
                        let response_tx = req.response_tx;
                        let nonce = format!(
                            "tmux-exec-{}-{}",
                            resolved.pane_id,
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|duration| duration.as_nanos())
                                .unwrap_or_default()
                        );
                        let shell_command = con_agent::tmux::build_tmux_exec_command(
                            &nonce,
                            location,
                            target.as_deref(),
                            &command,
                            window_name.as_deref(),
                            cwd.as_deref(),
                            detached,
                        );
                        self.spawn_shell_anchor_command(
                            tab_idx,
                            resolved.pane,
                            resolved.pane_index,
                            shell_command,
                            12,
                            move |lines| {
                                con_agent::tmux::parse_tmux_exec_lines(
                                    &lines, &nonce, location, detached,
                                )
                                .map(con_agent::PaneResponse::TmuxExec)
                            },
                            response_tx,
                            cx,
                        );
                        return;
                    }
                }
                Err(err) => PaneResponse::Error(err),
            },
            PaneQuery::ProbeShellContext { target } => match self
                .resolve_pane_target_for_tab(tab_idx, target)
            {
                Ok(resolved) => {
                    let pane_index = resolved.pane_index;
                    let pane_id = resolved.pane_id;
                    let pane = resolved.pane;
                    let (_, runtime) =
                        self.observe_terminal_runtime_for_tab(tab_idx, &pane, 20, cx);
                    let control = con_agent::control::PaneControlState::from_runtime(&runtime);
                    if !control.allows_shell_probe() {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) does not currently expose the probe_shell_context capability. \
                             It must be a proven fresh shell prompt before shell-scoped probing is allowed.\n\
                             visible_target: {}\ncontrol_attachments: {}\ncontrol_capabilities: {}",
                            pane_index,
                            pane_id,
                            control.visible_target.summary(),
                            con_agent::control::format_control_attachments(&control.attachments),
                            control
                                .capabilities
                                .iter()
                                .map(|capability| capability.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        ))
                    } else if Self::pane_blocks_shell_anchor_control(&pane, cx) {
                        PaneResponse::Error(format!(
                            "Pane {} (id {}) is busy. Wait for the current command to finish before running a shell probe.",
                            pane_index, pane_id
                        ))
                    } else {
                        let response_tx = req.response_tx;
                        let nonce = format!(
                            "{}-{}",
                            pane_id,
                            std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|duration| duration.as_nanos())
                                .unwrap_or_default()
                        );
                        let command = con_agent::shell_probe::build_shell_probe_command(&nonce);
                        let _ = pane.take_command_finished(cx);
                        pane.write(format!("{command}\n").as_bytes(), cx);

                        cx.spawn(async move |this, cx| {
                            let deadline = std::time::Instant::now()
                                + std::time::Duration::from_secs(10);

                            loop {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(250))
                                    .await;

                                if std::time::Instant::now() >= deadline {
                                    let _ = response_tx.send(PaneResponse::Error(format!(
                                        "Shell probe timed out in pane {} (id {}) after 10s.",
                                        pane_index, pane_id
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

                                let lines = this
                                    .update(cx, |_, cx| pane.recent_lines(200, cx))
                                    .unwrap_or_default();
                                match con_agent::shell_probe::parse_shell_probe_lines(&lines, &nonce)
                                {
                                    Ok(result) => {
                                        let recorded_result = result.clone();
                                        let _ = this.update(cx, |workspace, cx| {
                                            workspace.record_runtime_event_for_terminal(
                                                tab_idx,
                                                &pane,
                                                con_agent::context::PaneRuntimeEvent::ShellProbe {
                                                    result: recorded_result,
                                                    captured_input_generation: pane.input_generation(cx),
                                                },
                                            );
                                        });
                                        let _ = response_tx.send(PaneResponse::ShellProbe(result));
                                    }
                                    Err(err) => {
                                        let excerpt = lines.join("\n");
                                        let _ = response_tx.send(PaneResponse::Error(format!(
                                            "Shell probe finished in pane {} (id {}) but the probe output could not be parsed: {}\nRecent output:\n{}",
                                            pane_index, pane_id, err, excerpt
                                        )));
                                    }
                                }
                                return;
                            }
                        })
                        .detach();
                        return;
                    }
                }
                Err(err) => PaneResponse::Error(err),
            },
            PaneQuery::CheckBusy { target } => {
                match self.resolve_pane_target_for_tab(tab_idx, target) {
                    Ok(resolved) => PaneResponse::BusyStatus {
                        surface_ready: resolved.pane.surface_ready(cx),
                        is_alive: resolved.pane.is_alive(cx),
                        is_busy: resolved.pane.is_busy(cx),
                        has_shell_integration: resolved.pane.has_shell_integration(cx),
                    },
                    Err(err) => PaneResponse::Error(err),
                }
            }
            PaneQuery::WaitFor {
                target,
                timeout_secs,
                pattern,
            } => {
                // Normalize empty pattern to None — "".contains("") is always true in Rust,
                // making empty pattern match instantly (useless). Treat as idle/quiescence.
                let pattern = pattern.filter(|p| !p.is_empty());

                log::info!(
                    "[wait_for] target={} timeout={:?} pattern={:?}",
                    target.describe(),
                    timeout_secs,
                    pattern,
                );

                match self.resolve_pane_target_for_tab(tab_idx, target) {
                    Ok(resolved) => {
                        let pane = resolved.pane;
                        let has_si = pane.has_shell_integration(cx);
                        let timeout = timeout_secs.unwrap_or(30).min(120);
                        let response_tx = req.response_tx;

                        log::info!("[wait_for] has_si={} is_busy={}", has_si, pane.is_busy(cx),);

                        // Check if already in target state before spawning async task
                        if pattern.is_none() && has_si && !pane.is_busy(cx) {
                            log::info!("[wait_for] → early idle return");
                            let output = pane.recent_lines(50, cx).join("\n");
                            let _ = response_tx.send(PaneResponse::WaitComplete {
                                status: "idle".into(),
                                output,
                            });
                            return;
                        }
                        if let Some(ref pat) = pattern {
                            let content = pane.recent_lines(50, cx).join("\n");
                            if content.contains(pat.as_str()) {
                                let _ = response_tx.send(PaneResponse::WaitComplete {
                                    status: "matched".into(),
                                    output: content,
                                });
                                return;
                            }
                        }

                        let use_quiescence = !has_si && pattern.is_none();

                        // Spawn async task — direct terminal access, no channel overhead
                        cx.spawn(async move |this, cx| {
                        let deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(timeout as u64);

                        // Three modes:
                        // 1. Shell integration idle: 100ms polling, check is_busy/command_finished
                        // 2. Pattern match: 500ms polling, check content
                        // 3. Quiescence (no SI, no pattern): 500ms polling, detect output stable for 2s
                        let interval: u64 = if has_si && pattern.is_none() { 100 } else { 500 };

                        // Normalize terminal output for stable comparison.
                        // ghostty_surface_read_text returns lines with trailing whitespace that
                        // varies with cursor position — trim each line to get content-only text.
                        let normalize_output = |lines: Vec<String>| -> String {
                            lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n")
                        };

                        // For quiescence mode: capture baseline INSIDE async task to
                        // avoid race between snapshot and first poll.
                        // 4 polls × 500ms = 2s — fast enough for interactive use,
                        // long enough to avoid false positives from progress output.
                        const QUIET_THRESHOLD: u32 = 4;
                        let mut last_snapshot = if use_quiescence {
                            match this.update(cx, |_, cx| normalize_output(pane.recent_lines(50, cx))) {
                                Ok(s) if !s.is_empty() => s,
                                _ => {
                                    // Terminal not ready yet — use sentinel that won't match real output.
                                    // First real poll will set the actual baseline.
                                    log::info!("[wait_for] quiescence: empty baseline, deferring to first poll");
                                    String::new()
                                }
                            }
                        } else {
                            String::new()
                        };
                        let mut stable_count: u32 = 0;

                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(interval))
                                .await;

                            if std::time::Instant::now() >= deadline {
                                let output = this
                                    .update(cx, |_, cx| pane.recent_lines(50, cx).join("\n"))
                                    .unwrap_or_default();
                                let _ = response_tx.send(PaneResponse::WaitComplete {
                                    status: "timeout".into(),
                                    output,
                                });
                                return;
                            }

                            let done = this
                                .update(cx, |_, cx| {
                                    if let Some(ref pat) = pattern {
                                        // Pattern mode
                                        let content = pane.recent_lines(50, cx).join("\n");
                                        if content.contains(pat.as_str()) {
                                            Some(("matched".to_string(), content))
                                        } else {
                                            None
                                        }
                                    } else if has_si {
                                        // Shell integration idle mode
                                        if pane.take_command_finished(cx).is_some()
                                            || !pane.is_busy(cx)
                                        {
                                            let output = pane.recent_lines(50, cx).join("\n");
                                            Some(("idle".to_string(), output))
                                        } else {
                                            None
                                        }
                                    } else {
                                        // Quiescence mode: output unchanged for QUIET_THRESHOLD polls
                                        let is_alive = pane.is_alive(cx);
                                        let surface_ready = pane.surface_ready(cx);
                                        if !is_alive || !surface_ready {
                                            // A pane can exist before its terminal surface is bootstrapped.
                                            // Never report quiescent-idle until the pane is both alive and ready.
                                            last_snapshot.clear();
                                            stable_count = 0;
                                            None
                                        } else {
                                            let current = normalize_output(pane.recent_lines(50, cx));
                                            if current.is_empty() {
                                                // Terminal not producing output yet — don't count as stable
                                                None
                                            } else if last_snapshot.is_empty() {
                                                // First non-empty snapshot — set baseline, start counting
                                                log::info!("[wait_for] quiescence: baseline set ({} bytes)", current.len());
                                                last_snapshot = current;
                                                stable_count = 0;
                                                None
                                            } else if current == last_snapshot {
                                                stable_count += 1;
                                                log::info!("[wait_for] quiescence: stable {}/{}", stable_count, QUIET_THRESHOLD);
                                                if stable_count >= QUIET_THRESHOLD {
                                                    Some(("idle".to_string(), current))
                                                } else {
                                                    None
                                                }
                                            } else {
                                                log::info!("[wait_for] quiescence: output changed, resetting (stable was {})", stable_count);
                                                last_snapshot = current;
                                                stable_count = 0;
                                                None
                                            }
                                        }
                                    }
                                })
                                .ok()
                                .flatten();

                            if let Some((status, output)) = done {
                                let _ = response_tx.send(PaneResponse::WaitComplete {
                                    status,
                                    output,
                                });
                                return;
                            }
                        }
                    })
                    .detach();
                        return; // Response sent by spawned task
                    }
                    Err(err) => PaneResponse::Error(err),
                }
            }
            PaneQuery::CreatePane { command, location } => {
                // Creating a terminal requires a Window, so defer into an explicit
                // window-aware callback instead of depending on a later render.
                let cwd = self.tabs[tab_idx]
                    .pane_tree
                    .try_focused_terminal()
                    .and_then(|t| t.current_dir(cx));
                self.pending_create_pane_requests.push(PendingCreatePane {
                    command,
                    cwd,
                    tab_idx,
                    location,
                    response_tx: req.response_tx,
                });
                self.schedule_pending_create_pane_flush(cx);
                return;
            }
        };

        let _ = req.response_tx.send(response);
    }
}
