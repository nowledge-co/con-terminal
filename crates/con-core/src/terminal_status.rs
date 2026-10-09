//! Ephemeral terminal presentation. These observations must never authorize
//! tools, satisfy control-plane waits, or replace the harness runtime tracker.

use std::time::{Duration, Instant};

use crate::program_status::{self, Attention, Contribution, State, SurfaceProgramStatus};
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

/// Presentation severity. Later variants outrank earlier ones: unknown, idle,
/// an unseen completion, then in-flight and attention states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    #[default]
    Unknown,
    Idle,
    /// Program status reported `done` and this surface has not acknowledged it.
    Done,
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
    /// OSC 7501. Explicit reports outrank heuristics for the scope they describe.
    ProgramStatus,
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
    /// clear independent title observations. An accepted OSC 7501 report
    /// suppresses this heuristic until a full reset.
    progress: Option<Progress>,
    /// Dropped with this incarnation. Pane and tab moves keep the entity, so
    /// the records move with the terminal; close and replacement do not.
    program: SurfaceProgramStatus,
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
            program: SurfaceProgramStatus::new(),
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

    pub fn program_records(&self) -> Vec<program_status::ProgramRecord> {
        self.program.records()
    }

    /// Text for the record that owns this surface's program-status indicator.
    pub fn program_detail(&self) -> Option<String> {
        self.program.detail_line()
    }

    /// Returns whether the aggregated activity snapshot changed.
    /// A stored message can change without that snapshot changing.
    pub fn observe_program_status(
        &mut self,
        report: program_status::Incoming<'_>,
        now: Instant,
    ) -> Result<bool, program_status::Reject> {
        self.with_program(now, |program| program.apply(report))
    }

    pub fn clear_program_status(
        &mut self,
        id: &str,
        now: Instant,
    ) -> Result<bool, program_status::Reject> {
        self.with_program(now, |program| program.clear(id))
    }

    pub fn program_status_prompt(&mut self, now: Instant) -> bool {
        self.with_program_lifecycle(now, SurfaceProgramStatus::on_shell_prompt)
    }

    pub fn program_status_process_exit(&mut self, now: Instant) -> bool {
        self.with_program_lifecycle(now, SurfaceProgramStatus::on_process_exit)
    }

    pub fn program_status_full_reset(&mut self, now: Instant) -> bool {
        self.with_program_lifecycle(now, SurfaceProgramStatus::on_full_reset)
    }

    pub fn program_status_soft_reset(&mut self, now: Instant) -> bool {
        self.with_program_lifecycle(now, SurfaceProgramStatus::on_soft_reset)
    }

    pub fn program_status_alternate_screen(&mut self, active: bool, now: Instant) -> bool {
        self.with_program_lifecycle(now, |program| program.on_alternate_screen(active))
    }

    /// A key delivered to this surface. Window focus is not acknowledgement.
    pub fn acknowledge_program_status(&mut self, now: Instant) -> bool {
        self.with_program_lifecycle(now, SurfaceProgramStatus::acknowledge)
    }

    pub fn status(&self, now: Instant) -> Option<Status> {
        let contribution = self.program.contribution();
        let heuristic =
            self.heuristic_status(now, contribution.protocol_seen, contribution.root_described);
        prefer_program(self.program_status(contribution), heuristic)
    }

    fn with_program(
        &mut self,
        now: Instant,
        change: impl FnOnce(&mut SurfaceProgramStatus) -> Result<bool, program_status::Reject>,
    ) -> Result<bool, program_status::Reject> {
        let before = self.status(now);
        change(&mut self.program)?;
        Ok(self.status(now) != before)
    }

    fn with_program_lifecycle(
        &mut self,
        now: Instant,
        change: impl FnOnce(&mut SurfaceProgramStatus) -> bool,
    ) -> bool {
        let before = self.status(now);
        change(&mut self.program);
        self.status(now) != before
    }

    fn program_status(&self, contribution: Contribution) -> Option<Status> {
        let attention = contribution.attention.or_else(|| {
            contribution.root_described.then_some(Attention {
                state: State::Idle,
                progress: None,
            })
        })?;
        let (activity, percent) = match attention.state {
            State::Idle => (Activity::Idle, None),
            State::Working => (Activity::Busy, attention.progress),
            State::Done => (Activity::Done, None),
            State::Blocked => (Activity::NeedsInput, attention.progress),
            State::Error => (Activity::Error, None),
        };
        Some(Status {
            surface_id: self.surface_id,
            activity,
            evidence: Evidence::ProgramStatus,
            percent,
        })
    }

    fn heuristic_status(
        &self,
        now: Instant,
        suppress_progress: bool,
        root_described: bool,
    ) -> Option<Status> {
        let reported = self.reported.filter(|report| {
            report.evidence != Evidence::TitleMotion
                || now.saturating_duration_since(report.observed_at) < MOTION_LEASE
        });
        let title = (!root_described)
            .then_some(reported)
            .flatten()
            .map(|report| Status {
                surface_id: self.surface_id,
                activity: report.activity,
                evidence: report.evidence,
                percent: None,
            });
        let progress = (!suppress_progress)
            .then_some(self.progress)
            .flatten()
            .map(|progress| Status {
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

fn prefer_program(program: Option<Status>, heuristic: Option<Status>) -> Option<Status> {
    match (program, heuristic) {
        (Some(program), Some(heuristic)) if heuristic.activity > program.activity => {
            Some(heuristic)
        }
        (Some(program), _) => Some(program),
        (None, heuristic) => heuristic,
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
    use crate::program_status;

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

    fn program<'a>(
        state: program_status::State,
        id: &'a str,
        progress: Option<u8>,
        message: Option<&'a str>,
    ) -> program_status::Incoming<'a> {
        program_status::Incoming {
            state,
            id,
            kind: None,
            progress,
            app: None,
            title: None,
            message,
        }
    }

    #[test]
    fn explicit_program_status_outranks_heuristics_without_hiding_a_blocked_child() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        surface.observe_title(Some("⠋ task"), now);
        surface.observe_title(Some("⠙ task"), now);
        surface.observe_progress(Some(Progress {
            activity: Activity::Error,
            percent: Some(9),
        }));
        assert_eq!(surface.status(now).unwrap().evidence, Evidence::Progress);
        assert!(
            surface
                .observe_program_status(program(program_status::State::Idle, "", None, None), now)
                .unwrap()
        );
        let idle = surface.status(now).unwrap();
        assert_eq!(idle.activity, Activity::Idle);
        assert_eq!(idle.evidence, Evidence::ProgramStatus);
        assert_eq!(idle.percent, None);
        assert!(
            surface
                .observe_program_status(
                    program(program_status::State::Blocked, "worker", Some(12), None),
                    now,
                )
                .unwrap()
        );
        let blocked = surface.status(now).unwrap();
        assert_eq!(blocked.activity, Activity::NeedsInput);
        assert_eq!(blocked.percent, Some(12));
        assert!(
            !surface
                .observe_program_status(
                    program(
                        program_status::State::Blocked,
                        "worker",
                        Some(12),
                        Some("still")
                    ),
                    now,
                )
                .unwrap()
        );
        assert_eq!(surface.program_records().len(), 2);
    }

    #[test]
    fn a_child_report_leaves_root_heuristics_until_the_root_is_described() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        surface.observe_title(Some("[ ! ] Action Required | task"), now);
        surface
            .observe_program_status(
                program(program_status::State::Working, "job", Some(40), None),
                now,
            )
            .unwrap();
        assert_eq!(surface.status(now).unwrap().activity, Activity::NeedsInput);
        assert_eq!(surface.status(now).unwrap().evidence, Evidence::AgentTitle);
        surface
            .observe_program_status(program(program_status::State::Idle, "", None, None), now)
            .unwrap();
        let described = surface.status(now).unwrap();
        assert_eq!(described.activity, Activity::Busy);
        assert_eq!(described.evidence, Evidence::ProgramStatus);
        assert_eq!(described.percent, Some(40));
    }

    #[test]
    fn an_explicit_record_wins_a_tie_with_title_motion() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        surface.observe_title(Some("⠋ task"), now);
        surface.observe_title(Some("⠙ task"), now);
        assert_eq!(surface.status(now).unwrap().evidence, Evidence::TitleMotion);
        surface
            .observe_program_status(
                program(program_status::State::Working, "job", Some(40), None),
                now,
            )
            .unwrap();
        let status = surface.status(now).unwrap();
        assert_eq!(status.activity, Activity::Busy);
        assert_eq!(status.evidence, Evidence::ProgramStatus);
        assert_eq!(status.percent, Some(40));
    }

    #[test]
    fn full_reset_restores_progress_and_acknowledgement_stays_on_one_surface() {
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(7);
        let mut other = SurfaceStatus::new(8);
        surface.observe_progress(Some(Progress {
            activity: Activity::Busy,
            percent: Some(50),
        }));
        assert!(surface.clear_program_status("", now).unwrap());
        assert_eq!(surface.status(now), None);
        surface
            .observe_program_status(
                program(program_status::State::Done, "", None, Some("ok")),
                now,
            )
            .unwrap();
        assert_eq!(surface.status(now).unwrap().activity, Activity::Done);
        assert!(surface.acknowledge_program_status(now));
        assert_eq!(surface.status(now).unwrap().activity, Activity::Idle);
        assert!(!surface.program_records()[0].unseen);
        other
            .observe_program_status(
                program(program_status::State::Error, "", None, Some("no")),
                now,
            )
            .unwrap();
        assert!(!other.program_status_soft_reset(now));
        assert!(!other.program_status_alternate_screen(true, now));
        assert_eq!(other.status(now).unwrap().activity, Activity::Error);
        assert!(surface.program_status_full_reset(now));
        assert_eq!(surface.status(now).unwrap().percent, Some(50));
        assert_eq!(surface.status(now).unwrap().evidence, Evidence::Progress);
        assert!(surface.program_records().is_empty());
        assert_eq!(other.program_records().len(), 1);
        other.observe_identity(
            8,
            1,
            Some(AgentIdentity {
                agent: "claude",
                scope: IdentityScope::ForegroundJob,
                generation: 1,
            }),
        );
        assert_eq!(other.program_records().len(), 1);
        drop(surface);
        assert_eq!(other.status(now).unwrap().activity, Activity::Error);
        let replaced = SurfaceStatus::new(8);
        assert!(replaced.program_records().is_empty());
    }
}
