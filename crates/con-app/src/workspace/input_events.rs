use super::*;

impl ConWorkspace {
    pub(super) fn on_input_escape(
        &mut self,
        _input_bar: &Entity<InputBar>,
        _event: &EscapeInput,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pane_scope_picker_open {
            self.pane_scope_picker_open = false;
            cx.notify();
        }
    }

    pub(super) fn on_skill_autocomplete_changed(
        &mut self,
        _input_bar: &Entity<InputBar>,
        _event: &SkillAutocompleteChanged,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
    }

    pub(super) fn on_toggle_pane_scope_picker_requested(
        &mut self,
        _input_bar: &Entity<InputBar>,
        _event: &TogglePaneScopePickerRequested,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_pane_scope_picker(&TogglePaneScopePicker, window, cx);
    }

    pub(super) fn on_input_edited(
        &mut self,
        _input_bar: &Entity<InputBar>,
        _event: &InputEdited,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_input_suggestion(window, cx);
        cx.notify();
    }

    pub(super) fn on_input_scope_changed(
        &mut self,
        _input_bar: &Entity<InputBar>,
        _event: &InputScopeChanged,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_active_terminal_focus_states(cx);
        cx.notify();
    }

    pub(super) fn on_input_submit(
        &mut self,
        input_bar: &Entity<InputBar>,
        _event: &SubmitInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_scope_picker_open = false;
        let (content, mode) = input_bar.update(cx, |bar, cx| {
            let content = bar.take_content(window, cx);
            bar.clear_completion_ui();
            (content, bar.mode())
        });

        if content.trim().is_empty() {
            return;
        }

        self.record_input_history(&content);
        let recent_inputs = self.recent_input_history(80);
        input_bar.update(cx, |bar, cx| bar.set_recent_commands(recent_inputs, cx));

        match mode {
            InputMode::Shell => {
                self.execute_shell(&content, window, cx);
            }
            InputMode::Agent => {
                self.send_to_agent(&content, cx);
            }
            InputMode::Smart => {
                let is_remote = self
                    .try_active_terminal()
                    .and_then(|t| self.effective_remote_host_for_tab(self.active_tab, t, cx))
                    .is_some();
                match self.harness.classify_input(&content, is_remote) {
                    InputKind::ShellCommand(cmd) => {
                        self.execute_shell(&cmd, window, cx);
                    }
                    InputKind::NaturalLanguage(text) => {
                        self.send_to_agent(&text, cx);
                    }
                    InputKind::SkillInvoke(name, args) => {
                        let context = self.build_agent_context(cx);
                        let session = &self.tabs[self.active_tab].session;
                        let agent_config = self.active_tab_agent_config();
                        if let Some(desc) = self.harness.invoke_skill(
                            session,
                            agent_config,
                            &name,
                            args.as_deref(),
                            context,
                        ) {
                            if !self.agent_panel_open {
                                self.agent_panel_open = true;
                            }
                            self.agent_panel.update(cx, |panel, cx| {
                                let label = format!("/{name}");
                                panel.add_message("user", &label, cx);
                                panel.add_step(&desc, cx);
                            });
                        }
                    }
                }
            }
        }

        cx.notify();
    }

    pub(super) fn handle_harness_event(&mut self, event: HarnessEvent, cx: &mut Context<Self>) {
        match event {
            HarnessEvent::Thinking => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.add_step("Thinking...", cx);
                });
            }
            HarnessEvent::ThinkingDelta(text) => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.update_thinking(&text, cx);
                });
            }
            HarnessEvent::Step(step) => {
                let step_text = crate::agent_panel::describe_agent_step(&step);
                self.agent_panel.update(cx, |panel, cx| {
                    panel.add_step(&step_text, cx);
                });
            }
            HarnessEvent::Token(token) => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.update_streaming(&token, cx);
                });
            }
            HarnessEvent::ToolCallStart {
                call_id,
                tool_name,
                args,
            } => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.add_tool_call(&call_id, &tool_name, &args, cx);
                });
            }
            HarnessEvent::ToolApprovalNeeded {
                call_id,
                tool_name,
                args,
                approval_tx,
            } => {
                // The hook's request-time policy owns this decision. A later
                // global config change must not bypass an approval already in flight.
                self.agent_panel.update(cx, |panel, cx| {
                    panel.add_pending_approval(&call_id, &tool_name, &args, approval_tx, cx);
                });
            }
            HarnessEvent::ToolApprovalEnded {
                call_id,
                approval_tx,
            } => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.finish_approval(&approval_tx, &call_id, cx);
                });
            }
            HarnessEvent::ToolCallComplete {
                call_id,
                tool_name,
                result,
            } => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.complete_tool_call(&call_id, &tool_name, &result, cx);
                });
            }
            HarnessEvent::RequestFinished { approval_tx } => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.finish_request(&approval_tx, cx);
                });
            }
            HarnessEvent::ResponseComplete(msg) => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.complete_response(&msg, cx);
                });
            }
            HarnessEvent::Error(err) => {
                self.agent_panel.update(cx, |panel, cx| {
                    panel.add_message("system", &format!("Error: {}", err), cx);
                });
            }
            HarnessEvent::SkillsUpdated(_) => {}
        }
        cx.notify();
    }
}
