use con_agent::handoff::AgentKind;
use con_core::handoff::HandoffState;

/// Target process presence when the panel opens; independent of new sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TargetPresence {
    /// Target identity is live, or delivery has not finished yet.
    /// May occupy the main card when it belongs to this Tab's session.
    Current,
    /// The process may still be the target, but this record cannot prove it.
    /// Requires explicit reconciliation; cannot identify a current target.
    Uncertain,
    /// The recorded process is gone, or an old record has no live target at all.
    Absent,
}

/// `observed_start` / `observed_agent` describe `recorded_pid` right now.
/// `None` start means that pid is not running. `live_target_agents` are the
/// Agents in other open terminals of this workspace; they never identify a
/// historical job by themselves.
pub(super) fn classify_target_presence(
    state: HandoffState,
    target_agent: AgentKind,
    recorded_pid: Option<u32>,
    recorded_start: Option<u64>,
    observed_start: Option<u64>,
    observed_agent: Option<AgentKind>,
    live_target_agents: &[AgentKind],
) -> TargetPresence {
    // An unavailable adapter cannot prove that an old target has stopped.
    // In particular, a legacy record without a pid must not auto-cancel.
    if target_agent == AgentKind::Unknown || state == HandoffState::NeedsInteraction {
        return TargetPresence::Uncertain;
    }
    if matches!(
        state,
        HandoffState::Prepared
            | HandoffState::LaunchPending
            | HandoffState::StartingTarget
            | HandoffState::Delivering
    ) {
        return TargetPresence::Current;
    }
    if state.is_terminal() {
        return TargetPresence::Uncertain;
    }
    match (recorded_pid, recorded_start) {
        (Some(_), Some(start)) => match (observed_start, observed_agent) {
            (Some(seen), Some(agent)) if seen == start && agent == target_agent => {
                TargetPresence::Current
            }
            (Some(seen), None) if seen == start => TargetPresence::Uncertain,
            _ => TargetPresence::Absent,
        },
        (Some(_), None) => {
            if observed_start.is_some() {
                TargetPresence::Uncertain
            } else {
                TargetPresence::Absent
            }
        }
        (None, _) => {
            if live_target_agents.contains(&target_agent) {
                TargetPresence::Uncertain
            } else {
                TargetPresence::Absent
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(pid: Option<u32>, start: Option<u64>, live: &[AgentKind]) -> TargetPresence {
        classify_target_presence(
            HandoffState::Active,
            AgentKind::Kimi,
            pid,
            start,
            None,
            None,
            live,
        )
    }

    #[test]
    fn unknown_target_stays_reviewable_without_automatic_cancellation() {
        for state in [
            HandoffState::Prepared,
            HandoffState::Active,
            HandoffState::NeedsInteraction,
        ] {
            assert_eq!(
                classify_target_presence(state, AgentKind::Unknown, None, None, None, None, &[]),
                TargetPresence::Uncertain,
            );
        }
    }

    #[test]
    fn failed_launch_stays_visible_after_target_exit() {
        assert_eq!(
            classify_target_presence(
                HandoffState::NeedsInteraction,
                AgentKind::Codex,
                Some(42),
                Some(100),
                None,
                None,
                &[]
            ),
            TargetPresence::Uncertain
        );
    }

    #[test]
    fn a_live_matching_process_stays_current() {
        assert_eq!(
            classify_target_presence(
                HandoffState::Active,
                AgentKind::Kimi,
                Some(42),
                Some(100),
                Some(100),
                Some(AgentKind::Kimi),
                &[],
            ),
            TargetPresence::Current
        );
    }

    #[test]
    fn a_dead_or_replaced_process_is_absent() {
        assert_eq!(
            active(Some(42), Some(100), &[]),
            TargetPresence::Absent,
            "pid is not running"
        );
        assert_eq!(
            classify_target_presence(
                HandoffState::Active,
                AgentKind::Kimi,
                Some(42),
                Some(100),
                Some(200),
                Some(AgentKind::Kimi),
                &[],
            ),
            TargetPresence::Absent,
            "pid was reused"
        );
        assert_eq!(
            classify_target_presence(
                HandoffState::Active,
                AgentKind::Kimi,
                Some(42),
                Some(100),
                Some(100),
                Some(AgentKind::Codex),
                &[],
            ),
            TargetPresence::Absent,
            "same process is no longer Kimi"
        );
    }

    #[test]
    fn an_unproven_live_process_requires_reconciliation() {
        assert_eq!(
            classify_target_presence(
                HandoffState::Active,
                AgentKind::Kimi,
                Some(42),
                Some(100),
                Some(100),
                None,
                &[],
            ),
            TargetPresence::Uncertain
        );
        assert_eq!(active(Some(42), None, &[]), TargetPresence::Absent);
        assert_eq!(
            classify_target_presence(
                HandoffState::Active,
                AgentKind::Kimi,
                Some(42),
                None,
                Some(100),
                Some(AgentKind::Kimi),
                &[],
            ),
            TargetPresence::Uncertain
        );
    }

    #[test]
    fn a_tab_id_without_a_process_is_not_a_current_target() {
        assert_eq!(
            active(None, None, &[]),
            TargetPresence::Absent,
            "no other terminal is open"
        );
        assert_eq!(
            active(None, None, &[AgentKind::Kimi]),
            TargetPresence::Uncertain,
            "an open Kimi tab does not prove this is the recorded target"
        );
    }

    #[test]
    fn unfinished_delivery_is_not_treated_as_a_missing_target() {
        assert_eq!(
            classify_target_presence(
                HandoffState::Delivering,
                AgentKind::Kimi,
                None,
                None,
                None,
                None,
                &[],
            ),
            TargetPresence::Current
        );
        assert_eq!(
            classify_target_presence(
                HandoffState::Prepared,
                AgentKind::Kimi,
                None,
                None,
                None,
                None,
                &[],
            ),
            TargetPresence::Current
        );
    }
}
