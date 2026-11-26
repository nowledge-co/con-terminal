//! Live portable VT ingress. Authentic reports remain proposals until the
//! workspace verifies ownership and the user authorizes execution.

use super::*;
use crate::resume_capture::CAPTURE_BYTES;
use con_terminal::program_resume::ResumeReport;

pub(super) const OSC_SEQUENCE_TAG: i32 = 1;

impl VtScreen {
    /// Configure a fresh live PTY before reading its first byte. The caller
    /// owns cryptographic token generation and the initial-child environment.
    pub fn enable_program_resume_capture(&self, token: String) -> anyhow::Result<()> {
        anyhow::ensure!(
            (22..=64).contains(&token.len())
                && token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
            "Invalid OSC 7577 token format"
        );
        let inner = self.inner.lock();
        let mut state = inner.callback_state.program_resume.lock();
        anyhow::ensure!(state.is_none(), "OSC 7577 capture already configured");
        for (option, value) in [
            (
                GhosttyTerminalOption::UnknownSequence,
                vt_unknown_sequence_callback as *const c_void,
            ),
            (
                GhosttyTerminalOption::UnknownMaxBytes,
                &CAPTURE_BYTES as *const usize as *const c_void,
            ),
        ] {
            // SAFETY: the terminal lock is held and both option values match
            // the pinned header. The byte limit is copied during the call.
            vt_check(
                unsafe { ghostty_terminal_set(inner.terminal, option, value) },
                "enable OSC 7577 capture",
            )?;
        }
        *state = Some(crate::resume_capture::Capture::new(token));
        Ok(())
    }

    /// Overflow invalidates the pending batch and signals the consumer to
    /// discard its candidates for this PTY incarnation.
    pub fn take_program_resume_reports(&self) -> anyhow::Result<Vec<ResumeReport>> {
        let inner = self.inner.lock();
        let mut state = inner.callback_state.program_resume.lock();
        let Some(capture) = state.as_mut() else {
            return Ok(Vec::new());
        };
        capture.take()
    }
}

#[cfg(test)]
mod tests {
    use super::VtScreen;
    use con_terminal::program_resume::ReportAction;
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    const TOKEN: &str = "dummy-token-replace-me";

    fn frame(body: &str, terminator: &str) -> String {
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let payload = format!("pid=42:time={time}:{body}");
        let mac = Sha256::digest(format!("{payload}:{TOKEN}"));
        format!("\x1b]7577;{payload}:mac={mac:x}{terminator}")
    }

    #[test]
    fn unknown_osc_callback_matches_the_linked_abi() {
        use super::{
            GhosttyTerminalUnknownOscSequence, GhosttyTerminalUnknownSequence, ghostty_type_json,
        };
        use std::mem::{offset_of, size_of};

        let ptr = unsafe { ghostty_type_json() };
        assert!(!ptr.is_null());
        let manifest = unsafe { std::ffi::CStr::from_ptr(ptr) };
        let manifest: serde_json::Value = serde_json::from_slice(manifest.to_bytes()).unwrap();
        let osc = &manifest["types"]["GhosttyTerminalUnknownOscSequence"];
        let wrapper = &manifest["types"]["GhosttyTerminalUnknownSequence"];
        assert_eq!(
            osc["size"].as_u64(),
            Some(size_of::<GhosttyTerminalUnknownOscSequence>() as u64)
        );
        assert_eq!(
            wrapper["size"].as_u64(),
            Some(size_of::<GhosttyTerminalUnknownSequence>() as u64)
        );
        assert_eq!(offset_of!(GhosttyTerminalUnknownSequence, value), 8);
        assert_eq!(offset_of!(GhosttyTerminalUnknownOscSequence, content), 8);
    }

    #[test]
    fn live_vt_delivers_complete_authentic_frames_to_the_correct_screen() {
        let screen = VtScreen::new(80, 24, None).unwrap();
        let other = VtScreen::new(80, 24, None).unwrap();
        screen
            .enable_program_resume_capture(TOKEN.to_owned())
            .unwrap();
        other
            .enable_program_resume_capture("different-terminal-token".to_owned())
            .unwrap();

        let st = frame("argv=bnZpbQ", "\x1b\\");
        screen.feed(&st.as_bytes()[..st.len() - 1]);
        assert!(screen.take_program_resume_reports().unwrap().is_empty());
        screen.feed(&st.as_bytes()[st.len() - 1..]);
        assert_eq!(screen.take_program_resume_reports().unwrap().len(), 1);

        let malformed = st.strip_suffix('\\').unwrap().to_owned() + "[0m";
        screen.feed(malformed.as_bytes());
        let contaminated = st.replace("pid=", "pi\x01d=");
        screen.feed(contaminated.as_bytes());
        assert!(screen.take_program_resume_reports().unwrap().is_empty());

        let report = frame("argv=bnZpbQ", "\x07");
        other.feed(report.as_bytes());
        assert!(other.take_program_resume_reports().unwrap().is_empty());

        for byte in &report.as_bytes()[..report.len() - 1] {
            screen.feed(std::slice::from_ref(byte));
        }
        assert!(screen.take_program_resume_reports().unwrap().is_empty());
        screen.feed(&report.as_bytes()[report.len() - 1..]);
        let reports = screen.take_program_resume_reports().unwrap();
        assert!(
            matches!(&reports.as_slice(), [report] if matches!(&report.action, ReportAction::Set(cmd) if cmd.argv == ["nvim"]))
        );

        let oversized = frame(&format!("clear=1:future={}", "a".repeat(4200)), "\x07");
        screen.feed(oversized.as_bytes());
        assert!(screen.take_program_resume_reports().unwrap().is_empty());
        screen.feed(frame("clear=1", "\x07").as_bytes());
        assert_eq!(
            screen
                .take_program_resume_reports()
                .unwrap()
                .pop()
                .unwrap()
                .action,
            ReportAction::Clear
        );
    }
}
