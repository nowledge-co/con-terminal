use super::*;

impl HandoffService {
    /// Existing-tab identity was checked by the live route but is not stored
    /// until delivery succeeds. Preserve it when entering C so A2 can observe
    /// the same process without altering the B-path receipt contract.
    pub fn record_kimi_fallback(
        &self,
        id: &str,
        revision: u64,
        message: &str,
        identity: Option<(u32, u64)>,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure!(
                job.target.agent == con_agent::handoff::AgentKind::Kimi
                    && job.state == HandoffState::Delivering,
                "Kimi delivery is not pending"
            );
            if let Some((pid, start)) = identity {
                ensure!(
                    pid > 0 && start > 0,
                    "Target process identity is incomplete"
                );
                ensure!(
                    job.target_pid.is_none_or(|saved| saved == pid)
                        && job.target_process_start.is_none_or(|saved| saved == start),
                    "Target identity changed"
                );
                job.target_pid = Some(pid);
                job.target_process_start = Some(start);
            }
            job.state = HandoffState::NeedsInteraction;
            job.set_fallback(
                FallbackKind::from_kimi_failure(message),
                message != "clipboard_failed",
            );
            Ok(())
        })
    }

    /// Compare-and-swap guidance only: no receipt, no PTY write, no Active transition.
    pub fn record_fallback_observation(
        &self,
        id: &str,
        revision: u64,
        kind: FallbackKind,
        evidence: FallbackEvidence,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure!(
                job.state == HandoffState::NeedsInteraction,
                "Fallback is not pending"
            );
            ensure!(
                job.target.agent == con_agent::handoff::AgentKind::Kimi
                    || evidence == FallbackEvidence::Idle,
                "Screen evidence requires Kimi"
            );
            let copied = job
                .fallback
                .as_ref()
                .is_some_and(|guide| guide.clipboard_copied);
            job.set_fallback(kind, copied);
            job.fallback.as_mut().unwrap().evidence = evidence;
            Ok(())
        })
    }
}
