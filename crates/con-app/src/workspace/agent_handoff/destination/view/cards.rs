use con_core::handoff::{HandoffResponse, HandoffState};
use gpui_component::Sizable;

use super::*;

impl HandoffDestinationPanel {
    pub(super) fn render_job(
        &self,
        job: &HandoffJob,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut card = card(palette).child(job_pair_row(job, palette));
        if let Some(error) = &job.error {
            card = card.child(
                card_row()
                    .text_sm()
                    .text_color(palette.danger)
                    .child(error.clone()),
            );
        }
        let manual = matches!(
            job.state,
            HandoffState::NeedsInteraction | HandoffState::AwaitingManualDelivery
        );
        let guide = fallback::Guide::new(job, self.copied_instructions.contains(&job.id));
        if manual {
            card = card.child(guide.render(palette));
        }
        if job.target.agent == con_agent::handoff::AgentKind::Kimi {
            if job.state == HandoffState::Delivering {
                card = card.child(card_row().text_sm().child("Sending instruction to Kimi…"));
            } else if job.receipt.as_deref() == Some("pty_injection_submit_observed") {
                card = card.child(card_row().text_sm().child("Instruction sent to Kimi"));
            }
        }
        if job.state == HandoffState::NeedsInteraction {
            card = card.child(card_row().text_sm().child(if job.target_pid.is_some() {
                "Target started; abandoning does not stop it."
            } else {
                "Target launch unconfirmed."
            }));
        }
        let mut actions = div().flex().flex_col().gap_2().px(px(10.0)).pb(px(10.0));
        let response = match job.state {
            HandoffState::NeedsInteraction if !job.can_abandon() => {
                Some(("Target stopped", Some(HandoffResponse::ConfirmStopped)))
            }
            HandoffState::NeedsInteraction if !guide.abandon_primary => Some((
                if guide.ready {
                    "Handoff sent — confirm"
                } else {
                    "Waiting for paste…"
                },
                Some(HandoffResponse::ConfirmReceived),
            )),
            _ if job.can_abandon() => Some(("Abandon handoff", Some(HandoffResponse::Abandon))),
            // Recovery entries for launches whose outcome is uncertain: the
            // user inspects the target, then cancels into NeedsInteraction
            // (never an automatic retry or resend).
            HandoffState::LaunchPending | HandoffState::StartingTarget => {
                Some(("Cancel launch", None))
            }
            HandoffState::Delivering if job.existing_target_tab_id.is_some() => Some((
                // Recovery only: the app died between persisting Delivering
                // and recording the observed write. New deliveries go
                // straight to Active on the observed write.
                "Handoff sent — confirm",
                Some(HandoffResponse::ConfirmReceived),
            )),
            HandoffState::Delivering => Some(("Cancel handoff…", None)),
            HandoffState::AwaitingConfirmation => Some((
                // Legacy jobs persisted before the send-is-confirmation
                // change still complete through this explicit receipt; new
                // flows never enter this state.
                "Confirm continuation",
                Some(HandoffResponse::ConfirmReceived),
            )),
            HandoffState::AwaitingManualDelivery => {
                Some(("I sent the instruction", Some(HandoffResponse::ConfirmSent)))
            }
            HandoffState::NeedsInteraction => {
                Some(("Target stopped", Some(HandoffResponse::ConfirmStopped)))
            }
            HandoffState::Active | HandoffState::Prepared => Some(("Cancel handoff…", None)),
            _ => None,
        };
        if let Some((label, response)) = response {
            let job_id = job.id.clone();
            actions = actions.child(
                Button::new("handoff-job-action")
                    .primary()
                    .label(label)
                    .disabled(
                        self.busy
                            || (job.state == HandoffState::NeedsInteraction
                                && job.can_abandon()
                                && !guide.abandon_primary
                                && !guide.ready),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.respond(job_id.clone(), response, window, cx)
                    })),
            );
        }
        let context = con_paths::app_data_dir()
            .join("handoffs")
            .join(&job.id)
            .join("context.md");
        let evidence = con_paths::app_data_dir()
            .join("handoffs")
            .join(&job.id)
            .join("evidence.json");
        let mut secondary = div().flex().flex_wrap().items_center().gap_2();
        if job.state == HandoffState::NeedsInteraction
            && job.can_abandon()
            && (job.target_pid.is_some() || job.existing_target_tab_id.is_some())
        {
            let id = job.id.clone();
            secondary = secondary.child(
                Button::new("handoff-confirm-stopped")
                    .ghost()
                    .small()
                    .label("Target stopped")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.respond(
                            id.clone(),
                            Some(HandoffResponse::ConfirmStopped),
                            window,
                            cx,
                        );
                    })),
            );
        }
        if job.state == HandoffState::Delivering && job.existing_target_tab_id.is_some() {
            // The delivery outcome is uncertain; instead of confirming, the
            // user may stop the target and cancel (cancel → NeedsInteraction).
            let job_id = job.id.clone();
            secondary = secondary.child(
                Button::new("handoff-job-cancel")
                    .ghost()
                    .small()
                    .label("Cancel handoff…")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.respond(job_id.clone(), None, window, cx)
                    })),
            );
        }
        if manual && job.can_abandon() && !guide.abandon_primary {
            let id = job.id.clone();
            secondary = secondary.child(
                Button::new("handoff-abandon")
                    .ghost()
                    .small()
                    .label("Abandon handoff")
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.respond(id.clone(), Some(HandoffResponse::Abandon), window, cx)
                    })),
            );
        }
        secondary = secondary.child(
            Button::new("handoff-context")
                .ghost()
                .small()
                .label("View handoff context")
                .on_click(move |_, _, cx| cx.reveal_path(&context)),
        );
        // The removed send-review page used to expose both artifacts; keep
        // the evidence reachable from the job card instead of dropping it.
        secondary = secondary.child(
            Button::new("handoff-evidence")
                .ghost()
                .small()
                .label("View evidence")
                .on_click(move |_, _, cx| cx.reveal_path(&evidence)),
        );
        if manual && self.instruction_available.as_deref() == Some(&job.id) {
            let id = job.id.clone();
            let instruction = con_core::handoff::instruction(&job.id);
            secondary = secondary.child(
                Button::new("handoff-copy-instruction")
                    .ghost()
                    .small()
                    .label("Copy again")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(instruction.clone()));
                        this.copied_instructions.insert(id.clone());
                        cx.notify();
                    })),
            );
        }
        card.child(actions.child(secondary))
    }
}
