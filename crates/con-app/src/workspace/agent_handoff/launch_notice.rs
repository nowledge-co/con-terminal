//! Observe persisted launch results, never guess clipboard success from dispatch.
use super::*;
use con_core::handoff::HandoffState;

#[derive(Debug, PartialEq)]
enum LaunchNotice {
    ManualDelivery,
    NeedsInteraction,
    Failed,
}

impl LaunchNotice {
    // Successful clipboard delivery is explained by the persistent job card.
    // Only outcomes requiring attention may interrupt the target terminal.
    fn prompt(&self) -> Option<(PromptLevel, &'static str)> {
        match self {
            Self::ManualDelivery => None,
            Self::NeedsInteraction => Some((
                PromptLevel::Warning,
                "Target launch needs review. Paste with ⌘V in the target, then confirm in Handoff.",
            )),
            Self::Failed => Some((
                PromptLevel::Critical,
                "Target launch failed. Check Handoff details.",
            )),
        }
    }
}

fn launch_notice(state: HandoffState, manual_shown: bool) -> Option<LaunchNotice> {
    match state {
        HandoffState::AwaitingManualDelivery if !manual_shown => Some(LaunchNotice::ManualDelivery),
        HandoffState::NeedsInteraction => Some(LaunchNotice::NeedsInteraction),
        HandoffState::Failed => Some(LaunchNotice::Failed),
        _ => None,
    }
}

impl ConWorkspace {
    pub(super) fn observe_handoff_launch(
        &self,
        id: String,
        tab_id: u64,
        terminal: TerminalPane,
        cx: &mut Context<Self>,
    ) {
        let runtime = self.harness.runtime_handle();
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let mut manual_shown = false;
            let mut pending_kimi = None;
            // Bounded startup observation; durable errors remain in the job card
            // even if a later runtime failure happens after this window.
            for _ in 0..120 {
                let id = id.clone();
                let result = runtime
                    .spawn_blocking(move || con_core::handoff::HandoffService::new()?.get(&id))
                    .await;
                if let Ok(Ok(job)) = result {
                    if job.target.agent == con_agent::handoff::AgentKind::Kimi
                        && job.state == HandoffState::Delivering
                        && job.target_pid.is_some()
                    {
                        let _ = this.update(cx, |workspace, cx| {
                            workspace.deliver_kimi_via_pty(job, tab_id, terminal.clone(), cx);
                        });
                        return;
                    }

                    if job.target.agent == con_agent::handoff::AgentKind::Kimi
                        && job.state == HandoffState::Delivering
                    {
                        pending_kimi = Some(job.clone());
                    }
                    if let Some(notice) = launch_notice(job.state, manual_shown) {
                        if let Some((level, message)) = notice.prompt() {
                            let _ = handle.update(cx, |_, window, cx| {
                                this.update(cx, |_, cx| {
                                    std::mem::drop(window.prompt(
                                        level,
                                        "Agent Handoff",
                                        Some(message),
                                        &["OK"],
                                        cx,
                                    ));
                                })
                            });
                        }
                        manual_shown |= notice == LaunchNotice::ManualDelivery;
                    }
                    if job.state == HandoffState::NeedsInteraction
                        && job.target.agent == con_agent::handoff::AgentKind::Kimi
                    {
                        let _ = this.update(cx, |workspace, cx| {
                            workspace.observe_kimi_fallback(
                                job.clone(),
                                tab_id,
                                terminal.clone(),
                                None,
                                cx,
                            );
                        });
                    }
                    if matches!(
                        job.state,
                        HandoffState::NeedsInteraction
                            | HandoffState::Cancelled
                            | HandoffState::Failed
                    ) {
                        break;
                    }
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
            }
            // A helper that never persisted its child identity is uncertain,
            // not successful. The shared path backs up the instruction and
            // rejects the missing identity without writing to the terminal.
            if let Some(job) = pending_kimi {
                let _ = this.update(cx, |workspace, cx| {
                    workspace.deliver_kimi_via_pty(job, tab_id, terminal, cx);
                });
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{HandoffState, LaunchNotice, PromptLevel, launch_notice};
    #[test]
    fn clipboard_notice_waits_for_confirmed_manual_start_and_shows_once() {
        for state in [
            HandoffState::LaunchPending,
            HandoffState::StartingTarget,
            HandoffState::Delivering,
            HandoffState::Active,
        ] {
            assert!(launch_notice(state, false).is_none());
        }
        let notice = launch_notice(HandoffState::AwaitingManualDelivery, false).unwrap();
        assert_eq!(notice, LaunchNotice::ManualDelivery);
        assert!(
            notice.prompt().is_none(),
            "clipboard success must never open a modal"
        );
        assert!(launch_notice(HandoffState::AwaitingManualDelivery, true).is_none());
    }

    #[test]
    fn attention_states_still_prompt_after_manual_delivery() {
        for manual_shown in [false, true] {
            let review = launch_notice(HandoffState::NeedsInteraction, manual_shown).unwrap();
            assert!(matches!(review.prompt(), Some((PromptLevel::Warning, _))));
            assert!(
                review
                    .prompt()
                    .unwrap()
                    .1
                    .contains("Paste with ⌘V in the target, then confirm in Handoff")
            );
            let failed = launch_notice(HandoffState::Failed, manual_shown).unwrap();
            assert!(matches!(failed.prompt(), Some((PromptLevel::Critical, _))));
        }
    }

    #[test]
    fn cancellation_does_not_interrupt_the_user() {
        for manual_shown in [false, true] {
            assert!(launch_notice(HandoffState::Cancelled, manual_shown).is_none());
        }
    }
}
