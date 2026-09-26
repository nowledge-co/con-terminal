//! Ephemeral terminal presentation. These observations must never authorize
//! tools, satisfy control-plane waits, or replace the harness runtime tracker.

use std::time::{Duration, Instant};

use crate::terminal_title::{TitleIndicator, title_indicator, without_indicator};

const MOTION_LEASE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityScope {
    ForegroundJob,
    ProcessTree,
    Title,
    Screen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentity {
    pub agent: &'static str,
    pub scope: IdentityScope,
    /// Changes on PID reuse and exec, even if the agent name stays the same.
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    #[default]
    Unknown,
    Idle,
    Busy,
    Paused,
    NeedsInput,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    TitleMotion,
    AgentTitle,
    Progress,
    BuiltinAgent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub activity: Activity,
    pub evidence: Evidence,
    pub observed_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub activity: Activity,
    pub percent: Option<u8>,
}

/// A snapshot retains the winning source; percentages are never combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub surface_id: u64,
    pub activity: Activity,
    pub evidence: Evidence,
    pub percent: Option<u8>,
}

/// One live surface incarnation. The owner removes it when that surface dies.
#[derive(Debug)]
pub struct SurfaceStatus {
    surface_id: u64,
    identity_sequence: Option<u64>,
    identity: Option<AgentIdentity>,
    last_title: Option<String>,
    reported: Option<Report>,
    /// OSC 9;4 timeout belongs to the terminal backend. Clearing it must not
    /// clear independent title observations.
    progress: Option<Progress>,
}

impl SurfaceStatus {
    pub fn new(surface_id: u64) -> Self {
        Self {
            surface_id,
            identity_sequence: None,
            identity: None,
            last_title: None,
            reported: None,
            progress: None,
        }
    }

    pub fn identity(&self) -> Option<&AgentIdentity> {
        self.identity.as_ref()
    }

    pub fn report(&self) -> Option<Report> {
        self.reported
    }

    /// Reject work for closed/replaced surfaces and out-of-order completions.
    pub fn observe_identity(
        &mut self,
        surface_id: u64,
        sequence: u64,
        identity: Option<AgentIdentity>,
    ) -> bool {
        if surface_id != self.surface_id
            || self
                .identity_sequence
                .is_some_and(|previous| sequence <= previous)
        {
            return false;
        }
        self.identity_sequence = Some(sequence);
        if self.identity == identity {
            return false;
        }
        self.identity = identity;
        // Only direct-agent busy/idle reports depend on executable identity.
        // Attention and observed title motion are independent terminal facts;
        // preserving them must not renew their original observation time.
        self.reported = self.reported.filter(|report| {
            report.evidence == Evidence::TitleMotion || report.activity == Activity::NeedsInput
        });
        // Keep last_title: an old title must not be replayed as a new report
        // from a different executable that inherited the same terminal.
        true
    }

    pub fn observe_title(&mut self, title: Option<&str>, now: Instant) {
        if self.last_title.as_deref() == title {
            return;
        }
        let candidate = title.and_then(title_indicator);
        let direct_claude = self.identity.as_ref().is_some_and(|identity| {
            identity.agent == "claude" && identity.scope == IdentityScope::ForegroundJob
        });
        let report = match candidate {
            Some((_, TitleIndicator::Attention(_))) => {
                Some((Activity::NeedsInput, Evidence::AgentTitle))
            }
            Some((_, TitleIndicator::Activity('◐' | '◑'))) if direct_claude => {
                Some((Activity::Busy, Evidence::AgentTitle))
            }
            Some((_, TitleIndicator::Activity('✳')))
                if direct_claude
                    && self.reported.is_some_and(|report| {
                        report.evidence == Evidence::AgentTitle && report.activity == Activity::Busy
                    }) =>
            {
                // Claude uses a static ✳ in multiplexers even while working.
                // Only a transition from the direct busy protocol reports idle.
                Some((Activity::Idle, Evidence::AgentTitle))
            }
            Some((range, TitleIndicator::Activity(frame))) => {
                let moving = self.last_title.as_deref().is_some_and(|previous| {
                    title_indicator(previous).is_some_and(|(old_range, old)| {
                        matches!(old, TitleIndicator::Activity(old_frame) if old_frame != frame)
                            && without_indicator(previous, old_range)
                                == without_indicator(title.unwrap(), range.clone())
                    })
                });
                moving.then_some((Activity::Busy, Evidence::TitleMotion))
            }
            None => None,
        };
        self.last_title = title.map(str::to_owned);
        self.reported = report.map(|(activity, evidence)| Report {
            activity,
            evidence,
            observed_at: now,
        });
    }

    pub fn observe_progress(&mut self, progress: Option<Progress>) {
        self.progress = progress;
    }

    pub fn status(&self, now: Instant) -> Option<Status> {
        let reported = self.reported.filter(|report| {
            report.evidence != Evidence::TitleMotion
                || now.saturating_duration_since(report.observed_at) < MOTION_LEASE
        });
        let title = reported.map(|report| Status {
            surface_id: self.surface_id,
            activity: report.activity,
            evidence: report.evidence,
            percent: None,
        });
        let progress = self.progress.map(|progress| Status {
            surface_id: self.surface_id,
            activity: progress.activity,
            evidence: Evidence::Progress,
            percent: progress.percent,
        });
        // Prefer actual progress to a title at equal activity. PTY writes do
        // not establish agent activity: a resident shell/TUI may never finish.
        [progress, title]
            .into_iter()
            .flatten()
            .fold(None, |best, next| match best {
                Some(previous) if previous.activity >= next.activity => Some(previous),
                _ => Some(next),
            })
    }
}

/// All surfaces participate. Priority, focused source, then stable tree order.
pub fn aggregate(
    statuses: impl IntoIterator<Item = Status>,
    focused_surface: Option<u64>,
) -> Option<Status> {
    statuses
        .into_iter()
        .fold(None, |best: Option<Status>, next| {
            let priority =
                |status: Status| (status.activity, Some(status.surface_id) == focused_surface);
            match best {
                Some(previous) if priority(previous) >= priority(next) => Some(previous),
                _ => Some(next),
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(surface: &mut SurfaceStatus, generation: u64, sequence: u64) {
        surface.observe_identity(
            7,
            sequence,
            Some(AgentIdentity {
                agent: "claude",
                scope: IdentityScope::ForegroundJob,
                generation,
            }),
        );
    }

    #[test]
    fn static_claude_marker_is_unknown_but_busy_to_idle_is_a_report() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        claude(&mut surface, 1, 1);
        surface.observe_title(Some("✳ task"), now);
        assert_eq!(surface.status(now), None);
        surface.observe_title(Some("◐ task"), now);
        assert_eq!(surface.status(now).unwrap().activity, Activity::Busy);
        surface.observe_title(Some("✳ task"), now);
        assert_eq!(surface.status(now).unwrap().activity, Activity::Idle);
    }

    #[test]
    fn generic_motion_expires_to_unknown_not_idle_at_exact_boundary() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        surface.observe_title(Some("⠋ task"), now);
        assert_eq!(surface.status(now), None);
        surface.observe_title(Some("⠙ task"), now);
        surface.observe_title(Some("⠙ task"), now + Duration::from_secs(2));
        assert_eq!(
            surface
                .status(now + MOTION_LEASE - Duration::from_nanos(1))
                .unwrap()
                .activity,
            Activity::Busy
        );
        assert_eq!(surface.status(now + MOTION_LEASE), None);
    }

    #[test]
    fn direct_claude_generic_spinner_does_not_clear_motion_at_asterisk() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        claude(&mut surface, 1, 1);
        surface.observe_title(Some("✢ task"), now);
        surface.observe_title(Some("✳ task"), now);
        assert_eq!(surface.status(now).unwrap().evidence, Evidence::TitleMotion);
        surface.observe_title(Some("✶ task"), now);
        assert_eq!(surface.status(now).unwrap().activity, Activity::Busy);
    }

    #[test]
    fn descendant_identity_does_not_enable_direct_title_protocol() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        surface.observe_identity(
            7,
            1,
            Some(AgentIdentity {
                agent: "claude",
                scope: IdentityScope::ProcessTree,
                generation: 1,
            }),
        );
        surface.observe_title(Some("◐ task"), now);
        assert_eq!(surface.status(now), None);
        surface.observe_title(Some("◑ task"), now);
        assert_eq!(surface.status(now).unwrap().evidence, Evidence::TitleMotion);
        assert_eq!(surface.status(now + MOTION_LEASE), None);
    }

    #[test]
    fn attention_beats_paused_without_losing_progress() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        surface.observe_progress(Some(Progress {
            activity: Activity::Paused,
            percent: Some(23),
        }));
        surface.observe_title(Some("[ ! ] Action Required | task"), now);
        assert_eq!(surface.status(now).unwrap().activity, Activity::NeedsInput);
        assert_eq!(surface.status(now).unwrap().percent, None);
        surface.observe_title(Some("task"), now);
        assert_eq!(surface.status(now).unwrap().percent, Some(23));
        surface.observe_progress(None);
        assert_eq!(surface.status(now), None);
    }

    #[test]
    fn clearing_progress_preserves_independent_activity_and_its_source() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        claude(&mut surface, 1, 1);
        surface.observe_title(Some("◑ task"), now);
        surface.observe_progress(Some(Progress {
            activity: Activity::Error,
            percent: Some(17),
        }));
        assert_eq!(surface.status(now).unwrap().activity, Activity::Error);
        surface.observe_progress(None);
        let remaining = surface.status(now + Duration::from_secs(20)).unwrap();
        assert_eq!(remaining.activity, Activity::Busy);
        assert_eq!(remaining.evidence, Evidence::AgentTitle);
        assert_eq!(remaining.percent, None);
    }

    #[test]
    fn stale_work_and_exec_cannot_replay_old_agent_activity() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        claude(&mut surface, 1, 2);
        surface.observe_title(Some("◐ task"), now);
        assert!(!surface.observe_identity(8, 3, None));
        assert!(!surface.observe_identity(7, 1, None));
        assert_eq!(surface.status(now).unwrap().activity, Activity::Busy);
        claude(&mut surface, 2, 3);
        surface.observe_title(Some("◐ task"), now);
        assert_eq!(surface.status(now), None);
    }

    #[test]
    fn identity_changes_preserve_independent_attention_and_motion_leases() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        surface.observe_title(Some("[ ! ] Action Required | task"), now);
        for generation in 1..=2 {
            claude(&mut surface, generation, generation);
            surface.observe_title(Some("[ ! ] Action Required | task"), now);
            assert_eq!(surface.status(now).unwrap().activity, Activity::NeedsInput);
        }
        surface.observe_title(Some("task"), now);
        assert_eq!(surface.status(now), None);
        surface.observe_title(Some("⠋ task"), now);
        surface.observe_title(Some("⠙ task"), now);
        claude(&mut surface, 3, 3);
        assert_eq!(surface.status(now).unwrap().evidence, Evidence::TitleMotion);
        assert_eq!(surface.status(now + MOTION_LEASE), None);
    }

    #[test]
    fn aggregation_prioritizes_severity_then_focus_without_averaging() {
        let make = |surface_id, activity, percent| Status {
            surface_id,
            activity,
            percent,
            evidence: Evidence::Progress,
        };
        let busy = make(1, Activity::Busy, Some(90));
        let paused = make(2, Activity::Paused, Some(13));
        let error = make(3, Activity::Error, Some(7));
        assert_eq!(aggregate([busy, paused, error], Some(1)), Some(error));
        assert_eq!(aggregate([busy, paused], Some(1)), Some(paused));
        let other_busy = make(4, Activity::Busy, Some(21));
        assert_eq!(aggregate([busy, other_busy], None), Some(busy));
        assert_eq!(aggregate([busy, other_busy], Some(4)), Some(other_busy));
    }
}
