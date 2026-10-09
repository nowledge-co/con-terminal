//! Bounded program-status records for one terminal surface (OSC 7501).
//!
//! Ghostty parses and validates the sequence. This module stores the records
//! and applies their lifetime. A record is a presentation fact: it does not
//! authorize a tool, prove which process is running, satisfy a control-plane
//! wait, or replace the harness tracker.

use std::cmp::Ordering;

/// Hard cap from the protocol. A new id past this limit evicts the record
/// that was updated least recently. Valid records do not expire with time.
pub const MAX_RECORDS: usize = 256;

const MAX_ID_BYTES: usize = 128;
const MAX_ID_DEPTH: usize = 8;
const MAX_SEGMENT_BYTES: usize = 32;
const MAX_APP_BYTES: usize = 32;
const MAX_TITLE_BYTES: usize = 192;
const MAX_MESSAGE_BYTES: usize = 2048;

/// Why a report was ignored. Nothing was stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// The id is not a root or a legal path. It must not fall back to root.
    Id,
    /// `title` or `message` is oversized or contains a control character.
    Text,
}

/// What the program says it is doing. `clear` is [`SurfaceProgramStatus::clear`],
/// not a stored state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

/// Why a blocked program cannot continue. Absent when the program did not
/// say, or said something this version does not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

/// Machine-readable program name, such as `cargo` or `terraform`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppName(String);

impl AppName {
    pub fn parse(raw: &str) -> Option<Self> {
        if (1..=MAX_APP_BYTES).contains(&raw.len()) && raw.bytes().all(is_token_byte) {
            Some(Self(raw.to_owned()))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Record identity. An empty path is the root record.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RecordId {
    segments: Vec<String>,
}

impl RecordId {
    pub fn root() -> Self {
        Self {
            segments: Vec::new(),
        }
    }

    /// `None` when `raw` is not the root and does not match the id grammar.
    /// An illegal id must not be rewritten as root.
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return Some(Self::root());
        }
        if raw.len() > MAX_ID_BYTES {
            return None;
        }
        let mut segments = Vec::new();
        for segment in raw.split('/') {
            if segment.is_empty()
                || segment.len() > MAX_SEGMENT_BYTES
                || !segment.bytes().all(is_token_byte)
            {
                return None;
            }
            segments.push(segment.to_owned());
            if segments.len() > MAX_ID_DEPTH {
                return None;
            }
        }
        Some(Self { segments })
    }

    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    pub fn path(&self) -> String {
        self.segments.join("/")
    }

    /// `ancestor` and every record beneath it, compared by segment.
    /// `build` covers `build/test` and does not cover `builder`.
    fn is_covered_by(&self, ancestor: &Self) -> bool {
        self.segments.starts_with(&ancestor.segments)
    }
}

impl PartialOrd for RecordId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RecordId {
    fn cmp(&self, other: &Self) -> Ordering {
        self.segments.cmp(&other.segments)
    }
}

/// One validated report. Omitted strings are `None`, never a retained value.
///
/// `kind` is kept only for [`State::Blocked`]. `progress` is kept only for
/// [`State::Working`] and [`State::Blocked`], and only for 0 through 100.
/// Anything else is absent. An `app` outside the token grammar is absent.
/// Those normalizations still apply the report. [`Reject`] discards it whole.
pub struct Incoming<'a> {
    pub state: State,
    pub id: &'a str,
    pub kind: Option<BlockedKind>,
    pub progress: Option<u8>,
    pub app: Option<&'a str>,
    pub title: Option<&'a str>,
    pub message: Option<&'a str>,
}

/// A stored record with `app` already resolved from the nearest ancestor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProgramRecord {
    pub id: RecordId,
    pub state: State,
    pub kind: Option<BlockedKind>,
    pub progress: Option<u8>,
    pub app: Option<AppName>,
    pub title: Option<String>,
    pub message: Option<String>,
    /// `done` and `error` stay unseen until this surface is acknowledged.
    pub unseen: bool,
}

/// The winning record for the single activity indicator.
///
/// Severity outranks recency, so a newer idle root cannot hide an older
/// blocked child. Equal severity uses the record updated later. The
/// percentage belongs to that record alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Attention {
    pub state: State,
    pub progress: Option<u8>,
}

/// What the record set contributes to surface aggregation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contribution {
    pub attention: Option<Attention>,
    /// A root record exists, so title and progress heuristics must not
    /// speak for that scope.
    pub root_described: bool,
    /// A report was accepted. OSC 9;4 stays suppressed until a full reset.
    pub protocol_seen: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    id: RecordId,
    state: State,
    kind: Option<BlockedKind>,
    progress: Option<u8>,
    app: Option<AppName>,
    title: Option<String>,
    message: Option<String>,
    updated: u64,
    unseen: bool,
}

/// Records for one terminal. The owner drops this value when that surface
/// is closed or replaced. Another surface has its own value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SurfaceProgramStatus {
    records: Vec<Record>,
    generation: u64,
    protocol_seen: bool,
}

impl SurfaceProgramStatus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn protocol_seen(&self) -> bool {
        self.protocol_seen
    }

    pub fn contribution(&self) -> Contribution {
        let mut best: Option<(u8, u64, Attention)> = None;
        for record in &self.records {
            let Some(rank) = attention_rank(record.state, record.unseen) else {
                continue;
            };
            let candidate = (
                rank,
                record.updated,
                Attention {
                    state: record.state,
                    progress: record.progress,
                },
            );
            if best.is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1)) {
                best = Some(candidate);
            }
        }
        Contribution {
            attention: best.map(|(_, _, attention)| attention),
            root_described: self.records.iter().any(|record| record.id.is_root()),
            protocol_seen: self.protocol_seen,
        }
    }

    pub fn records(&self) -> Vec<ProgramRecord> {
        let mut records: Vec<_> = self
            .records
            .iter()
            .map(|record| ProgramRecord {
                id: record.id.clone(),
                state: record.state,
                kind: record.kind,
                progress: record.progress,
                app: self.inherited_app(&record.id),
                title: record.title.clone(),
                message: record.message.clone(),
                unseen: record.unseen,
            })
            .collect();
        records.sort_by(|left, right| left.id.cmp(&right.id));
        records
    }

    /// Plain text for the record that owns the activity indicator.
    ///
    /// The words are the protocol's own state and kind. The message is
    /// shown, not interpreted. Formatting characters that would reorder
    /// text outside the terminal grid are removed. An acknowledged
    /// completion that no longer raises the indicator is omitted, and an
    /// idle root does not hide a blocked child.
    pub fn detail_line(&self) -> Option<String> {
        let record = self.winning_record()?;
        let mut parts = Vec::new();
        if let Some(app) = self.inherited_app(&record.id) {
            parts.push(app.as_str().to_string());
        }
        if !record.id.is_root() {
            let id = present_text(&record.id.path());
            if !id.is_empty() {
                parts.push(id);
            }
        }
        parts.push(
            match record.state {
                State::Idle => "idle",
                State::Working => "working",
                State::Done => "done",
                State::Blocked => "blocked",
                State::Error => "error",
            }
            .to_string(),
        );
        if let Some(kind) = record.kind {
            parts.push(
                match kind {
                    BlockedKind::Permission => "permission",
                    BlockedKind::Question => "question",
                    BlockedKind::Auth => "auth",
                }
                .to_string(),
            );
        }
        if let Some(progress) = record.progress {
            parts.push(format!("{progress}%"));
        }
        if let Some(title) = record.title.as_deref() {
            let title = present_text(title);
            if !title.is_empty() {
                parts.push(title);
            }
        }
        let mut line = parts.join(" · ");
        if let Some(message) = record.message.as_deref() {
            let message = present_text(message);
            if !message.is_empty() {
                line.push_str(" — ");
                line.push_str(&message);
            }
        }
        let line = truncate_chars(&line, 180);
        (!line.is_empty()).then_some(line)
    }

    fn winning_record(&self) -> Option<&Record> {
        let mut best: Option<(u8, u64, usize)> = None;
        for (index, record) in self.records.iter().enumerate() {
            let Some(rank) = attention_rank(record.state, record.unseen) else {
                continue;
            };
            let candidate = (rank, record.updated, index);
            if best.is_none_or(|current| (candidate.0, candidate.1) > (current.0, current.1)) {
                best = Some(candidate);
            }
        }
        best.map(|(_, _, index)| &self.records[index])
    }

    /// Replace one record completely.
    ///
    /// Returns whether the activity contribution changed. An identical
    /// completion that was already acknowledged stays acknowledged, so a
    /// repeated report does not raise it again. Any other `done` or `error`
    /// is unseen. The bool does not mean the stored bytes were identical:
    /// a message on a record that is not winning the indicator can change
    /// without a redraw of that indicator.
    pub fn apply(&mut self, report: Incoming<'_>) -> Result<bool, Reject> {
        let id = RecordId::parse(report.id).ok_or(Reject::Id)?;
        let title = normalize_text(report.title, MAX_TITLE_BYTES)?;
        let message = normalize_text(report.message, MAX_MESSAGE_BYTES)?;
        let app = report.app.and_then(AppName::parse);
        let kind = match report.state {
            State::Blocked => report.kind,
            _ => None,
        };
        let progress = match report.state {
            State::Working | State::Blocked => report.progress.filter(|value| *value <= 100),
            _ => None,
        };
        let unseen = match report.state {
            State::Done | State::Error => !self.records.iter().any(|previous| {
                previous.id == id
                    && !previous.unseen
                    && previous.state == report.state
                    && previous.app == app
                    && previous.title == title
                    && previous.message == message
            }),
            _ => false,
        };
        let before = self.contribution();
        let updated = self.bump();
        let record = Record {
            id,
            state: report.state,
            kind,
            progress,
            app,
            title,
            message,
            updated,
            unseen,
        };
        if let Some(index) = self.position(&record.id) {
            self.records[index] = record;
        } else {
            self.evict_if_full();
            self.records.push(record);
        }
        self.protocol_seen = true;
        debug_assert!(self.records.len() <= MAX_RECORDS);
        Ok(self.contribution() != before)
    }

    /// Remove `id` and every record beneath it. An empty id removes all
    /// records. This is a protocol report, so OSC 9;4 stays suppressed.
    pub fn clear(&mut self, id: &str) -> Result<bool, Reject> {
        let id = RecordId::parse(id).ok_or(Reject::Id)?;
        let before = self.contribution();
        self.records.retain(|record| !record.id.is_covered_by(&id));
        self.protocol_seen = true;
        Ok(self.contribution() != before)
    }

    /// OSC 133 prompt start. Drops in-flight records and keeps resting ones.
    pub fn on_shell_prompt(&mut self) -> bool {
        self.drop_in_flight()
    }

    /// The process attached to this terminal exited. Same retention as a
    /// shell prompt: `done` and `error` are still there for the user to find.
    pub fn on_process_exit(&mut self) -> bool {
        self.drop_in_flight()
    }

    /// RIS (`ESC c`). Drops every record and lets OSC 9;4 contribute again.
    pub fn on_full_reset(&mut self) -> bool {
        let before = self.contribution();
        self.records.clear();
        self.protocol_seen = false;
        self.contribution() != before
    }

    /// DECSTR does not touch records.
    pub fn on_soft_reset(&mut self) -> bool {
        false
    }

    /// Primary and alternate screens share these records.
    pub fn on_alternate_screen(&mut self, _active: bool) -> bool {
        false
    }

    /// The user sent a key to this surface.
    ///
    /// That acknowledges `done` and `error` here and nowhere else. Focusing
    /// the window, or looking at a different surface, is not acknowledgement:
    /// those records stay unseen until this surface receives the key.
    /// `working`, `blocked`, and `idle` are unchanged because the program is
    /// still in that state. The records remain stored.
    pub fn acknowledge(&mut self) -> bool {
        let before = self.contribution();
        for record in &mut self.records {
            if matches!(record.state, State::Done | State::Error) {
                record.unseen = false;
            }
        }
        self.contribution() != before
    }

    fn drop_in_flight(&mut self) -> bool {
        let before = self.contribution();
        self.records
            .retain(|record| !matches!(record.state, State::Working | State::Blocked));
        self.contribution() != before
    }

    fn inherited_app(&self, id: &RecordId) -> Option<AppName> {
        (0..=id.segments.len()).rev().find_map(|len| {
            self.records.iter().find_map(|record| {
                (record.id.segments == id.segments[..len])
                    .then_some(record.app.clone())
                    .flatten()
            })
        })
    }

    fn position(&self, id: &RecordId) -> Option<usize> {
        self.records.iter().position(|record| record.id == *id)
    }

    fn bump(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    fn evict_if_full(&mut self) {
        if self.records.len() < MAX_RECORDS {
            return;
        }
        let victim = self
            .records
            .iter()
            .enumerate()
            .min_by_key(|(index, record)| (record.updated, *index))
            .map(|(index, _)| index);
        if let Some(index) = victim {
            self.records.swap_remove(index);
        }
    }
}

/// Acknowledged completions occupy their id without raising the indicator.
fn attention_rank(state: State, unseen: bool) -> Option<u8> {
    match state {
        State::Error if unseen => Some(5),
        State::Blocked => Some(4),
        State::Working => Some(3),
        State::Done if unseen => Some(2),
        State::Idle => Some(1),
        State::Done | State::Error => None,
    }
}

fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-')
}

fn present_text(value: &str) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for ch in value.chars() {
        if is_hidden_format(ch) {
            continue;
        }
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(ch);
    }
    out
}

fn is_hidden_format(ch: char) -> bool {
    let code = u32::from(ch);
    ch.is_control()
        || matches!(code, 0x061C | 0x200B | 0x200E | 0x200F)
        || (0x202A..=0x202E).contains(&code)
        || (0x2060..=0x206F).contains(&code)
        || code == 0xFEFF
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let count = value.chars().count();
    if count <= max_chars {
        return value.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = value.chars().take(keep).collect();
    out.push('…');
    out
}

fn has_control(text: &str) -> bool {
    text.chars().any(|ch| {
        let code = u32::from(ch);
        code <= 0x1F || code == 0x7F || (0x80..=0x9F).contains(&code)
    })
}

fn normalize_text(value: Option<&str>, max_bytes: usize) -> Result<Option<String>, Reject> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.len() > max_bytes || has_control(value) {
        return Err(Reject::Text);
    }
    Ok(Some(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn incoming<'a>(
        state: State,
        id: &'a str,
        app: Option<&'a str>,
        title: Option<&'a str>,
        message: Option<&'a str>,
    ) -> Incoming<'a> {
        Incoming {
            state,
            id,
            kind: None,
            progress: None,
            app,
            title,
            message,
        }
    }

    fn record<'a>(records: &'a [ProgramRecord], id: &str) -> &'a ProgramRecord {
        records
            .iter()
            .find(|record| record.id.path() == id)
            .unwrap_or_else(|| panic!("missing record {id}"))
    }

    #[test]
    fn a_report_replaces_its_record_and_drops_omitted_fields() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(Incoming {
                state: State::Working,
                id: "",
                kind: Some(BlockedKind::Permission),
                progress: Some(40),
                app: Some("brew"),
                title: Some("Upgrade"),
                message: Some("Installing updates"),
            })
            .unwrap();
        assert!(
            surface
                .apply(Incoming {
                    state: State::Working,
                    id: "",
                    kind: Some(BlockedKind::Auth),
                    progress: Some(101),
                    app: None,
                    title: None,
                    message: Some(""),
                })
                .unwrap()
        );
        assert!(
            !surface
                .apply(incoming(State::Working, "", None, None, None))
                .unwrap()
        );
        let stored = surface.records();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].state, State::Working);
        assert_eq!(stored[0].kind, None);
        assert_eq!(stored[0].progress, None);
        assert_eq!(stored[0].app, None);
        assert_eq!(stored[0].title, None);
        assert_eq!(stored[0].message, None);
    }

    #[test]
    fn blocked_keeps_kind_and_progress_and_other_states_do_not() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(Incoming {
                state: State::Blocked,
                id: "plan",
                kind: Some(BlockedKind::Permission),
                progress: Some(0),
                app: Some("terraform"),
                title: None,
                message: Some("Apply 3 to add?"),
            })
            .unwrap();
        let blocked = &surface.records()[0];
        assert_eq!(blocked.kind, Some(BlockedKind::Permission));
        assert_eq!(blocked.progress, Some(0));
        surface
            .apply(Incoming {
                state: State::Done,
                id: "plan",
                kind: Some(BlockedKind::Question),
                progress: Some(100),
                app: Some("not a legal app"),
                title: None,
                message: Some("Applied"),
            })
            .unwrap();
        let done = &surface.records()[0];
        assert_eq!(done.state, State::Done);
        assert_eq!(done.kind, None);
        assert_eq!(done.progress, None);
        assert_eq!(done.app, None);
        assert!(done.unseen);
    }

    #[test]
    fn illegal_ids_and_control_text_leave_storage_untouched() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(incoming(
                State::Idle,
                "",
                Some("shell"),
                None,
                Some("ready"),
            ))
            .unwrap();
        for id in [
            "build/",
            "/build",
            "build//test",
            "builder extra",
            "a/b/c/d/e/f/g/h/i",
        ] {
            assert_eq!(
                surface.apply(incoming(State::Working, id, None, None, None)),
                Err(Reject::Id)
            );
        }
        assert_eq!(
            surface.apply(incoming(
                State::Error,
                "",
                Some("shell"),
                None,
                Some("failed\nbadly"),
            )),
            Err(Reject::Text)
        );
        assert_eq!(
            surface.apply(incoming(
                State::Error,
                "",
                None,
                Some(&"x".repeat(MAX_TITLE_BYTES + 1)),
                None,
            )),
            Err(Reject::Text)
        );
        let message = "m".repeat(MAX_MESSAGE_BYTES + 1);
        assert_eq!(
            surface.apply(incoming(State::Error, "", None, None, Some(&message))),
            Err(Reject::Text)
        );
        assert!(surface.protocol_seen());
        let stored = &surface.records()[0];
        assert_eq!(stored.state, State::Idle);
        assert_eq!(stored.message.as_deref(), Some("ready"));
        assert_eq!(stored.app.as_ref().unwrap().as_str(), "shell");
    }

    #[test]
    fn app_comes_from_the_nearest_ancestor_and_build_is_not_builder() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(incoming(State::Working, "", Some("deploy"), None, None))
            .unwrap();
        surface
            .apply(incoming(State::Working, "build", Some("cargo"), None, None))
            .unwrap();
        surface
            .apply(incoming(State::Working, "build/test", None, None, None))
            .unwrap();
        surface
            .apply(incoming(
                State::Blocked,
                "builder",
                None,
                None,
                Some("wait"),
            ))
            .unwrap();
        surface
            .apply(incoming(
                State::Working,
                "build/test/unit",
                None,
                None,
                None,
            ))
            .unwrap();
        let records = surface.records();
        assert_eq!(
            record(&records, "build/test")
                .app
                .as_ref()
                .unwrap()
                .as_str(),
            "cargo"
        );
        assert_eq!(
            record(&records, "build/test/unit")
                .app
                .as_ref()
                .unwrap()
                .as_str(),
            "cargo"
        );
        assert_eq!(
            record(&records, "builder").app.as_ref().unwrap().as_str(),
            "deploy"
        );
        assert_ne!(record(&records, "builder").id.path(), "build");

        surface
            .apply(incoming(State::Working, "orphan/job", None, None, None))
            .unwrap();
        assert_eq!(
            record(&surface.records(), "orphan/job")
                .app
                .as_ref()
                .unwrap()
                .as_str(),
            "deploy"
        );
        surface.clear("build").unwrap();
        let records = surface.records();
        assert!(records.iter().all(|record| record.id.path() != "build"));
        assert!(
            records
                .iter()
                .all(|record| !record.id.path().starts_with("build/"))
        );
        assert_eq!(record(&records, "builder").state, State::Blocked);
        assert_eq!(
            record(&records, "").app.as_ref().unwrap().as_str(),
            "deploy"
        );
    }

    #[test]
    fn clearing_root_removes_every_record_without_forgetting_the_protocol() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(incoming(State::Done, "child", None, None, Some("ok")))
            .unwrap();
        assert!(surface.clear("").unwrap());
        assert!(surface.records().is_empty());
        assert!(surface.protocol_seen());
        assert!(surface.clear("nope/").is_err());
    }

    #[test]
    fn the_least_recently_updated_record_is_evicted_at_the_cap() {
        let mut surface = SurfaceProgramStatus::new();
        for index in 0..MAX_RECORDS {
            surface
                .apply(incoming(
                    State::Working,
                    &format!("job{index}"),
                    None,
                    None,
                    None,
                ))
                .unwrap();
        }
        surface
            .apply(incoming(State::Idle, "job1", None, None, None))
            .unwrap();
        surface
            .apply(incoming(State::Working, "job256", None, None, None))
            .unwrap();
        let records = surface.records();
        assert_eq!(records.len(), MAX_RECORDS);
        assert!(records.iter().all(|record| record.id.path() != "job0"));
        assert!(records.iter().any(|record| record.id.path() == "job1"));
        assert!(records.iter().any(|record| record.id.path() == "job256"));
        surface
            .apply(incoming(State::Working, "job2", None, None, Some("still")))
            .unwrap();
        surface
            .apply(incoming(State::Working, "job257", None, None, None))
            .unwrap();
        let records = surface.records();
        assert!(records.iter().all(|record| record.id.path() != "job3"));
        assert_eq!(record(&records, "job2").message.as_deref(), Some("still"));
    }

    #[test]
    fn prompt_and_exit_drop_in_flight_work_and_keep_results() {
        for end in [
            SurfaceProgramStatus::on_shell_prompt,
            SurfaceProgramStatus::on_process_exit,
        ] {
            let mut surface = SurfaceProgramStatus::new();
            surface
                .apply(incoming(State::Working, "", None, None, None))
                .unwrap();
            surface
                .apply(incoming(State::Blocked, "ask", None, None, None))
                .unwrap();
            surface
                .apply(incoming(State::Idle, "shell", None, None, None))
                .unwrap();
            surface
                .apply(incoming(State::Done, "task", None, None, Some("done")))
                .unwrap();
            surface
                .apply(incoming(
                    State::Error,
                    "task/fail",
                    None,
                    None,
                    Some("nope"),
                ))
                .unwrap();
            assert!(end(&mut surface));
            let records = surface.records();
            assert!(records.iter().all(|record| {
                matches!(record.state, State::Idle | State::Done | State::Error)
            }));
            assert!(record(&records, "task").unseen);
            assert!(record(&records, "task/fail").unseen);
            assert!(!end(&mut surface));
        }
    }

    #[test]
    fn reset_and_screen_changes_follow_the_protocol() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(incoming(State::Working, "build", Some("cargo"), None, None))
            .unwrap();
        assert!(!surface.on_soft_reset());
        assert!(!surface.on_alternate_screen(true));
        assert!(!surface.on_alternate_screen(false));
        assert_eq!(surface.records().len(), 1);
        assert!(surface.protocol_seen());
        assert!(surface.on_full_reset());
        assert!(surface.records().is_empty());
        assert!(!surface.protocol_seen());
        assert!(!surface.on_full_reset());
    }

    #[test]
    fn an_idle_root_does_not_hide_a_blocked_child() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(Incoming {
                state: State::Working,
                id: "eu-west",
                kind: None,
                progress: Some(80),
                app: None,
                title: None,
                message: None,
            })
            .unwrap();
        surface
            .apply(incoming(
                State::Idle,
                "",
                Some("deploy"),
                None,
                Some("waiting"),
            ))
            .unwrap();
        surface
            .apply(Incoming {
                state: State::Blocked,
                id: "us-east",
                kind: Some(BlockedKind::Auth),
                progress: Some(15),
                app: None,
                title: Some("US East"),
                message: Some("Password required"),
            })
            .unwrap();
        let contribution = surface.contribution();
        assert_eq!(
            contribution.attention,
            Some(Attention {
                state: State::Blocked,
                progress: Some(15),
            })
        );
        assert!(contribution.root_described);
        let records = surface.records();
        assert_eq!(records.len(), 3);
        assert_eq!(record(&records, "").state, State::Idle);
        assert_eq!(record(&records, "us-east").kind, Some(BlockedKind::Auth));
    }

    #[test]
    fn acknowledgement_is_per_surface_and_does_not_clear_in_flight_work() {
        let mut first = SurfaceProgramStatus::new();
        let mut second = SurfaceProgramStatus::new();
        first
            .apply(incoming(State::Done, "", None, None, Some("shipped")))
            .unwrap();
        second
            .apply(incoming(State::Error, "", None, None, Some("failed")))
            .unwrap();
        assert!(first.acknowledge());
        first
            .apply(incoming(
                State::Blocked,
                "ask",
                None,
                None,
                Some("approve?"),
            ))
            .unwrap();
        assert!(!first.acknowledge());
        let records = first.records();
        assert!(!record(&records, "").unseen);
        assert_eq!(record(&records, "ask").state, State::Blocked);
        assert_eq!(
            first
                .contribution()
                .attention
                .map(|attention| attention.state),
            Some(State::Blocked)
        );
        assert!(second.records()[0].unseen);
        assert_eq!(second.contribution().attention.unwrap().state, State::Error);

        let mut done = SurfaceProgramStatus::new();
        done.apply(incoming(State::Done, "", None, None, Some("shipped")))
            .unwrap();
        assert!(done.acknowledge());
        assert!(
            !done
                .apply(incoming(State::Done, "", None, None, Some("shipped")))
                .unwrap()
        );
        assert!(!done.records()[0].unseen);
        assert!(
            done.apply(incoming(State::Done, "", None, None, Some("shipped again")))
                .unwrap()
        );
        assert!(done.records()[0].unseen);
    }

    #[test]
    fn a_message_is_stored_literally() {
        let mut surface = SurfaceProgramStatus::new();
        let message = "error: not really, still working";
        surface
            .apply(incoming(
                State::Working,
                "",
                Some("cargo"),
                None,
                Some(message),
            ))
            .unwrap();
        let stored = &surface.records()[0];
        assert_eq!(stored.state, State::Working);
        assert_eq!(stored.message.as_deref(), Some(message));
        assert_eq!(
            surface.detail_line().as_deref(),
            Some("cargo · working — error: not really, still working")
        );
    }

    #[test]
    fn the_detail_line_names_the_blocked_child_and_drops_hidden_formatting() {
        let mut surface = SurfaceProgramStatus::new();
        surface
            .apply(incoming(State::Idle, "", None, None, Some("idle root")))
            .unwrap();
        surface
            .apply(Incoming {
                state: State::Blocked,
                id: "build/test",
                kind: Some(BlockedKind::Permission),
                progress: Some(40),
                app: Some("cargo"),
                title: Some("Plan"),
                message: Some("Apply?\u{202E}hidden"),
            })
            .unwrap();
        assert_eq!(
            surface.detail_line().as_deref(),
            Some("cargo · build/test · blocked · permission · 40% · Plan — Apply?hidden")
        );
        surface.acknowledge();
        assert_eq!(
            surface.detail_line().as_deref(),
            Some("cargo · build/test · blocked · permission · 40% · Plan — Apply?hidden")
        );
    }

    #[test]
    fn detail_text_preserves_joiners_but_removes_direction_overrides() {
        assert_eq!(
            super::present_text("👩\u{200D}💻 ع\u{200C}رب\u{061C}\u{2060}\u{2067}"),
            "👩\u{200D}💻 ع\u{200C}رب"
        );
    }
}
