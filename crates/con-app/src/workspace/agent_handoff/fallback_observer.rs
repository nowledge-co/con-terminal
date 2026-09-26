//! Read-only A2 observer. Detection enables a human confirmation; it never sends.
use super::kimi_screen::{
    Readiness, confirm_kimi_submit, kimi_instruction_pending, wait_kimi_ready,
};
use super::*;
use con_core::handoff::{FallbackEvidence, FallbackKind, HandoffJob, HandoffService, HandoffState};
use std::time::{Duration, Instant};

pub(super) const OBSERVATION_LIMIT: Duration = Duration::from_secs(60);

fn observation_pending(
    elapsed: Duration,
    state: HandoffState,
    revision: u64,
    expected: u64,
) -> bool {
    elapsed < OBSERVATION_LIMIT && state == HandoffState::NeedsInteraction && revision == expected
}

pub(super) fn poll_delay(focused: bool) -> Duration {
    Duration::from_millis(if focused { 250 } else { 1000 })
}

pub(super) fn observe_fallback(
    before: &[String],
    screen: &[String],
    instruction: &str,
) -> FallbackEvidence {
    if confirm_kimi_submit(before, screen, instruction) {
        FallbackEvidence::Submitted
    } else if !screen
        .iter()
        .any(|line| line.contains("Error: LLM not set"))
        && kimi_instruction_pending(screen, instruction)
    {
        FallbackEvidence::PasteDetected
    } else {
        FallbackEvidence::Idle
    }
}

impl ConWorkspace {
    pub(super) fn observe_kimi_fallback(
        &self,
        job: HandoffJob,
        tab_id: u64,
        terminal: TerminalPane,
        before: Option<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let runtime = self.harness.runtime_handle();
        cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let instruction = con_core::handoff::instruction(&job.id);
            let mut before = before;
            let mut revision = job.revision;
            while started.elapsed() < OBSERVATION_LIMIT {
                let current = runtime
                    .spawn_blocking({
                        let id = job.id.clone();
                        move || HandoffService::new()?.get(&id)
                    })
                    .await;
                let Ok(Ok(current)) = current else { break };
                if !observation_pending(
                    started.elapsed(),
                    current.state,
                    current.revision,
                    revision,
                ) {
                    break;
                }
                if current
                    .target_pid
                    .zip(current.target_process_start)
                    .is_none()
                {
                    break;
                }
                let observed = this.update(cx, |workspace, cx| {
                    let index = workspace
                        .tabs
                        .iter()
                        .position(|tab| tab.summary_id == tab_id)?;
                    if !workspace.tabs[index]
                        .pane_tree
                        .all_surface_terminals()
                        .iter()
                        .any(|item| item.entity_id() == terminal.entity_id())
                    {
                        return None;
                    }
                    let live = terminal.is_alive(cx)
                        && current.target_pid.is_some_and(|pid| {
                            current.target_process_start.is_some()
                                && con_agent::handoff::process_start_secs(pid as i32)
                                    == current.target_process_start
                        });
                    // Existing tabs must still own the same foreground process.
                    let identity = current.existing_target_tab_id.is_none()
                        || terminal.foreground_process_group_id(cx)
                            == current.target_pid.map(u64::from);
                    Some((
                        live,
                        identity,
                        index == workspace.active_tab,
                        terminal.content_lines(200, cx),
                    ))
                });
                let Ok(Some((live, identity, focused, screen))) = observed else {
                    break;
                };
                if !identity {
                    break;
                }
                let baseline = before.get_or_insert_with(|| screen.clone());
                let evidence = if live {
                    observe_fallback(baseline, &screen, &instruction)
                } else {
                    FallbackEvidence::Idle
                };
                let kind = if !live {
                    FallbackKind::TargetDead
                } else if wait_kimi_ready(&screen) == Readiness::TrustPending {
                    FallbackKind::Trust
                } else if evidence != FallbackEvidence::Idle
                    || wait_kimi_ready(&screen) == Readiness::Ready
                {
                    FallbackKind::PastePending
                } else {
                    current
                        .fallback
                        .as_ref()
                        .map_or(FallbackKind::NotReady, |g| g.kind)
                };
                if current
                    .fallback
                    .as_ref()
                    .is_none_or(|g| g.kind != kind || g.evidence != evidence)
                {
                    let result = runtime
                        .spawn_blocking({
                            let id = job.id.clone();
                            move || {
                                HandoffService::new()?
                                    .record_fallback_observation(&id, revision, kind, evidence)
                            }
                        })
                        .await;
                    let Ok(Ok(updated)) = result else { break };
                    revision = updated.revision;
                }
                if !live {
                    break;
                }
                cx.background_executor().timer(poll_delay(focused)).await;
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Duration, FallbackEvidence, HandoffState, OBSERVATION_LIMIT, observation_pending,
        observe_fallback, poll_delay,
    };
    fn screen(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }
    #[test]
    fn fallback_distinguishes_paste_submit_and_unrelated_screens() {
        let before = screen("Session: session_old\n>");
        assert_eq!(
            observe_fallback(&before, &before, "Read context"),
            FallbackEvidence::Idle
        );
        assert_eq!(
            observe_fallback(
                &before,
                &screen("Session: session_old\n> Read context"),
                "Read context"
            ),
            FallbackEvidence::PasteDetected
        );
        assert_eq!(
            observe_fallback(
                &before,
                &screen("Session: session_old\nRead context\n>"),
                "Read context"
            ),
            FallbackEvidence::Submitted
        );
        for after in [
            "Trust this folder\n> Read context",
            "Session: session_old\n> Read",
            "Session: session_new\nError: LLM not set\n>",
        ] {
            assert_eq!(
                observe_fallback(&before, &screen(after), "Read context"),
                FallbackEvidence::Idle
            );
        }
    }
    #[test]
    fn observer_is_bounded_and_background_tabs_poll_less() {
        assert!(observation_pending(
            Duration::from_secs(59),
            HandoffState::NeedsInteraction,
            2,
            2
        ));
        assert!(!observation_pending(
            OBSERVATION_LIMIT,
            HandoffState::NeedsInteraction,
            2,
            2
        ));
        assert!(!observation_pending(
            Duration::ZERO,
            HandoffState::NeedsInteraction,
            3,
            2
        ));
        for state in [
            HandoffState::Cancelled,
            HandoffState::Active,
            HandoffState::Failed,
        ] {
            assert!(!observation_pending(Duration::ZERO, state, 2, 2));
        }
        assert_eq!(poll_delay(true), Duration::from_millis(250));
        assert_eq!(poll_delay(false), Duration::from_secs(1));
    }
}
