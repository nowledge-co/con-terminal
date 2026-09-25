//! Screen-driven decisions, separate from PTY writes and durable receipts.
use super::kimi_screen::{
    Readiness, confirm_kimi_submit, kimi_instruction_pending, wait_kimi_ready,
};
use std::time::Duration;

#[derive(Debug, PartialEq)]
pub(super) enum Action {
    Wait,
    Paste,
    Submit,
    Confirmed,
}

#[derive(Default)]
pub(super) struct Delivery {
    before: Option<Vec<String>>,
    written_at: Duration,
    retried: bool,
}

impl Delivery {
    pub(super) fn baseline(&self) -> Option<Vec<String>> {
        self.before.clone()
    }

    pub(super) fn observe(
        &mut self,
        screen: &[String],
        instruction: &str,
        now: Duration,
    ) -> Action {
        if let Some(before) = &self.before {
            if confirm_kimi_submit(before, screen, instruction) {
                return Action::Confirmed;
            }
            if !self.retried
                && now.saturating_sub(self.written_at) >= Duration::from_secs(2)
                && kimi_instruction_pending(screen, instruction)
            {
                self.retried = true;
                return Action::Submit;
            }
        } else if wait_kimi_ready(screen) == Readiness::Ready {
            self.before = Some(screen.to_vec());
            self.written_at = now;
            return Action::Paste;
        }
        Action::Wait
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).into()).collect()
    }
    #[test]
    fn mock_terminal_requires_submit_evidence_and_retries_once() {
        let mut delivery = Delivery::default();
        let ready = screen(&["Welcome to Kimi Code", "│ > │"]);
        let pending = screen(&["Session:", "│ > Read context │"]);
        assert_eq!(
            delivery.observe(&screen(&[]), "Read context", Duration::ZERO),
            Action::Wait
        );
        assert_eq!(
            delivery.observe(
                &screen(&["Trust this folder", "│ > │"]),
                "Read context",
                Duration::ZERO
            ),
            Action::Wait
        );
        assert_eq!(
            delivery.observe(&ready, "Read context", Duration::ZERO),
            Action::Paste
        );
        // A mock PTY accepting the paste produces no receipt on an unchanged screen.
        assert_eq!(
            delivery.observe(&ready, "Read context", Duration::from_secs(3)),
            Action::Wait
        );
        assert_eq!(
            delivery.observe(&pending, "Read context", Duration::from_secs(1)),
            Action::Wait
        );
        assert_eq!(
            delivery.observe(&pending, "Read context", Duration::from_secs(2)),
            Action::Submit
        );
        assert_eq!(
            delivery.observe(&pending, "Read context", Duration::from_secs(4)),
            Action::Wait
        );
        let submitted = screen(&["Session: session_new", "Read context", "│ > │"]);
        assert_eq!(
            delivery.observe(&submitted, "Read context", Duration::from_secs(5)),
            Action::Confirmed
        );
    }
    #[test]
    fn user_edits_and_trust_never_trigger_submit_retry() {
        let mut delivery = Delivery::default();
        delivery.observe(&screen(&["Session:", ">"]), "Read context", Duration::ZERO);
        for lines in [
            &["Session:", "> user edit"][..],
            &["Trust this folder", "> Read context"][..],
        ] {
            assert_eq!(
                delivery.observe(&screen(lines), "Read context", Duration::from_secs(5)),
                Action::Wait
            );
        }
    }
}
