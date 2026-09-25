use super::*;

impl HandoffService {
    pub fn reserve_start(&self, id: &str, revision: u64) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure_supported_agents(&job.request)?;
            ensure!(
                job.target.agent != con_agent::handoff::AgentKind::Unknown,
                "Unsupported target Agent"
            );
            ensure!(
                job.state == HandoffState::Prepared,
                "Handoff has already been started"
            );
            self.verify_snapshot(id)?;
            job.state = HandoffState::LaunchPending;
            Ok(())
        })
    }

    /// Reserve the one allowed delivery into an already-running Agent tab.
    /// The caller must validate the tab, terminal, cwd, foreground process,
    /// Agent identity and source history before this call, then validate the
    /// destination again immediately before writing to its PTY. Once this
    /// returns, delivery is uncertain even if the caller observes a write
    /// error: the instruction must never be replayed automatically.
    pub fn begin_existing_delivery(
        &self,
        id: &str,
        revision: u64,
        tab_id: u64,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure_supported_agents(&job.request)?;
            ensure!(
                job.target.agent != con_agent::handoff::AgentKind::Unknown,
                "Unsupported target Agent"
            );
            ensure!(
                job.state == HandoffState::Prepared,
                "Handoff has already been started"
            );
            // The route's per-tab guard shares the state-write lock: two
            // concurrent sends cannot both reserve the same PTY. Other tabs
            // and completed/uncertain jobs never block a new delivery.
            ensure!(
                !self.store.jobs()?.iter().any(|other| {
                    other.id != id
                        && other.existing_target_tab_id == Some(tab_id)
                        && matches!(
                            other.state,
                            HandoffState::Delivering | HandoffState::LaunchPending
                        )
                }),
                "Target tab is receiving a handoff; try again shortly"
            );
            self.stage_and_verify(&self.verify_snapshot(id)?)?;
            job.existing_target_tab_id = Some(tab_id);
            // Persist the destination and uncertainty before any PTY write.
            job.state = HandoffState::Delivering;
            Ok(())
        })
    }

    /// An observed PTY write completes the delivery: the Send click that
    /// prepared this job was already the explicit confirmation, so the job
    /// goes straight to Active without a second confirmation step. The
    /// receipt records that delivery was attempted and observed, not that
    /// the target understood the instruction.
    pub fn record_existing_delivery(
        &self,
        id: &str,
        revision: u64,
        pid: u32,
        start_secs: u64,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure!(
                job.state == HandoffState::Delivering && job.existing_target_tab_id.is_some(),
                "Existing target delivery is not pending"
            );
            ensure!(
                pid > 0 && start_secs > 0,
                "Target process identity is incomplete"
            );
            ensure!(
                job.target.agent != con_agent::handoff::AgentKind::Kimi,
                "Kimi requires observed submission, not only a PTY write"
            );
            job.target_pid = Some(pid);
            job.target_process_start = Some(start_secs);
            job.state = HandoffState::Active;
            job.receipt = Some("delivery_write_observed".into());
            Ok(())
        })
    }

    /// Expire a launch which never reached its visible helper; never race a live helper.
    pub fn expire_pending_launch(&self, id: &str, revision: u64) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure!(
                job.state == HandoffState::LaunchPending,
                "Launch has progressed"
            );
            let _guard = self.store.launch_guard(id)?;
            job.state = HandoffState::NeedsInteraction;
            job.set_fallback(FallbackKind::LaunchTimeout, false);
            Ok(())
        })
    }

    /// Called only by the visible con-cli helper, immediately before creating a target.
    pub fn begin_launch(
        &self,
        id: &str,
        revision: u64,
        target: &TargetCapabilities,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure_supported_agents(&job.request)?;
            ensure!(
                job.target.agent != con_agent::handoff::AgentKind::Unknown,
                "Unsupported target Agent"
            );
            ensure!(
                job.state == HandoffState::LaunchPending,
                "Cannot repeat or resume an uncertain launch"
            );
            ensure!(
                job.target.agent == target.agent
                    && job.target.executable == target.executable
                    && job.target.version == target.version
                    && job.target.automatic_delivery == target.automatic_delivery,
                "Target agent changed after prepare; prepare a new handoff"
            );
            self.stage_and_verify(&self.verify_snapshot(id)?)?;
            job.state = HandoffState::StartingTarget;
            Ok(())
        })
    }

    pub fn record_target(&self, id: &str, revision: u64, target_id: &str) -> Result<HandoffJob> {
        self.record_launch(id, revision, Some(target_id))
    }

    pub fn record_launch(
        &self,
        id: &str,
        revision: u64,
        target_id: Option<&str>,
    ) -> Result<HandoffJob> {
        if let Some(target_id) = target_id {
            ensure!(
                !target_id.is_empty()
                    && target_id.len() <= 256
                    && !target_id.starts_with('-')
                    && !target_id.chars().any(char::is_control),
                "Invalid native target session ID"
            );
        }
        self.update(id, revision, |job| {
            ensure!(
                job.state == HandoffState::StartingTarget && job.target_session_id.is_none(),
                "Target creation is not pending"
            );
            job.target_session_id = target_id.map(str::to_owned);
            // Persist before any prompt can enter the target process.
            job.state = HandoffState::Delivering;
            Ok(())
        })
    }

    pub fn record_spawn(
        &self,
        id: &str,
        revision: u64,
        pid: u32,
        start_secs: Option<u64>,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure!(
                job.state == HandoffState::Delivering,
                "Delivery state changed"
            );
            ensure!(pid > 0, "Target process identity is incomplete");
            job.target_pid = Some(pid);
            job.target_process_start = start_secs.filter(|start| *start > 0);
            job.state = if job.target.agent == con_agent::handoff::AgentKind::Kimi {
                // Spawn alone is not delivery: Con must observe TUI submission.
                HandoffState::Delivering
            } else if job.target.automatic_delivery {
                // The initial prompt rode the target's own argv, and the
                // user's Send click was the explicit confirmation: spawning
                // the target completes delivery without a second confirmation.
                job.receipt = Some("automatic_delivery_spawn_observed".into());
                HandoffState::Active
            } else {
                HandoffState::AwaitingManualDelivery
            };
            Ok(())
        })
    }

    /// Called by Con only after fresh screen evidence confirms Kimi submission.
    pub fn record_kimi_submit(
        &self,
        id: &str,
        revision: u64,
        pid: u32,
        start_secs: u64,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure!(
                job.target.agent == con_agent::handoff::AgentKind::Kimi
                    && job.state == HandoffState::Delivering,
                "Kimi delivery is not pending"
            );
            job.state = HandoffState::Active;
            ensure!(
                pid > 0 && start_secs > 0,
                "Target process identity is incomplete"
            );
            job.target_pid = Some(pid);
            job.target_process_start = Some(start_secs);
            job.receipt = Some("pty_injection_submit_observed".into());
            Ok(())
        })
    }

    /// A timed-out observer must not overwrite a newer user/helper decision.
    pub fn record_kimi_failure(
        &self,
        id: &str,
        revision: u64,
        message: &str,
    ) -> Result<HandoffJob> {
        self.record_kimi_fallback(id, revision, message, None)
    }

    pub fn launch_error(&self, id: &str, message: &str) -> Result<HandoffJob> {
        let job = self.get(id)?;
        self.update(id, job.revision, |job| {
            ensure!(!job.state.is_terminal(), "Handoff is finished");
            // Keep detailed diagnostics in the helper terminal; the card has a short guide.
            log::warn!("handoff launch: {}", sanitize(message));
            let kind = if job.target.agent == con_agent::handoff::AgentKind::Kimi
                && message.to_lowercase().contains("trust")
            {
                FallbackKind::Trust
            } else {
                FallbackKind::ArgvBootstrap
            };
            job.set_fallback(kind, false);
            // Errors do not prove a child never started, nor that background work stopped.
            job.state = HandoffState::NeedsInteraction;
            Ok(())
        })
    }

    /// Cancel this job because the recorded target process is gone.
    /// Uncertain launches (`Prepared` through `Delivering`) stay for the user:
    /// a missing pid there means the outcome is unknown, not that the target
    /// exited. The launch guard refuses this while the helper still holds it.
    pub fn cancel_absent_target(&self, id: &str, revision: u64) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            ensure!(
                job.target.agent != con_agent::handoff::AgentKind::Unknown,
                "Unsupported target Agent; cancel and confirm it stopped"
            );
            ensure!(
                matches!(
                    job.state,
                    HandoffState::Active
                        | HandoffState::AwaitingConfirmation
                        | HandoffState::AwaitingManualDelivery
                        | HandoffState::NeedsInteraction
                ),
                "Only a delivered handoff with a missing target can be cancelled"
            );
            let _guard = self.store.launch_guard(id)?;
            job.state = HandoffState::Cancelled;
            job.receipt = Some("target_process_absent".into());
            Ok(())
        })
    }

    pub fn cancel(&self, id: &str, revision: u64) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            if job.state == HandoffState::Delivering && job.can_abandon() {
                job.state = HandoffState::Cancelled;
                job.receipt = Some("user_abandoned".into());
                return Ok(());
            }
            job.state = match job.state {
                HandoffState::Prepared | HandoffState::Cancelled | HandoffState::Failed => {
                    HandoffState::Cancelled
                }
                _ => HandoffState::NeedsInteraction,
            };
            Ok(())
        })
    }

    pub fn respond(
        &self,
        id: &str,
        revision: u64,
        response: HandoffResponse,
    ) -> Result<HandoffJob> {
        self.update(id, revision, |job| {
            match response {
                HandoffResponse::Abandon => {
                    ensure!(
                        job.can_abandon(),
                        "Confirmed delivery requires target-stop confirmation"
                    );
                    job.state = HandoffState::Cancelled;
                    job.receipt = Some("user_abandoned".into());
                }

                HandoffResponse::ConfirmSent => {
                    ensure!(
                        job.state == HandoffState::AwaitingManualDelivery,
                        "Manual delivery is not pending"
                    );
                    // "I sent the instruction" is the user's explicit final
                    // confirmation for a manual delivery; no second receipt
                    // confirmation follows.
                    job.state = HandoffState::Active;
                    job.receipt = Some("user_confirmed_sent".into());
                }
                HandoffResponse::ConfirmReceived => {
                    // Legacy jobs persisted in AwaitingConfirmation (and jobs
                    // stuck in Delivering after a crash) still complete through
                    // this explicit receipt; new flows no longer enter
                    // AwaitingConfirmation.
                    ensure!(
                        job.state == HandoffState::AwaitingConfirmation
                            || (job.state == HandoffState::NeedsInteraction
                                && job.fallback_confirmation_ready())
                            || (job.state == HandoffState::Delivering
                                && job.existing_target_tab_id.is_some()),
                        "Receipt confirmation is not pending"
                    );
                    job.error = None;
                    // Some native TUIs expose their ID only after launch. The explicit
                    // receipt confirms the visible handoff UUID, not an inferred native ID.
                    job.state = HandoffState::Active;
                    job.receipt = Some(
                        if job.target_session_id.is_some() {
                            "user_confirmed_received"
                        } else {
                            "user_confirmed_handoff_native_id_unobserved"
                        }
                        .into(),
                    );
                }
                HandoffResponse::ConfirmStopped => {
                    ensure!(
                        job.state == HandoffState::NeedsInteraction,
                        "Request cancellation before confirming stopped"
                    );
                    let _guard = self.store.launch_guard(id)?;
                    // No process signal is inferred from this response; the user stops native TUI work.
                    job.state = HandoffState::Cancelled;
                }
            }
            Ok(())
        })
    }
}
