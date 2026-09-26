use super::*;

/// Send eligibility is established only by live evidence about the current
/// foreground process (name/argv or an interpreter with a live banner).
/// The cached Tab brand can veto a
/// mismatch but never substitutes for live evidence: when the process cannot
/// be identified right now, the answer is `None` (refuse).
pub(super) fn running_source_agent(
    detected: Option<con_agent::handoff::AgentKind>,
    process: &str,
    pid: u64,
    screen: &[String],
) -> Option<con_agent::handoff::AgentKind> {
    running_source_agent_with_group(
        detected,
        process,
        pid,
        screen,
        con_agent::handoff::agent_from_process_group,
    )
}

fn running_source_agent_with_group(
    detected: Option<con_agent::handoff::AgentKind>,
    process: &str,
    pid: u64,
    screen: &[String],
    group_agent: impl FnOnce(u64) -> Option<con_agent::handoff::AgentKind>,
) -> Option<con_agent::handoff::AgentKind> {
    if process_name_is_shell(process) {
        return None;
    }
    let direct = process_agent(process)
        .or_else(|| group_agent(pid))
        .or_else(|| {
            // Only known interpreter hosts may use live screen evidence.
            // Shells/editors and a cached brand alone never qualify.
            matches!(process, "node" | "bun" | "python" | "python3")
                .then(|| super::super::tab_presentation::agent_from_screen_text(screen))
                .flatten()
                .and_then(handoff_agent)
        })?;
    match detected {
        Some(cached) if cached != direct => None,
        _ => Some(direct),
    }
}

impl ConWorkspace {
    /// Re-verify the source Tab, terminal, foreground process group, Agent
    /// identity and working directory against the binding taken when the
    /// panel opened. Reads live GPUI/process state only — cheap enough for
    /// the UI thread and run before and after the async service work.
    pub(super) fn check_source_binding(
        &self,
        binding: &SourceAgentTab,
        job: &con_core::handoff::HandoffJob,
        cx: &App,
    ) -> anyhow::Result<usize> {
        let source_index = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == binding.tab_id)
            .ok_or_else(|| anyhow::anyhow!("Source Tab was closed"))?;
        let source = self.tabs[source_index]
            .pane_tree
            .surface_terminals()
            .into_iter()
            .find(|(_, _, terminal)| terminal.entity_id() == binding.terminal_id)
            .map(|(_, _, terminal)| terminal)
            .ok_or_else(|| anyhow::anyhow!("Source terminal was replaced"))?;
        let group = source
            .foreground_process_group_id(cx)
            .ok_or_else(|| anyhow::anyhow!("Source process unavailable"))?;
        anyhow::ensure!(
            group == binding.foreground_group,
            "Source Agent changed; reopen Handoff"
        );
        let process = crate::process_name::process_name(group)
            .ok_or_else(|| anyhow::anyhow!("Source process unavailable"))?;
        anyhow::ensure!(
            running_source_agent(
                self.tabs[source_index].agent_cli.and_then(handoff_agent),
                &process,
                group,
                &source.content_lines(200, cx),
            ) == Some(binding.agent),
            "Source Agent stopped or changed; reopen Handoff"
        );
        anyhow::ensure!(
            job.request.source_agent == binding.agent,
            "Source Agent changed"
        );
        let cwd = job.request.cwd.canonicalize()?;
        anyhow::ensure!(
            source
                .current_dir(cx)
                .map(PathBuf::from)
                .and_then(|path| path.canonicalize().ok())
                == Some(cwd),
            "Source working directory changed"
        );
        Ok(source_index)
    }

    /// Re-verify the selected existing target Tab is still live with the same
    /// terminal, foreground process group and Agent identity.
    pub(super) fn check_existing_target(
        &self,
        source_index: usize,
        target: &ExistingAgentTab,
        job: &con_core::handoff::HandoffJob,
        cx: &App,
    ) -> anyhow::Result<usize> {
        let index = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == target.tab_id)
            .ok_or_else(|| anyhow::anyhow!("Target Tab was closed"))?;
        anyhow::ensure!(index != source_index, "Source and target Tab must differ");
        let cwd = job.request.cwd.canonicalize()?;
        let live = self
            .available_handoff_tabs(source_index, &cwd, cx)
            .into_iter()
            .find(|tab| {
                tab.tab_id == target.tab_id
                    && tab.terminal_id == target.terminal_id
                    && tab.foreground_group == target.foreground_group
                    && tab.agent == target.agent
            })
            .ok_or_else(|| anyhow::anyhow!("Target Agent changed or stopped; choose it again"))?;
        anyhow::ensure!(
            live.agent == job.target.agent,
            "Selected target Agent changed"
        );
        Ok(index)
    }
}

#[cfg(test)]
mod tests {
    use super::{running_source_agent, running_source_agent_with_group};

    #[test]
    fn kimi_screen_fallback_is_bounded_to_interpreter_hosts() {
        use con_agent::handoff::AgentKind::Kimi;
        let screen = vec!["Welcome to Kimi Code!".into()];
        assert_eq!(
            running_source_agent(None, "Kimi Code", 999_999_999, &[]),
            Some(Kimi)
        );
        assert_eq!(
            running_source_agent(None, "node", 999_999_999, &screen),
            Some(Kimi)
        );
        for process in ["zsh", "vim", "cat"] {
            assert_eq!(
                running_source_agent(Some(Kimi), process, 999_999_999, &screen),
                None
            );
        }
        assert_eq!(running_source_agent(None, "node", 999_999_999, &[]), None);
        assert_eq!(
            running_source_agent(
                Some(con_agent::handoff::AgentKind::Codex),
                "node",
                999_999_999,
                &screen
            ),
            None
        );
    }

    #[test]
    fn source_must_still_be_the_running_agent() {
        use con_agent::handoff::AgentKind;

        assert_eq!(
            running_source_agent(Some(AgentKind::Codex), "codex", 999_999_999, &[]),
            Some(AgentKind::Codex)
        );
        assert_eq!(
            running_source_agent(Some(AgentKind::Codex), "zsh", 999_999_999, &[]),
            None
        );
        assert_eq!(
            running_source_agent(Some(AgentKind::Codex), "kimi", 999_999_999, &[]),
            None
        );
        assert_eq!(
            running_source_agent(None, "codex", 999_999_999, &[]),
            Some(AgentKind::Codex)
        );
        // A cached brand must never substitute for live process evidence:
        // an unrecognized foreground process refuses the handoff.
        assert_eq!(
            running_source_agent(Some(AgentKind::Codex), "vim", 999_999_999, &[]),
            None
        );
    }

    #[test]
    fn process_group_evidence_preserves_source_gates() {
        use con_agent::handoff::AgentKind::{Codex, Kimi};
        let screen = vec!["Welcome to Kimi Code!".into()];
        for (process, group, cached, expected) in [
            ("con-cli", Some(Kimi), None, Some(Kimi)),
            ("con-cli", Some(Kimi), Some(Kimi), Some(Kimi)),
            ("con-cli", Some(Kimi), Some(Codex), None),
            ("con-cli", None, Some(Kimi), None),
            ("vim", None, Some(Kimi), None),
            ("cat", None, None, None),
            ("node", None, None, None),
        ] {
            // An unrelated node script without a banner still cannot qualify.
            let lines = if process == "node" {
                &[][..]
            } else {
                screen.as_slice()
            };
            assert_eq!(
                running_source_agent_with_group(cached, process, 42, lines, |pid| {
                    assert_eq!(pid, 42);
                    group
                }),
                expected,
                "{process}, group={group:?}, cached={cached:?}"
            );
        }
    }

    #[test]
    fn shell_rejection_and_direct_kimi_precede_group_inspection() {
        use con_agent::handoff::AgentKind::Kimi;
        let screen = vec!["Welcome to Kimi Code!".into()];
        for (process, expected) in [("zsh", None), ("bash", None), ("kimi", Some(Kimi))] {
            assert_eq!(
                running_source_agent_with_group(Some(Kimi), process, 42, &screen, |_| {
                    panic!("{process} must be decided before inspecting the group")
                }),
                expected
            );
        }
    }
}
