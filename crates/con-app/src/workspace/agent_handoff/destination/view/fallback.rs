use super::*;
use con_core::handoff::{FallbackKind, HandoffState};

pub(super) struct Guide {
    pub steps: [String; 3],
    pub ready: bool,
    pub abandon_primary: bool,
}

impl Guide {
    pub(super) fn new(job: &HandoffJob, copied_now: bool) -> Self {
        let kind = job
            .fallback
            .as_ref()
            .map_or(FallbackKind::ArgvBootstrap, |g| g.kind);
        let copied = copied_now
            || job.state == HandoffState::AwaitingManualDelivery
            || job.fallback.as_ref().is_some_and(|g| g.clipboard_copied);
        let ready =
            job.state == HandoffState::AwaitingManualDelivery || job.fallback_confirmation_ready();
        let target = job.target.agent.label();
        let paste = match kind {
            FallbackKind::NotReady => "Wait, then ⌘V in Kimi".into(),
            FallbackKind::Trust => "Tap Trust this folder, then ⌘V".into(),
            FallbackKind::SubmitUncertain => "Paste again if empty — ⌘V".into(),
            FallbackKind::LaunchTimeout => "Open target Tab or send again".into(),
            FallbackKind::ArgvBootstrap if job.state == HandoffState::NeedsInteraction => {
                "Fix login/network in target, then ⌘V".into()
            }
            FallbackKind::TargetDead => "Abandon this handoff".into(),
            _ => format!("Paste in {target} — ⌘V"),
        };
        Self {
            steps: [
                if copied {
                    "Copied to clipboard"
                } else {
                    "Use Copy again to copy instruction"
                }
                .into(),
                paste,
                if ready {
                    "Handoff sent — confirm"
                } else {
                    "Waiting for paste…"
                }
                .into(),
            ],
            ready,
            abandon_primary: matches!(
                kind,
                FallbackKind::TargetDead
                    | FallbackKind::LaunchTimeout
                    | FallbackKind::ArgvBootstrap
            ),
        }
    }

    pub(super) fn render(&self, palette: &Palette) -> Div {
        let mut steps = card_row().flex_col().items_start().gap_2();
        for (index, (text, icon)) in self
            .steps
            .iter()
            .zip([
                "phosphor/clipboard-text.svg",
                "phosphor/arrow-square-out.svg",
                "phosphor/check-circle-fill.svg",
            ])
            .enumerate()
        {
            let color = if index == 2 && self.ready {
                palette.primary
            } else {
                palette.muted_foreground
            };
            steps = steps.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .w_full()
                    .child(row_icon_tinted(icon, color))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_sm()
                            .text_color(color)
                            .child(text.clone()),
                    ),
            );
        }
        steps
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentKind, FallbackKind, Guide, HandoffJob, HandoffState};
    use con_core::handoff::{FallbackEvidence, HandoffFallbackGuide, PrepareRequest};
    #[test]
    fn seven_guides_have_three_steps_and_correct_confirmation_priority() {
        // Use a real serialized job shape to exercise backward-compatible defaults too.
        let mut job: HandoffJob = serde_json::from_value(serde_json::json!({
            "id":"test", "revision":1,"created_at":0,
            "request": PrepareRequest { source_agent:AgentKind::Codex, target_agent:AgentKind::Kimi, request_id:"test".into(), cwd:"/tmp".into(), source_session_id:"source".into(), goal:String::new(), target_model:None },
            "state":"needs_interaction", "target":{"agent":"kimi","executable":"/kimi","version":"2.1.1","automatic_delivery":false}
        })).unwrap();
        for (kind, message, step) in [
            (
                FallbackKind::NotReady,
                "Kimi still starting",
                "Wait, then ⌘V in Kimi",
            ),
            (
                FallbackKind::Trust,
                "Approve folder trust in Kimi first",
                "Tap Trust this folder, then ⌘V",
            ),
            (
                FallbackKind::PastePending,
                "Paste the instruction in Kimi",
                "Paste in Kimi — ⌘V",
            ),
            (
                FallbackKind::SubmitUncertain,
                "Could not verify send — check Kimi",
                "Paste again if empty — ⌘V",
            ),
            (
                FallbackKind::LaunchTimeout,
                "Target did not start in time",
                "Open target Tab or send again",
            ),
            (
                FallbackKind::ArgvBootstrap,
                "Target failed to start — see target Tab",
                "Fix login/network in target, then ⌘V",
            ),
            (
                FallbackKind::TargetDead,
                "Target process ended",
                "Abandon this handoff",
            ),
        ] {
            job.fallback = Some(HandoffFallbackGuide {
                kind,
                clipboard_copied: true,
                evidence: FallbackEvidence::Idle,
            });
            let guide = Guide::new(&job, false);
            assert_eq!(kind.message(), message);
            assert_eq!(
                guide.steps,
                ["Copied to clipboard", step, "Waiting for paste…"]
            );
            assert!(!guide.ready);
            assert_eq!(
                guide.abandon_primary,
                matches!(
                    kind,
                    FallbackKind::TargetDead
                        | FallbackKind::LaunchTimeout
                        | FallbackKind::ArgvBootstrap
                )
            );
        }
        job.fallback.as_mut().unwrap().kind = FallbackKind::PastePending;
        job.fallback.as_mut().unwrap().evidence = FallbackEvidence::PasteDetected;
        assert_eq!(Guide::new(&job, false).steps[2], "Handoff sent — confirm");
        job.fallback.as_mut().unwrap().clipboard_copied = false;
        assert!(Guide::new(&job, false).steps[0].contains("Copy again"));
        assert_eq!(Guide::new(&job, true).steps[0], "Copied to clipboard");
        for agent in [AgentKind::Codex, AgentKind::Cursor] {
            job.state = HandoffState::NeedsInteraction;
            job.target.agent = agent;
            job.fallback.as_mut().unwrap().kind = FallbackKind::ArgvBootstrap;
            assert!(Guide::new(&job, false).abandon_primary);
            assert!(!Guide::new(&job, false).ready);
            job.fallback.as_mut().unwrap().kind = FallbackKind::PastePending;
            job.state = HandoffState::AwaitingManualDelivery;
            assert!(Guide::new(&job, false).ready);
            assert_eq!(
                Guide::new(&job, false).steps[1],
                format!("Paste in {} — ⌘V", agent.label())
            );
        }
    }
}
