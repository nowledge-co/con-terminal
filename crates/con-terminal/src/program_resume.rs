//! OSC 7577 revision 0.5 report validation. The receiver must separately
//! authorize the owner and the exact command before saving or executing it.

use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::Command,
};
use subtle::ConstantTimeEq;

pub const TOKEN_ENV: &str = "OSC7577_TOKEN";

/// Generate a fresh token for one PTY incarnation. The caller must pass it
/// only to the initial child and its own ingress verifier.
pub fn new_token() -> Result<String, getrandom::Error> {
    let mut random = [0; 32];
    getrandom::fill(&mut random)?;
    Ok(URL_SAFE_NO_PAD.encode(random))
}

pub const TERMINAL_CONTEXT_ENV: &str = "CON_PROGRAM_RESUME_CONTEXT";

/// One initial-child environment and the verifier's copy of its capability.
pub struct PtyResumeEnvironment {
    token: String,
    context: uuid::Uuid,
}

impl PtyResumeEnvironment {
    pub fn new() -> Result<Self, getrandom::Error> {
        Ok(Self {
            token: new_token()?,
            context: uuid::Uuid::new_v4(),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn context(&self) -> uuid::Uuid {
        self.context
    }

    pub fn variables(&self) -> [(&'static str, String); 2] {
        [
            (TERMINAL_CONTEXT_ENV, self.context.to_string()),
            (TOKEN_ENV, self.token.clone()),
        ]
    }
}

const MAX_BODY_BYTES: usize = 4096;
const MAX_ARGUMENTS: usize = 128;
const TIME_WINDOW_SECS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResumeCommand {
    pub argv: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// A quoted representation for display only. Keep the argv boundaries and
/// escape invisible formatting characters that could reorder a review.
#[derive(Debug, PartialEq, Eq)]
pub struct ResumeReview {
    pub executable: String,
    pub arguments: Vec<String>,
    pub directory: String,
}

impl ResumeCommand {
    /// Revalidate the shape after reading a saved record, before presenting it.
    pub fn validate(&self) -> io::Result<()> {
        self.parts().map(|_| ())
    }

    pub fn review(&self) -> io::Result<ResumeReview> {
        let (program, args, cwd) = self.parts()?;
        Ok(ResumeReview {
            executable: format!("{program:?}"),
            arguments: args.iter().map(|arg| format!("{arg:?}")).collect(),
            directory: format!("{:?}", cwd.to_str().expect("validated UTF-8 directory")),
        })
    }

    fn parts(&self) -> io::Result<(&str, &[String], &Path)> {
        let cwd = self
            .cwd
            .as_deref()
            .filter(|path| {
                path.is_absolute()
                    && path.to_str().is_some_and(|directory| {
                        directory.len() <= MAX_BODY_BYTES
                            && !directory.chars().any(char::is_control)
                    })
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Invalid resume directory")
            })?;
        let (program, args) = self.argv.split_first().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Missing resume executable")
        })?;
        let combined_bytes = self.argv.iter().map(String::len).sum::<usize>();
        if !valid_program_name(program)
            || self.argv.len() > MAX_ARGUMENTS
            || combined_bytes > MAX_BODY_BYTES
            || self
                .argv
                .iter()
                .any(|arg| arg.chars().any(char::is_control))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid resume arguments",
            ));
        }
        Ok((program, args, cwd))
    }

    /// Construct an approved launch in the caller's environment. This does
    /// not authorize a launch. The caller must review the exact value with
    /// the user, consume the saved record once, and keep the job visible.
    pub fn literal_command(&self) -> io::Result<Command> {
        let (program, args, cwd) = self.parts()?;
        if !cwd.metadata()?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Resume directory is unavailable",
            ));
        }

        #[cfg(windows)]
        let executable = resolve_windows_executable(program)?;
        #[cfg(not(windows))]
        let executable = program;
        let mut launch = Command::new(executable);
        launch.args(args).current_dir(cwd);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            launch.arg0(program);
        }
        Ok(launch)
    }
}

fn valid_program_name(program: &str) -> bool {
    !program.is_empty()
        && (Path::new(program).is_absolute()
            || (!program.starts_with('-')
                && !program.contains(['/', '\\', ':'])
                && !matches!(program, "." | "..")))
}

#[cfg(windows)]
fn resolve_windows_executable(program: &str) -> io::Result<PathBuf> {
    // ConPTY connects to programs using Win32's command-line convention.
    // Require a native executable so std does not hand the arguments to cmd.
    let path = Path::new(program);
    let exe = std::ffi::OsStr::new("exe");
    let is_exe = |path: &Path| {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case(exe))
    };
    let missing = || io::Error::new(io::ErrorKind::NotFound, "Resume executable unavailable");
    if path.is_absolute() {
        return (is_exe(path) && path.is_file())
            .then(|| path.to_owned())
            .ok_or_else(missing);
    }
    let name = if is_exe(path) {
        path.to_owned()
    } else {
        PathBuf::from(format!("{program}.exe"))
    };
    let search = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&search)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(&name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(missing)
}

#[derive(Debug, PartialEq, Eq)]
pub enum ReportAction {
    Set(ResumeCommand),
    Clear,
    /// An authentic owner changed its report to one we cannot accept.
    Reject,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ResumeReport {
    pub pid: NonZeroU32,
    pub time: u64,
    pub action: ReportAction,
}

/// Replay ordering for one live PTY. Keep clear/reject timestamps until
/// their reports expire so captured older reports cannot resurrect a record.
#[derive(Default)]
pub struct ReportOrder {
    accepted: HashMap<NonZeroU32, u64>,
}

impl ReportOrder {
    /// Apply only after authenticating the report and checking its live owner.
    /// A full window fails closed instead of evicting a live replay barrier.
    pub fn observe(&mut self, report: &ResumeReport, now: u64) -> bool {
        const MAX_TRACKED_OWNERS: usize = 128;
        self.accepted
            .retain(|_, time| *time >= now.saturating_sub(TIME_WINDOW_SECS));
        let at_capacity = self.accepted.len() >= MAX_TRACKED_OWNERS;
        match self.accepted.entry(report.pid) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if report.time < *entry.get() {
                    return false;
                }
                entry.insert(report.time);
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                if at_capacity {
                    return false;
                }
                entry.insert(report.time);
            }
        }
        true
    }
}

/// Process incarnation independently observed by the host for this PTY.
/// A PID from a report alone is never sufficient to populate this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumeOwner {
    pub pid: NonZeroU32,
    pub started_at: u64,
}

/// One live surface's candidate. This state does not authorize a launch.
#[derive(Default)]
pub struct LiveResume {
    order: ReportOrder,
    candidate: Option<(ResumeOwner, ResumeCommand)>,
}

impl LiveResume {
    /// The host supplies the current verified owner (including PTY membership)
    /// and an absolute, independently observed working directory when available.
    pub fn apply(
        &mut self,
        owner: ResumeOwner,
        report: ResumeReport,
        observed_cwd: Option<PathBuf>,
        now: u64,
    ) -> bool {
        if owner.pid != report.pid
            || report.time.abs_diff(now) > TIME_WINDOW_SECS
            || !self.order.observe(&report, now)
        {
            return false;
        }
        if let ReportAction::Set(mut command) = report.action {
            if command.cwd.is_none() {
                command.cwd = observed_cwd;
            }
            if command.validate().is_ok() {
                if self
                    .candidate
                    .as_ref()
                    .is_some_and(|(current, existing)| *current == owner && *existing == command)
                {
                    return false;
                }
                self.candidate = Some((owner, command));
                return true;
            }
        }
        if self
            .candidate
            .as_ref()
            .is_some_and(|(current, _)| *current == owner)
        {
            self.candidate = None;
            return true;
        }
        false
    }

    pub fn candidate(&self) -> Option<(&ResumeOwner, &ResumeCommand)> {
        self.candidate
            .as_ref()
            .map(|(owner, command)| (owner, command))
    }

    /// Compare complete process incarnations. An unrelated exit must never
    /// discard another process's record.
    pub fn owner_exited(&mut self, owner: ResumeOwner) -> bool {
        if self
            .candidate
            .as_ref()
            .is_some_and(|(current, _)| *current == owner)
        {
            self.candidate = None;
            return true;
        }
        false
    }

    /// An incomplete ingress batch cannot safely preserve a prior candidate.
    pub fn invalidate_candidate(&mut self) {
        self.candidate = None;
    }
}

/// Validate an entire OSC body. `None` must leave all records unchanged.
/// The receiver must check chronological ordering per live owner, including
/// authenticated rejects and clears, before mutating its records.
pub fn parse_report(body: &str, token: &str, now: u64) -> Option<ResumeReport> {
    if body.len() > MAX_BODY_BYTES {
        return None;
    }
    let (payload, mac) = body.rsplit_once(":mac=")?;
    let mut received = [0; 32];
    if mac.len() != 64 {
        return None;
    }
    for (index, bytes) in mac.as_bytes().chunks_exact(2).enumerate() {
        received[index] = (lower_hex(bytes[0])? << 4) | lower_hex(bytes[1])?;
    }
    let expected = Sha256::new()
        .chain_update(payload)
        .chain_update(b":")
        .chain_update(token)
        .finalize();
    if !bool::from(expected.as_slice().ct_eq(&received)) {
        return None;
    }

    let pid = field_once(payload, "pid")
        .and_then(decimal)
        .and_then(|n| n.try_into().ok())
        .and_then(NonZeroU32::new)?;
    let time = field_once(payload, "time").and_then(decimal)?;
    if now.abs_diff(time) > TIME_WINDOW_SECS {
        return None;
    }
    Some(ResumeReport {
        pid,
        time,
        action: parse_action(payload).unwrap_or(ReportAction::Reject),
    })
}

fn lower_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn decimal(value: &str) -> Option<u64> {
    if !matches!(value.as_bytes().first(), Some(b'1'..=b'9'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

fn field_once<'a>(payload: &'a str, key: &str) -> Option<&'a str> {
    let mut values = payload
        .split(':')
        .filter_map(|pair| pair.split_once('='))
        .filter_map(|(name, value)| (name == key).then_some(value));
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn parse_action(payload: &str) -> Option<ReportAction> {
    let mut fields = HashMap::new();
    for pair in payload.split(':') {
        let (key, value) = pair.split_once('=')?;
        if key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.,+/=-".contains(&b))
            || key == "mac"
            || fields.insert(key, value).is_some()
        {
            return None;
        }
    }
    if fields.get("need").is_some_and(|need| {
        need.split(',').any(|key| {
            !matches!(
                key,
                "pid" | "time" | "mac" | "argv" | "cwd" | "clear" | "need"
            )
        })
    }) {
        return None;
    }
    let cwd = match fields.get("cwd") {
        Some(value) => {
            let path = PathBuf::from(decode(value)?);
            if !path.is_absolute() {
                return None;
            }
            Some(path)
        }
        None => None,
    };
    match (fields.get("argv"), fields.get("clear")) {
        (None, Some(&"1")) => Some(ReportAction::Clear),
        (Some(argv), None) => {
            let argv = argv
                .split(',')
                .take(MAX_ARGUMENTS + 1)
                .map(decode)
                .collect::<Option<Vec<_>>>()?;
            if argv.len() > MAX_ARGUMENTS || !valid_program_name(&argv[0]) {
                return None;
            }
            Some(ReportAction::Set(ResumeCommand { argv, cwd }))
        }
        _ => None,
    }
}

fn decode(value: &str) -> Option<String> {
    let bytes = if value.ends_with('=') {
        STANDARD.decode(value)
    } else {
        STANDARD_NO_PAD.decode(value)
    }
    .ok()?;
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "dummy-token-replace-me";
    const NOW: u64 = 1795543200;

    fn signed(payload: &str) -> String {
        format!(
            "{payload}:mac={:x}",
            Sha256::digest(format!("{payload}:{TOKEN}"))
        )
    }

    #[test]
    fn review_keeps_arguments_distinct_and_displays_bidi_controls() {
        let cwd = std::env::temp_dir();
        let command = ResumeCommand {
            argv: vec![
                "example".into(),
                "".into(),
                "a b".into(),
                "a\u{202e}b".into(),
            ],
            cwd: Some(cwd.clone()),
        };
        let review = command.review().unwrap();
        assert_eq!(review.executable, r#""example""#);
        assert_eq!(review.arguments, [r#""""#, r#""a b""#, r#""a\u{202e}b""#]);
        assert_eq!(review.directory, format!("{:?}", cwd.to_str().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn literal_launch_preserves_argument_boundaries_and_directory() {
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let launch = ResumeCommand {
            argv: [
                "/bin/sh",
                "-c",
                r#"printf '%s\n%s\n%s\n%s\n' "$PWD" "$1" "$2" "$3""#,
                "fixture",
                "",
                "$(printf evaluated)",
                "a; b",
            ]
            .map(str::to_owned)
            .to_vec(),
            cwd: Some(cwd.clone()),
        };
        let output = launch.literal_command().unwrap().output().unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{}\n\n$(printf evaluated)\na; b\n", cwd.display())
        );
    }

    #[test]
    fn literal_launch_rechecks_deserialized_command_shape() {
        let cwd = Some(std::env::temp_dir());
        for executable in ["", "dir/program", "../program", "-program", "program\0"] {
            let command = ResumeCommand {
                argv: vec![executable.to_owned()],
                cwd: cwd.clone(),
            };
            assert_eq!(
                command.literal_command().unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn accepts_published_vector_and_preserves_exact_arguments() {
        let body = "pid=74877:time=1795543200:argv=bnZpbQ==,LVM=,L2hvbWUvdXNlci8ubG9jYWwvc3RhdGUvbnZpbS9zZXNzaW9ucy84ZjJjMWQudmlt:mac=9dbb99bfb856749a87563839d7ce9062818d78cf25909db93f309e0cd3b1909b";
        let report = parse_report(body, TOKEN, NOW).unwrap();
        let ReportAction::Set(command) = report.action else {
            panic!("expected command")
        };
        assert_eq!(
            command.argv,
            [
                "nvim",
                "-S",
                "/home/user/.local/state/nvim/sessions/8f2c1d.vim"
            ]
        );
        let report = parse_report(
            &signed("pid=1:time=1795543200:argv=bnZpbQ,,JCh4KQ"),
            TOKEN,
            NOW,
        )
        .unwrap();
        let ReportAction::Set(command) = report.action else {
            panic!("expected command")
        };
        assert_eq!(command.argv, ["nvim", "", "$(x)"]);
    }

    #[test]
    fn invalid_mac_or_stale_time_has_no_effect() {
        let body = signed("pid=1:time=1795543200:clear=1");
        assert!(parse_report(&body, "wrong-terminal-token", NOW).is_none());
        assert!(parse_report(&body, TOKEN, NOW + 61).is_none());
        assert!(parse_report(&body, TOKEN, NOW - 61).is_none());
        assert!(parse_report(&body[..body.len() - 1], TOKEN, NOW).is_none());
        assert_eq!(
            parse_report(&body, TOKEN, NOW).unwrap().action,
            ReportAction::Clear
        );
    }

    #[test]
    fn clear_keeps_its_replay_barrier() {
        let mut order = ReportOrder::default();
        let set = parse_report(&signed("pid=1:time=1795543200:argv=bnZpbQ"), TOKEN, NOW).unwrap();
        let clear = parse_report(&signed("pid=1:time=1795543201:clear=1"), TOKEN, NOW).unwrap();
        assert!(order.observe(&set, NOW));
        assert!(order.observe(&clear, NOW + 1));
        assert!(!order.observe(&set, NOW + 1));
    }

    #[cfg(unix)]
    #[test]
    fn live_candidate_clear_and_exit_are_scoped_to_the_owner_incarnation() {
        let first = ResumeOwner {
            pid: NonZeroU32::new(1).unwrap(),
            started_at: 1,
        };
        let reused = ResumeOwner {
            started_at: 2,
            ..first
        };
        let other = ResumeOwner {
            pid: NonZeroU32::new(2).unwrap(),
            ..first
        };
        let mut state = LiveResume::default();
        let set =
            || parse_report(&signed("pid=1:time=1795543200:argv=bnZpbQ"), TOKEN, NOW).unwrap();
        let clear = || parse_report(&signed("pid=2:time=1795543200:clear=1"), TOKEN, NOW).unwrap();

        assert!(!state.apply(other, set(), Some("/project".into()), NOW));
        assert!(!state.apply(first, set(), Some("relative".into()), NOW));
        assert!(state.apply(first, set(), Some("/project".into()), NOW));
        assert_eq!(
            state.candidate().unwrap().1.cwd.as_deref(),
            Some(std::path::Path::new("/project"))
        );
        assert!(!state.apply(other, clear(), None, NOW));
        assert!(!state.owner_exited(reused));
        assert!(state.owner_exited(first));
        assert!(state.candidate().is_none());
    }

    #[test]
    fn authenticated_invalid_command_rejects_the_owner() {
        for tail in [
            "argv=Li9ydW4uc2g=",
            "argv=bnZpbQ==:cwd=cHJvamVjdA==",
            "argv=bnZpbQ==:need=future",
            "argv=bnZpbQ==:clear=1",
            "argv=bnZpbQ==,Cg==",
            "argv=bnZpbQ=",
            "clear=1:clear=1",
        ] {
            let body = signed(&format!("pid=1:time=1795543200:{tail}"));
            assert_eq!(
                parse_report(&body, TOKEN, NOW).unwrap().action,
                ReportAction::Reject,
                "{tail}"
            );
        }
    }
}
