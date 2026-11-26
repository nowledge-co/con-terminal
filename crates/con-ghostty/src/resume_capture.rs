//! A bounded, authenticated ingress queue shared by the Ghostty backends.

use con_terminal::program_resume::{ResumeReport, parse_report};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const CAPTURE_BYTES: usize = 4096 + b"7577;".len();
const MAX_PENDING_REPORTS: usize = 16;

pub(crate) struct Capture {
    token: String,
    reports: Vec<ResumeReport>,
    overflowed: bool,
}

impl Capture {
    pub(crate) fn new(token: String) -> Self {
        Self {
            token,
            reports: Vec::new(),
            overflowed: false,
        }
    }

    pub(crate) fn take(&mut self) -> anyhow::Result<Vec<ResumeReport>> {
        anyhow::ensure!(!self.overflowed, "OSC 7577 capture overflowed");
        Ok(std::mem::take(&mut self.reports))
    }

    pub(crate) fn receive(&mut self, bytes: &[u8]) {
        let Some(body) = bytes.strip_prefix(b"7577;") else {
            return;
        };
        if self.overflowed {
            return;
        }
        let Ok(body) = std::str::from_utf8(body) else {
            return;
        };
        let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
            return;
        };
        let Some(report) = parse_report(body, &self.token, now.as_secs()) else {
            return;
        };
        if self.reports.len() == MAX_PENDING_REPORTS {
            // Dropping a clear or reject could preserve a superseded command.
            self.reports.clear();
            self.overflowed = true;
            return;
        }
        self.reports.push(report);
    }
}
