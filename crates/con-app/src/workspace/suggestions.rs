use super::*;

impl ConWorkspace {
    pub(super) fn record_shell_command(
        &mut self,
        tab_idx: usize,
        pane_id: usize,
        command: &str,
        cwd: Option<String>,
    ) {
        let trimmed = command.trim();
        if trimmed.is_empty() || tab_idx >= self.tabs.len() {
            return;
        }

        let history = self.tabs[tab_idx].shell_history.entry(pane_id).or_default();
        if let Some(existing_idx) = history.iter().position(|entry| entry.command == trimmed) {
            history.remove(existing_idx);
        }
        history.push_back(CommandSuggestionEntry {
            command: trimmed.to_string(),
            cwd: cwd.clone(),
        });
        while history.len() > MAX_SHELL_HISTORY_PER_PANE {
            history.pop_front();
        }

        if let Some(existing_idx) = self
            .global_shell_history
            .iter()
            .position(|entry| entry.command == trimmed)
        {
            self.global_shell_history.remove(existing_idx);
        }
        self.global_shell_history.push_back(CommandSuggestionEntry {
            command: trimmed.to_string(),
            cwd,
        });
        while self.global_shell_history.len() > MAX_GLOBAL_SHELL_HISTORY {
            self.global_shell_history.pop_front();
        }
    }

    /// Public-ish hook so the workspace can re-ask the AI summarizer
    /// after recording a new shell command. Separate from the
    /// `&mut self` mutation above so the borrow checker is happy.
    pub(super) fn after_shell_command_recorded(&self, cx: &App) {
        self.request_tab_summaries(cx);
    }

    pub(super) fn record_input_history(&mut self, input: &str) {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return;
        }

        if let Some(existing_idx) = self
            .global_input_history
            .iter()
            .position(|entry| entry == trimmed)
        {
            self.global_input_history.remove(existing_idx);
        }
        self.global_input_history.push_back(trimmed.to_string());
        while self.global_input_history.len() > MAX_GLOBAL_INPUT_HISTORY {
            self.global_input_history.pop_front();
        }
    }

    pub(super) fn recent_shell_commands(&self, limit: usize) -> Vec<String> {
        self.global_shell_history
            .iter()
            .rev()
            .take(limit)
            .map(|entry| entry.command.clone())
            .collect()
    }

    pub(super) fn recent_input_history(&self, limit: usize) -> Vec<String> {
        self.global_input_history
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub(super) fn history_completion_for_prefix(
        &self,
        prefix: &str,
        cwd: Option<&str>,
        mode: InputMode,
        is_remote: bool,
    ) -> Option<String> {
        let mut fallback: Option<String> = None;

        for entry in self.global_shell_history.iter().rev() {
            if entry.command == prefix || !entry.command.starts_with(prefix) {
                continue;
            }

            if cwd.is_some() && entry.cwd.as_deref() == cwd {
                return Some(entry.command.clone());
            }

            if fallback.is_none() {
                fallback = Some(entry.command.clone());
            }
        }

        if fallback.is_some() {
            return fallback;
        }

        self.global_input_history
            .iter()
            .rev()
            .find(|entry| {
                entry.as_str() != prefix
                    && entry.starts_with(prefix)
                    && (mode == InputMode::Shell
                        || matches!(
                            self.harness.classify_input(entry, is_remote),
                            InputKind::ShellCommand(_)
                        ))
            })
            .cloned()
    }

    pub(super) fn local_path_completion_for_prefix(
        &self,
        tab_idx: usize,
        pane_id: usize,
        input: &str,
        cx: &App,
    ) -> Option<LocalPathCompletion> {
        let pane_tree = &self.tabs.get(tab_idx)?.pane_tree;
        let terminal = pane_tree
            .pane_terminals()
            .into_iter()
            .find_map(|(id, terminal)| (id == pane_id).then_some(terminal))?;

        if self
            .effective_remote_host_for_tab(tab_idx, &terminal, cx)
            .is_some()
        {
            return None;
        }

        let cwd = terminal.current_dir(cx)?;
        let token_start = input
            .char_indices()
            .rev()
            .find_map(|(idx, ch)| ch.is_whitespace().then_some(idx + ch.len_utf8()))
            .unwrap_or(0);
        let token = &input[token_start..];
        if token.is_empty() {
            return None;
        }

        let head = input[..token_start].trim_end();
        let first_word = head.split_whitespace().next().unwrap_or_default();
        let completes_path = first_word == "cd"
            || token.starts_with('~')
            || token.starts_with('.')
            || token.contains('/');
        if !completes_path {
            return None;
        }

        let directories_only = first_word == "cd";
        let home_dir = dirs::home_dir();
        let (search_dir, dir_prefix, search_prefix) = if let Some(stripped) =
            token.strip_prefix("~/")
        {
            let home = home_dir?;
            match stripped.rsplit_once('/') {
                Some((dir, prefix)) => (home.join(dir), format!("~/{dir}/"), prefix.to_string()),
                None => (home, "~/".to_string(), stripped.to_string()),
            }
        } else if token == "~" {
            let home = home_dir?;
            (home, String::new(), "~".to_string())
        } else if let Some((dir, prefix)) = token.rsplit_once('/') {
            let base = if dir.is_empty() {
                PathBuf::from("/")
            } else if Path::new(dir).is_absolute() {
                PathBuf::from(dir)
            } else {
                PathBuf::from(&cwd).join(dir)
            };
            (base, format!("{dir}/"), prefix.to_string())
        } else {
            (PathBuf::from(&cwd), String::new(), token.to_string())
        };

        let mut matches = std::fs::read_dir(&search_dir)
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let file_type = entry.file_type().ok()?;
                if directories_only && !file_type.is_dir() {
                    return None;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                name.starts_with(&search_prefix)
                    .then_some((name, file_type.is_dir()))
            })
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return None;
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));

        let matched_name = if matches.len() == 1 {
            let (name, is_dir) = &matches[0];
            let mut single = name.clone();
            if *is_dir {
                single.push('/');
            }
            single
        } else {
            let prefix = longest_common_prefix(matches.iter().map(|(name, _)| name.as_str()));
            if prefix.chars().count() <= search_prefix.chars().count() {
                let candidates = matches
                    .into_iter()
                    .map(|(name, is_dir)| {
                        let mut candidate = if token == "~" {
                            name
                        } else {
                            format!("{dir_prefix}{name}")
                        };
                        if is_dir {
                            candidate.push('/');
                        }
                        format!("{}{}", &input[..token_start], candidate)
                    })
                    .collect::<Vec<_>>();
                return Some(LocalPathCompletion::Candidates(candidates));
            }
            prefix
        };

        let completed_token = if token == "~" {
            matched_name
        } else {
            format!("{dir_prefix}{matched_name}")
        };

        Some(LocalPathCompletion::Inline(format!(
            "{}{}",
            &input[..token_start],
            completed_token
        )))
    }

    pub(super) fn refresh_input_suggestion(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.has_active_tab() {
            log::debug!(target: "con::suggestions", "skip suggestion: no active tab");
            self.shell_suggestion_engine.cancel();
            self.input_bar
                .update(cx, |bar, _cx| bar.clear_completion_ui());
            return;
        }

        let (text, mode, target_ids) = self.input_bar.update(cx, |bar, cx| {
            (bar.current_text(cx), bar.mode(), bar.target_pane_ids())
        });

        let trimmed = text.trim();
        if trimmed.is_empty()
            || text.contains('\n')
            || trimmed.starts_with('/')
            || target_ids.len() != 1
        {
            log::debug!(
                target: "con::suggestions",
                "skip suggestion: empty={} multiline={} slash={} targets={}",
                trimmed.is_empty(),
                text.contains('\n'),
                trimmed.starts_with('/'),
                target_ids.len()
            );
            self.shell_suggestion_engine.cancel();
            self.input_bar
                .update(cx, |bar, _cx| bar.clear_completion_ui());
            return;
        }

        let pane_id = target_ids[0];
        let pane_tree = &self.tabs[self.active_tab].pane_tree;
        let pane = pane_tree
            .pane_terminals()
            .into_iter()
            .find_map(|(id, terminal)| (id == pane_id).then_some(terminal));
        let cwd = pane.as_ref().and_then(|pane| pane.current_dir(cx));
        let is_remote = pane.as_ref().is_some_and(|terminal| {
            self.effective_remote_host_for_tab(self.active_tab, terminal, cx)
                .is_some()
        });

        if let Some(path_match) =
            self.local_path_completion_for_prefix(self.active_tab, pane_id, &text, cx)
        {
            self.shell_suggestion_engine.cancel();
            match path_match {
                LocalPathCompletion::Inline(path_match) => {
                    log::debug!(
                        target: "con::suggestions",
                        "use path suggestion prefix={:?} completion={:?}",
                        text,
                        path_match
                    );
                    self.input_bar.update(cx, |bar, _cx| {
                        bar.set_path_inline_suggestion(&text, &path_match);
                    });
                }
                LocalPathCompletion::Candidates(candidates) => {
                    log::debug!(
                        target: "con::suggestions",
                        "use path candidates prefix={:?} count={}",
                        text,
                        candidates.len()
                    );
                    self.input_bar.update(cx, |bar, _cx| {
                        bar.set_path_completion_candidates(&text, candidates);
                    });
                }
            }
            return;
        }

        if let Some(history_match) =
            self.history_completion_for_prefix(&text, cwd.as_deref(), mode, is_remote)
        {
            log::debug!(
                target: "con::suggestions",
                "use history suggestion prefix={:?} completion={:?}",
                text,
                history_match
            );
            self.shell_suggestion_engine.cancel();
            self.input_bar.update(cx, |bar, _cx| {
                bar.set_history_inline_suggestion(&text, &history_match);
            });
            return;
        }

        self.input_bar
            .update(cx, |bar, _cx| bar.clear_completion_ui());

        let shell_probe_too_short = mode == InputMode::Smart && trimmed.chars().count() < 2;
        let is_shell_mode = match mode {
            InputMode::Shell => true,
            InputMode::Smart => {
                if shell_probe_too_short {
                    log::debug!(
                        target: "con::suggestions",
                        "skip ai suggestion: smart-mode probe too short prefix={:?}",
                        text
                    );
                    false
                } else {
                    matches!(
                        self.harness.classify_input(&text, is_remote),
                        InputKind::ShellCommand(_)
                    )
                }
            }
            InputMode::Agent => false,
        };

        if !is_shell_mode {
            log::debug!(
                target: "con::suggestions",
                "skip ai suggestion: input classified as non-shell prefix={:?}",
                text
            );
            self.shell_suggestion_engine.cancel();
            self.input_bar
                .update(cx, |bar, _cx| bar.clear_path_completion_candidates());
            return;
        }

        if !self.harness.config().suggestion_model.enabled {
            log::debug!(
                target: "con::suggestions",
                "skip ai suggestion: disabled in config prefix={:?}",
                text
            );
            self.shell_suggestion_engine.cancel();
            self.input_bar
                .update(cx, |bar, _cx| bar.clear_path_completion_candidates());
            return;
        }

        let suggestion_tx = self.shell_suggestion_tx.clone();
        let prefix = text.clone();
        let callback_prefix = prefix.clone();
        let tab_idx = self.active_tab;
        let recent_commands = self.recent_shell_commands(6);
        self.shell_suggestion_engine.request(
            &prefix,
            SuggestionContext {
                cwd,
                recent_commands,
            },
            move |completion| {
                let _ = suggestion_tx.send(ShellSuggestionResult {
                    tab_idx,
                    pane_id,
                    prefix: callback_prefix.clone(),
                    completion,
                });
            },
        );
    }

    pub(super) fn apply_shell_suggestion(
        &mut self,
        result: ShellSuggestionResult,
        cx: &mut Context<Self>,
    ) {
        if result.tab_idx != self.active_tab {
            log::debug!(
                target: "con::suggestions",
                "drop ai suggestion for inactive tab={} active={}",
                result.tab_idx,
                self.active_tab
            );
            return;
        }

        let (text, mode, target_ids) = self.input_bar.update(cx, |bar, cx| {
            (bar.current_text(cx), bar.mode(), bar.target_pane_ids())
        });

        if matches!(mode, InputMode::Agent)
            || target_ids.as_slice() != [result.pane_id]
            || text.trim().starts_with('/')
            || text.contains('\n')
        {
            log::debug!(
                target: "con::suggestions",
                "drop ai suggestion prefix={:?}: text/mode/target changed",
                result.prefix
            );
            return;
        }

        let input_changed = text != result.prefix;

        let pane_tree = &self.tabs[self.active_tab].pane_tree;
        let cwd = pane_tree
            .pane_terminals()
            .into_iter()
            .find_map(|(id, terminal)| (id == result.pane_id).then(|| terminal.current_dir(cx)))
            .flatten();

        let is_remote = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| {
                tab.pane_tree
                    .pane_terminals()
                    .into_iter()
                    .find_map(|(id, terminal)| {
                        (id == result.pane_id).then(|| {
                            self.effective_remote_host_for_tab(self.active_tab, &terminal, cx)
                                .is_some()
                        })
                    })
            })
            .unwrap_or(false);

        if self
            .history_completion_for_prefix(&text, cwd.as_deref(), mode, is_remote)
            .is_some()
        {
            log::debug!(
                target: "con::suggestions",
                "drop ai suggestion prefix={:?}: history match became available",
                result.prefix
            );
            return;
        }

        if input_changed {
            let full_suggestion = if result.completion.starts_with(&result.prefix) {
                result.completion.clone()
            } else {
                format!("{}{}", result.prefix, result.completion)
            };

            if !full_suggestion.starts_with(&text) || full_suggestion == text {
                log::debug!(
                    target: "con::suggestions",
                    "drop ai suggestion prefix={:?}: text changed to incompatible prefix {:?}",
                    result.prefix,
                    text
                );
                return;
            }

            log::debug!(
                target: "con::suggestions",
                "apply ai suggestion prefix={:?} current={:?} completion={:?}",
                result.prefix,
                text,
                full_suggestion
            );
            self.input_bar.update(cx, |bar, _cx| {
                bar.set_ai_inline_suggestion(&text, &full_suggestion);
            });
            cx.notify();
            return;
        }

        log::debug!(
            target: "con::suggestions",
            "apply ai suggestion prefix={:?} completion={:?}",
            result.prefix,
            result.completion
        );
        self.input_bar.update(cx, |bar, _cx| {
            bar.set_ai_inline_suggestion(&result.prefix, &result.completion);
        });
        cx.notify();
    }

    /// Result delivered by [`TabSummaryEngine`] — locate the tab by
    /// `summary_id` (NOT index, since reorders / closes shift the
    /// indexes), update its `ai_label` / `ai_icon`, and republish to
    /// the sidebar.
    pub(super) fn apply_tab_summary(
        &mut self,
        generation: u64,
        summary_epoch: u64,
        summary: TabSummary,
        cx: &mut Context<Self>,
    ) {
        if generation != self.tab_summary_generation
            || !self.harness.config().suggestion_model.enabled
        {
            return;
        }
        let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|t| t.summary_id == summary.tab_id)
        else {
            // Tab was closed while the request was in flight.
            return;
        };
        if !tab_summary_epoch_matches(summary_epoch, tab.summary_epoch) {
            log::debug!(
                target: "con::tab_summary",
                "tab_summary drop stale tab_id={} request_epoch={} current_epoch={}",
                summary.tab_id,
                summary_epoch,
                tab.summary_epoch,
            );
            return;
        }
        let label = summary.label.trim().to_string();
        let icon = summary.icon;
        let label_changed = tab.ai_label.as_deref() != Some(label.as_str());
        let icon_changed = tab.ai_icon != Some(icon);
        if !label_changed && !icon_changed {
            return;
        }
        tab.ai_label = Some(label);
        tab.ai_icon = Some(icon);
        log::debug!(
            target: "con::tab_summary",
            "tab_summary applied tab_id={} label={:?} icon={:?}",
            summary.tab_id,
            tab.ai_label,
            tab.ai_icon,
        );
        self.sync_sidebar(cx);
        cx.notify();
    }

    #[cfg(test)]
    pub(super) fn new_tab_sync_policy_for_tests() -> NewTabSyncPolicy {
        NewTabSyncPolicy {
            activates_new_tab: true,
            syncs_sidebar: true,
            notifies_ui: true,
            syncs_native_visibility: true,
            reuses_shared_tab_activation_flow: true,
        }
    }

    pub(super) fn should_defer_top_chrome_refresh_when_tab_strip_appears() -> bool {
        true
    }

    #[cfg(test)]
    pub(super) fn should_defer_top_chrome_refresh_when_tab_strip_appears_for_tests() -> bool {
        Self::should_defer_top_chrome_refresh_when_tab_strip_appears()
    }

    pub(super) fn request_tab_summaries(&self, cx: &App) {
        log::trace!(target: "con::activity", "summary_poll");
        if !self.harness.config().suggestion_model.enabled {
            return;
        }
        let tx = self.tab_summary_tx.clone();
        let generation = self.tab_summary_generation;
        for (i, tab) in self.tabs.iter().enumerate() {
            let summary_epoch = tab.summary_epoch;
            let Some(terminal) = tab.pane_tree.try_focused_terminal().cloned() else {
                continue;
            };
            // The terminal's recent scrollback is the only signal we
            // get for commands the user typed *directly* into the
            // pane (Con doesn't intercept those into shell_history).
            // Pull a small tail and pass it to the model — same lines
            // the user can see right now.
            let recent_output = terminal.recent_lines(24, cx);
            let req = TabSummaryRequest {
                tab_id: tab.summary_id,
                cwd: terminal.current_dir(cx),
                title: terminal.title_name(cx),
                ssh_host: self.effective_remote_host_for_tab(i, &terminal, cx),
                recent_commands: {
                    let mut histories: Vec<_> = tab.shell_history.iter().collect();
                    histories.sort_by_key(|(pane_id, _)| *pane_id);
                    histories
                        .into_iter()
                        .flat_map(|(_, q)| q.iter().rev())
                        .map(|entry| entry.command.clone())
                        .take(8)
                        .collect()
                },
                recent_output,
                agent_cli: tab.agent_cli,
            };
            let tx = tx.clone();
            self.tab_summary_engine.request(req, move |summary| {
                let _ = tx.send((generation, summary_epoch, summary));
            });
        }
    }
}

fn tab_summary_epoch_matches(request_epoch: u64, current_epoch: u64) -> bool {
    request_epoch == current_epoch
}

#[cfg(test)]
mod tests {
    use super::tab_summary_epoch_matches;

    #[test]
    fn tab_summary_epoch_rejects_late_response_after_context_change() {
        assert!(tab_summary_epoch_matches(7, 7));
        assert!(!tab_summary_epoch_matches(7, 8));
    }
}
