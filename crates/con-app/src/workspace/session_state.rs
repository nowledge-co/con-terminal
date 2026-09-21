use super::*;

impl ConWorkspace {
    pub(super) fn panel_state_from_conversation(&self, conv: &Conversation) -> PanelState {
        let mut state = PanelState::new();
        for msg in &conv.messages {
            match msg.role {
                con_agent::MessageRole::User => {
                    let visible = self
                        .harness
                        .display_label_for_user_message(&msg.content)
                        .unwrap_or_else(|| msg.content.clone());
                    state.restore_message("user", &visible, None, None);
                }
                con_agent::MessageRole::Assistant => {
                    state.restore_message(
                        "assistant",
                        &msg.content,
                        msg.model.as_deref(),
                        msg.duration_ms,
                    );
                    state.restore_last_assistant_trace(msg.thinking.as_deref(), &msg.steps);
                }
                con_agent::MessageRole::System | con_agent::MessageRole::Tool => {
                    state.restore_message("system", &msg.content, None, None);
                }
            }
        }
        state
    }

    pub(super) fn snapshot_session(&self, cx: &App) -> Session {
        self.snapshot_session_with_options(cx, self.config.appearance.restore_terminal_text)
    }

    pub(super) fn snapshot_session_with_options(
        &self,
        cx: &App,
        capture_screen_text: bool,
    ) -> Session {
        let tabs: Vec<con_core::session::TabState> = self
            .tabs
            .iter()
            .map(|tab| {
                let focused_terminal = tab.pane_tree.try_visible_focus_terminal();
                let terminal = focused_terminal.map(|(_, terminal)| terminal);
                let cwd = terminal.and_then(|t| t.current_dir(cx));
                let title = terminal
                    .and_then(|t| t.title(cx))
                    .unwrap_or_else(|| tab.title.clone());
                let pane_layout = tab.pane_tree.to_persisted_state(cx, capture_screen_text);
                let focused_pane_id = focused_pane_id_for_persisted_layout(
                    tab.pane_tree.focused_pane_id(),
                    pane_layout.as_ref(),
                );
                let pane_states = tab
                    .pane_tree
                    .pane_terminals()
                    .into_iter()
                    .map(|(_, terminal)| con_core::session::PaneState {
                        cwd: terminal.current_dir(cx),
                    })
                    .collect();
                let shell_history = tab
                    .shell_history
                    .iter()
                    .map(
                        |(pane_id, entries)| con_core::session::PaneCommandHistoryState {
                            pane_id: Some(*pane_id),
                            entries: entries
                                .iter()
                                .map(|entry| con_core::session::CommandHistoryEntryState {
                                    command: entry.command.clone(),
                                    cwd: entry.cwd.clone(),
                                })
                                .collect(),
                        },
                    )
                    .collect();
                con_core::session::TabState {
                    title,
                    cwd,
                    layout: pane_layout,
                    focused_pane_id: Some(focused_pane_id),
                    panes: pane_states,
                    shell_history,
                    conversation_id: Some(tab.session.conversation_id()),
                    agent_routing: tab.agent_routing.clone(),
                    user_label: tab.user_label.clone(),
                    color: tab.color,
                }
            })
            .collect();

        Session {
            tabs,
            active_tab: self.active_tab,
            agent_panel_open: self.agent_panel_open,
            agent_panel_width: Some(self.agent_panel_width),
            input_bar_visible: self.input_bar_visible,
            global_shell_history: self
                .global_shell_history
                .iter()
                .map(|entry| con_core::session::CommandHistoryEntryState {
                    command: entry.command.clone(),
                    cwd: entry.cwd.clone(),
                })
                .collect(),
            input_history: self.global_input_history.iter().cloned().collect(),
            conversation_id: None, // deprecated — per-tab now
            left_panel_width: Some(self.sidebar.read(cx).panel_width()),
            vertical_tabs_pinned: Some(self.sidebar.read(cx).is_pinned()),
            activity_slot: Some(self.activity_slot.as_str().to_string()),
            left_panel_open: Some(self.left_panel_open),
            editor_area_height: None,
        }
    }

    pub(super) fn snapshot_global_history(&self) -> GlobalHistoryState {
        GlobalHistoryState {
            global_shell_history: self
                .global_shell_history
                .iter()
                .map(|entry| con_core::session::CommandHistoryEntryState {
                    command: entry.command.clone(),
                    cwd: entry.cwd.clone(),
                })
                .collect(),
            input_history: self.global_input_history.iter().cloned().collect(),
        }
    }

    pub(super) fn save_session(&self, cx: &App) {
        let session = self.snapshot_session(cx);
        let history = self.snapshot_global_history();
        if let Err(err) = self
            .session_save_tx
            .send(SessionSaveRequest::Save(session, history))
        {
            log::warn!("Failed to queue session save: {}", err);
        }
    }

    pub(super) fn flush_session_save(&self, cx: &App) {
        let session = self.snapshot_session(cx);
        let history = self.snapshot_global_history();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        if let Err(err) = self.session_save_tx.send(SessionSaveRequest::Flush(
            session.clone(),
            history.clone(),
            done_tx,
        )) {
            log::warn!("Failed to flush session save queue: {}", err);
            if let Err(save_err) = session.save() {
                log::warn!("Failed to save session directly during flush: {}", save_err);
            }
            if let Err(save_err) = history.save() {
                log::warn!(
                    "Failed to save command history directly during flush: {}",
                    save_err
                );
            }
            return;
        }

        if let Err(err) = done_rx.recv_timeout(Duration::from_secs(2)) {
            log::warn!("Timed out waiting for session save flush: {}", err);
            if let Err(save_err) = session.save() {
                log::warn!(
                    "Failed to save session directly after flush timeout: {}",
                    save_err
                );
            }
            if let Err(save_err) = history.save() {
                log::warn!(
                    "Failed to save command history directly after flush timeout: {}",
                    save_err
                );
            }
        }
    }

    pub(super) fn restore_shell_history(
        tab_state: &con_core::session::TabState,
    ) -> HashMap<usize, VecDeque<CommandSuggestionEntry>> {
        let mut restored = HashMap::new();

        for pane_history in &tab_state.shell_history {
            let Some(pane_id) = pane_history.pane_id else {
                continue;
            };
            let entries = pane_history
                .entries
                .iter()
                .filter(|entry| !entry.command.trim().is_empty())
                .map(|entry| CommandSuggestionEntry {
                    command: entry.command.trim().to_string(),
                    cwd: entry.cwd.clone(),
                })
                .collect::<VecDeque<_>>();
            if !entries.is_empty() {
                restored.insert(pane_id, entries);
            }
        }

        restored
    }

    pub(super) fn restore_global_shell_history(
        session: &con_core::session::Session,
        tabs: &[Tab],
    ) -> VecDeque<CommandSuggestionEntry> {
        let from_session: VecDeque<_> = session
            .global_shell_history
            .iter()
            .filter_map(|entry| {
                let command = entry.command.trim();
                (!command.is_empty()).then(|| CommandSuggestionEntry {
                    command: command.to_string(),
                    cwd: entry.cwd.clone(),
                })
            })
            .collect();
        if !from_session.is_empty() {
            return from_session;
        }

        let mut aggregated = VecDeque::new();
        for tab in tabs {
            for entries in tab.shell_history.values() {
                for entry in entries {
                    if let Some(existing_idx) =
                        aggregated
                            .iter()
                            .position(|existing: &CommandSuggestionEntry| {
                                existing.command == entry.command
                            })
                    {
                        aggregated.remove(existing_idx);
                    }
                    aggregated.push_back(entry.clone());
                    while aggregated.len() > MAX_GLOBAL_SHELL_HISTORY {
                        aggregated.pop_front();
                    }
                }
            }
        }
        aggregated
    }

    pub(super) fn default_agent_routing(config: &AgentConfig) -> AgentRoutingState {
        let mut model_overrides = Vec::new();
        for provider in [
            ProviderKind::Anthropic,
            ProviderKind::OpenAI,
            ProviderKind::ChatGPT,
            ProviderKind::GitHubCopilot,
            ProviderKind::OpenAICompatible,
            ProviderKind::MiniMax,
            ProviderKind::MiniMaxAnthropic,
            ProviderKind::Moonshot,
            ProviderKind::MoonshotAnthropic,
            ProviderKind::ZAI,
            ProviderKind::ZAIAnthropic,
            ProviderKind::DeepSeek,
            ProviderKind::Groq,
            ProviderKind::Cohere,
            ProviderKind::Gemini,
            ProviderKind::Ollama,
            ProviderKind::OpenRouter,
            ProviderKind::Perplexity,
            ProviderKind::Mistral,
            ProviderKind::Together,
            ProviderKind::XAI,
        ] {
            if let Some(model) = config
                .providers
                .get(&provider)
                .and_then(|entry| entry.model.clone())
            {
                model_overrides.push(AgentModelOverrideState { provider, model });
            }
        }

        AgentRoutingState {
            provider: Some(config.provider.clone()),
            model_overrides,
        }
    }

    pub(super) fn apply_agent_routing(
        base: &AgentConfig,
        routing: &AgentRoutingState,
    ) -> AgentConfig {
        let mut config = base.clone();
        if let Some(provider) = routing.provider.as_ref() {
            config.provider = provider.clone();
        }

        for override_state in &routing.model_overrides {
            if override_state.model.trim().is_empty() {
                continue;
            }
            let mut provider_config = config.providers.get_or_default(&override_state.provider);
            provider_config.model = Some(override_state.model.clone());
            config
                .providers
                .set(&override_state.provider, provider_config);
        }

        config
    }

    pub(super) fn tab_agent_config(&self, tab_idx: usize) -> AgentConfig {
        Self::apply_agent_routing(self.harness.config(), &self.tabs[tab_idx].agent_routing)
    }

    pub(super) fn active_tab_agent_config(&self) -> AgentConfig {
        self.tab_agent_config(self.active_tab)
    }

    pub(super) fn provider_models_for_config(&self, config: &AgentConfig) -> Vec<String> {
        self.model_registry.models_for_config(
            &config.provider,
            &config.providers.get_or_default(&config.provider),
        )
    }

    pub(super) fn set_tab_provider_override(&mut self, tab_idx: usize, provider: ProviderKind) {
        self.tabs[tab_idx].agent_routing.provider = Some(provider);
    }

    pub(super) fn set_tab_model_override(
        &mut self,
        tab_idx: usize,
        provider: ProviderKind,
        model: String,
    ) {
        let routing = &mut self.tabs[tab_idx].agent_routing;
        if let Some(existing) = routing
            .model_overrides
            .iter_mut()
            .find(|entry| entry.provider == provider)
        {
            existing.model = model;
            return;
        }

        routing
            .model_overrides
            .push(AgentModelOverrideState { provider, model });
    }

    pub(super) fn merge_shell_histories(
        mut restored: VecDeque<CommandSuggestionEntry>,
        persisted_history: &GlobalHistoryState,
    ) -> VecDeque<CommandSuggestionEntry> {
        for entry in &persisted_history.global_shell_history {
            let command = entry.command.trim();
            if command.is_empty() {
                continue;
            }
            if let Some(existing_idx) = restored
                .iter()
                .position(|existing| existing.command == command)
            {
                restored.remove(existing_idx);
            }
            restored.push_back(CommandSuggestionEntry {
                command: command.to_string(),
                cwd: entry.cwd.clone(),
            });
            while restored.len() > MAX_GLOBAL_SHELL_HISTORY {
                restored.pop_front();
            }
        }
        restored
    }

    pub(super) fn restore_global_input_history(
        session: &con_core::session::Session,
        persisted_history: &GlobalHistoryState,
        shell_history: &VecDeque<CommandSuggestionEntry>,
    ) -> VecDeque<String> {
        let mut restored = VecDeque::new();
        for entry in session
            .input_history
            .iter()
            .chain(persisted_history.input_history.iter())
        {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some(existing_idx) = restored
                .iter()
                .position(|existing: &String| existing == trimmed)
            {
                restored.remove(existing_idx);
            }
            restored.push_back(trimmed.to_string());
            while restored.len() > MAX_GLOBAL_INPUT_HISTORY {
                restored.pop_front();
            }
        }

        if !restored.is_empty() {
            return restored;
        }

        shell_history
            .iter()
            .filter_map(|entry| {
                let trimmed = entry.command.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            })
            .collect()
    }
}

fn focused_pane_id_for_persisted_layout(
    focused_pane_id: usize,
    layout: Option<&PaneLayoutState>,
) -> usize {
    let Some(layout) = layout else {
        return focused_pane_id;
    };
    if layout_contains_pane(layout, focused_pane_id) {
        return focused_pane_id;
    }
    first_pane_id_in_layout(layout).unwrap_or(focused_pane_id)
}

fn layout_contains_pane(layout: &PaneLayoutState, pane_id: usize) -> bool {
    match layout {
        PaneLayoutState::Leaf {
            pane_id: leaf_id, ..
        } => *leaf_id == pane_id,
        PaneLayoutState::Split { first, second, .. } => {
            layout_contains_pane(first, pane_id) || layout_contains_pane(second, pane_id)
        }
    }
}

fn first_pane_id_in_layout(layout: &PaneLayoutState) -> Option<usize> {
    match layout {
        PaneLayoutState::Leaf { pane_id, .. } => Some(*pane_id),
        PaneLayoutState::Split { first, second, .. } => {
            first_pane_id_in_layout(first).or_else(|| first_pane_id_in_layout(second))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::focused_pane_id_for_persisted_layout;
    use con_core::session::{PaneLayoutState, PaneSplitDirection};

    fn leaf(pane_id: usize) -> PaneLayoutState {
        PaneLayoutState::Leaf {
            pane_id,
            cwd: None,
            active_surface_id: None,
            surfaces: Vec::new(),
        }
    }

    #[test]
    fn persisted_focus_prefers_focused_pane_when_layout_contains_it() {
        let layout = PaneLayoutState::Split {
            direction: PaneSplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(leaf(1)),
            second: Box::new(leaf(2)),
        };

        assert_eq!(focused_pane_id_for_persisted_layout(2, Some(&layout)), 2);
    }

    #[test]
    fn persisted_focus_falls_back_when_focused_pane_is_not_serialized() {
        let layout = PaneLayoutState::Split {
            direction: PaneSplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(leaf(1)),
            second: Box::new(leaf(2)),
        };

        assert_eq!(focused_pane_id_for_persisted_layout(99, Some(&layout)), 1);
    }
}
