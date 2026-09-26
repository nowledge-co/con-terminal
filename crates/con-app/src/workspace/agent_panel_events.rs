use super::*;

impl ConWorkspace {
    pub(super) fn on_new_conversation(
        &mut self,
        _panel: &Entity<AgentPanel>,
        _event: &NewConversation,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tabs[self.active_tab].session.new_conversation();
        self.agent_panel.update(cx, |panel, cx| {
            panel.clear_messages(cx);
        });
        self.save_session(cx);
    }

    pub(super) fn on_load_conversation(
        &mut self,
        _panel: &Entity<AgentPanel>,
        event: &LoadConversation,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tabs[self.active_tab]
            .session
            .load_conversation(&event.id)
        {
            // Rebuild panel state from the loaded conversation
            let conv = self.tabs[self.active_tab].session.conversation();
            let conv = conv.lock();
            let new_state = self.panel_state_from_conversation(&conv);
            drop(conv);
            self.agent_panel.update(cx, |panel, cx| {
                panel.swap_state(new_state, cx);
            });
            self.save_session(cx);
        }
    }

    pub(super) fn on_delete_conversation(
        &mut self,
        _panel: &Entity<AgentPanel>,
        event: &DeleteConversation,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(e) = con_agent::Conversation::delete(&event.id) {
            log::warn!("Failed to delete conversation: {}", e);
        }
        // Refresh the conversation list in the agent panel
        self.agent_panel.update(cx, |panel, cx| {
            panel.refresh_conversation_list(cx);
        });
    }

    pub(super) fn on_inline_input_submit(
        &mut self,
        _panel: &Entity<AgentPanel>,
        event: &InlineInputSubmit,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Forward inline input as an agent message
        self.send_to_agent(&event.text, cx);
        cx.notify();
    }

    pub(super) fn on_inline_skill_autocomplete_changed(
        &mut self,
        _panel: &Entity<AgentPanel>,
        _event: &InlineSkillAutocompleteChanged,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
    }

    pub(super) fn on_cancel_request(
        &mut self,
        _panel: &Entity<AgentPanel>,
        _event: &CancelRequest,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.tabs[self.active_tab].session.cancel_current();
        self.agent_panel.update(_cx, |panel, cx| panel.stop(cx));
    }

    pub(super) fn on_set_auto_approve(
        &mut self,
        _panel: &Entity<AgentPanel>,
        event: &SetAutoApprove,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.harness.set_auto_approve(event.enabled);
    }

    pub(super) fn on_select_session_model(
        &mut self,
        _panel: &Entity<AgentPanel>,
        event: &SelectSessionModel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let provider = self.tab_agent_config(self.active_tab).provider.clone();
        self.set_tab_model_override(self.active_tab, provider.clone(), event.model.clone());
        let config = self.tab_agent_config(self.active_tab);
        let available_models = self.provider_models_for_config(&config);

        self.agent_panel.update(cx, |panel, cx| {
            panel.set_session_provider_options(
                AgentPanel::configured_session_providers(&config),
                window,
                cx,
            );
            panel.set_model_name(event.model.clone(), cx);
            panel.set_session_model_options(available_models, window, cx);
        });

        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn on_select_session_provider(
        &mut self,
        _panel: &Entity<AgentPanel>,
        event: &SelectSessionProvider,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_tab_provider_override(self.active_tab, event.provider.clone());
        let config = self.tab_agent_config(self.active_tab);

        let provider = config.provider.clone();
        let model_name = AgentHarness::active_model_name_for(&config);
        let available_models = self.provider_models_for_config(&config);

        self.agent_panel.update(cx, |panel, cx| {
            panel.set_session_provider_options(
                AgentPanel::configured_session_providers(&config),
                window,
                cx,
            );
            panel.set_provider_name(provider.clone(), window, cx);
            panel.set_model_name(model_name, cx);
            panel.set_session_model_options(available_models, window, cx);
        });

        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn on_rerun_from_message(
        &mut self,
        _panel: &Entity<AgentPanel>,
        event: &RerunFromMessage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The panel already truncated its messages and re-added the user message.
        // Now sync the underlying conversation and re-send to agent.
        let panel_msg_count = self.agent_panel.read(cx).state().message_count();
        // Truncate conversation to match panel state (minus the re-added user message)
        let conv = self.tabs[self.active_tab].session.conversation();
        conv.lock().truncate_to(panel_msg_count.saturating_sub(1));

        let context = self.build_agent_context(cx);
        let session = &self.tabs[self.active_tab].session;
        let agent_config = self.active_tab_agent_config();
        if event.content.trim().starts_with('/') {
            match self.harness.classify_input(
                &event.content,
                self.try_active_terminal()
                    .and_then(|t| self.effective_remote_host_for_tab(self.active_tab, t, cx))
                    .is_some(),
            ) {
                InputKind::SkillInvoke(name, args) => {
                    if let Some(desc) = self.harness.invoke_skill(
                        session,
                        agent_config.clone(),
                        &name,
                        args.as_deref(),
                        context,
                    ) {
                        self.agent_panel.update(cx, |panel, cx| {
                            panel.add_step(&desc, cx);
                        });
                    }
                }
                _ => self.harness.send_message(
                    session,
                    agent_config.clone(),
                    event.content.clone(),
                    context,
                ),
            }
        } else {
            self.harness
                .send_message(session, agent_config, event.content.clone(), context);
        }
    }
}
