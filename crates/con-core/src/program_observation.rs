//! Explicit program reports for external orchestrators. Not task receipts,
//! process identity, approval, or inferred UI activity.
use serde::{Deserialize, Serialize};

use crate::program_status::{BlockedKind, State, SurfaceProgramStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl std::str::FromStr for ReportedState {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "idle" => Ok(Self::Idle),
            "working" => Ok(Self::Working),
            "done" => Ok(Self::Done),
            "blocked" => Ok(Self::Blocked),
            "error" => Ok(Self::Error),
            _ => Err("Expected idle, working, done, blocked, or error".into()),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ReportedRecord {
    pub id: String,
    pub sequence: u64,
    pub state: ReportedState,
    pub kind: Option<&'static str>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProgramObservation {
    pub cursor: String,
    pub protocol_seen: bool,
    /// Null until this incarnation has received a valid report.
    pub evidence: Option<&'static str>,
    pub records: Vec<ReportedRecord>,
}

impl ProgramObservation {
    pub fn new(incarnation: uuid::Uuid, program: &SurfaceProgramStatus) -> Self {
        Self {
            cursor: format!("{}:{}", incarnation, program.sequence()),
            protocol_seen: program.protocol_seen(),
            evidence: program.protocol_seen().then_some("osc_7501"),
            records: program
                .records()
                .into_iter()
                .map(|record| ReportedRecord {
                    id: record.id.path(),
                    sequence: record.sequence,
                    state: match record.state {
                        State::Idle => ReportedState::Idle,
                        State::Working => ReportedState::Working,
                        State::Done => ReportedState::Done,
                        State::Blocked => ReportedState::Blocked,
                        State::Error => ReportedState::Error,
                    },
                    kind: record.kind.map(|kind| match kind {
                        BlockedKind::Permission => "permission",
                        BlockedKind::Question => "question",
                        BlockedKind::Auth => "auth",
                    }),
                    progress: record.progress,
                    app: record.app.map(|app| app.as_str().to_owned()),
                    title: record.title,
                    message: record.message,
                })
                .collect(),
        }
    }

    /// Only an exact record newer than the supplied baseline can match.
    /// Cursor mismatch/future baselines are errors, never silent retargeting.
    pub fn match_after(
        &self,
        after: &str,
        record_id: &str,
        states: &[ReportedState],
    ) -> Result<Option<&ReportedRecord>, String> {
        let parse = |cursor: &str| -> Result<(uuid::Uuid, u64), String> {
            let (epoch, sequence) = cursor.rsplit_once(':').ok_or("Invalid status cursor")?;
            Ok((
                epoch.parse().map_err(|_| "Invalid status cursor")?,
                sequence.parse().map_err(|_| "Invalid status cursor")?,
            ))
        };
        let (epoch, sequence) = parse(&self.cursor)?;
        let (previous_epoch, previous_sequence) = parse(after)?;
        if epoch != previous_epoch {
            return Err("Status cursor belongs to another terminal incarnation".into());
        }
        if previous_sequence > sequence {
            return Err("Status cursor is ahead of this terminal".into());
        }
        if crate::program_status::RecordId::parse(record_id).is_none() {
            return Err("Invalid program record id".into());
        }
        Ok(self.records.iter().find(|record| {
            record.id == record_id
                && record.sequence > previous_sequence
                && states.contains(&record.state)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program_status::Incoming;

    fn report(program: &mut SurfaceProgramStatus, id: &str, state: State) {
        program
            .apply(Incoming {
                state,
                id,
                kind: Some(BlockedKind::Permission),
                progress: None,
                app: Some("worker"),
                title: None,
                message: Some("Untrusted report"),
            })
            .unwrap();
    }

    #[test]
    fn freshness_ignores_acknowledgement_and_other_records_but_accepts_repeated_reports() {
        let epoch = uuid::Uuid::new_v4();
        let mut program = SurfaceProgramStatus::new();
        report(&mut program, "", State::Done);
        let baseline = ProgramObservation::new(epoch, &program).cursor;
        program.acknowledge();
        report(&mut program, "child", State::Blocked);
        let snapshot = ProgramObservation::new(epoch, &program);
        assert!(
            snapshot
                .match_after(&baseline, "", &[ReportedState::Done])
                .unwrap()
                .is_none()
        );
        assert_eq!(
            snapshot
                .match_after(&baseline, "child", &[ReportedState::Blocked])
                .unwrap()
                .unwrap()
                .kind,
            Some("permission")
        );
        report(&mut program, "", State::Done);
        assert!(
            ProgramObservation::new(epoch, &program)
                .match_after(&baseline, "", &[ReportedState::Done])
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn reset_never_rewinds_and_replacement_never_reuses_a_cursor() {
        let epoch = uuid::Uuid::new_v4();
        let mut program = SurfaceProgramStatus::new();
        report(&mut program, "", State::Done);
        let baseline = ProgramObservation::new(epoch, &program).cursor;
        program.on_full_reset();
        let snapshot = ProgramObservation::new(epoch, &program);
        assert!(!snapshot.protocol_seen);
        assert!(
            snapshot
                .match_after(&baseline, "", &[ReportedState::Done])
                .unwrap()
                .is_none()
        );
        report(&mut program, "", State::Done);
        let snapshot = ProgramObservation::new(epoch, &program);
        assert!(
            snapshot
                .match_after(&baseline, "", &[ReportedState::Done])
                .unwrap()
                .is_some()
        );
        assert!(
            snapshot
                .match_after(&format!("{}:0", uuid::Uuid::new_v4()), "", &[])
                .is_err()
        );
        assert!(
            snapshot
                .match_after(&format!("{epoch}:999999"), "", &[])
                .is_err()
        );
        assert!(snapshot.match_after("garbage", "", &[]).is_err());
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::program_status::Incoming;

    #[test]
    fn clear_prompt_and_rejected_reports_never_refresh_retained_completion() {
        let epoch = uuid::Uuid::new_v4();
        let mut program = SurfaceProgramStatus::new();
        program
            .apply(Incoming {
                state: State::Done,
                id: "",
                kind: None,
                progress: None,
                app: None,
                title: None,
                message: None,
            })
            .unwrap();
        let baseline = ProgramObservation::new(epoch, &program).cursor;
        assert!(
            program
                .apply(Incoming {
                    state: State::Done,
                    id: "bad//id",
                    kind: None,
                    progress: None,
                    app: None,
                    title: None,
                    message: None
                })
                .is_err()
        );
        assert_eq!(ProgramObservation::new(epoch, &program).cursor, baseline);
        program.on_shell_prompt();
        program.clear("child").unwrap();
        assert!(
            ProgramObservation::new(epoch, &program)
                .match_after(&baseline, "", &[ReportedState::Done])
                .unwrap()
                .is_none()
        );
        program.clear("").unwrap();
        assert!(ProgramObservation::new(epoch, &program).records.is_empty());
    }
}
