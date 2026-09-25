use std::sync::atomic::Ordering;

use con_agent::handoff::{active_session_binding, discover_sessions, refresh_installed_agents};
use con_core::handoff::{
    HandoffService, PrepareRequest, new_request_id, validate_target_model_for_agent,
};

use super::*;

impl HandoffDestinationPanel {
    pub fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.observe_job_card(cx);
        self.loading_agents = true;
        let cwd = self.cwd.clone();
        let task = self.runtime.spawn(async move {
            tokio::join!(refresh_installed_agents(), async move {
                tokio::task::spawn_blocking(move || HandoffService::new()?.list(&cwd)).await
            })
        });
        cx.spawn_in(window, async move |this, window| {
            let result = task.await;
            let _ = window.update(|_, cx| {
                let _ = this.update(cx, |panel, cx| {
                    let (agents, jobs) = match result {
                        Ok(value) => value,
                        Err(error) => {
                            panel.loading_agents = false;
                            panel.error = Some(format!("Could not load Agent Handoff: {error}"));
                            cx.notify();
                            return;
                        }
                    };
                    panel.agents = agents;
                    match jobs {
                        Ok(Ok(jobs)) => {
                            panel.jobs = jobs;
                            panel.promote_related_job(cx);
                        }
                        Ok(Err(error)) => panel.error = Some(error.to_string()),
                        Err(error) => panel.error = Some(error.to_string()),
                    }
                    let targets = panel.target_agents();
                    panel.loading_agents = false;
                    log::info!(
                        "handoff: loaded {} agents ({} targets), active_job={}",
                        panel.agents.len(),
                        targets.len(),
                        panel.active_job.is_some()
                    );
                    cx.notify();
                });
            });
        })
        .detach();
        // Session detection is independent of the agent inventory; start it
        // immediately so both probes overlap.
        let source = self.source_agent;
        self.load_sessions(source, window, cx);
    }

    fn load_sessions(&mut self, agent: AgentKind, window: &mut Window, cx: &mut Context<Self>) {
        self.loading_sessions = true;
        self.binding_warning = None;
        self.error = None;
        let cwd = self.cwd.clone();
        let foreground_group = self.source_foreground_group;
        let screen = self.source_terminal.content_lines(200, cx);
        // An empty Kimi banner is not proof that disk history is absent.
        // Disable automatic selection but keep history available for a user pick.
        let session_not_started =
            agent == AgentKind::Kimi && con_agent::handoff::kimi_session_not_started(&screen);
        let task = self.runtime.spawn(async move {
            // Listing sessions (may spawn an app-server) and binding the live
            // session (process inspection) are independent; run them together.
            let (sessions, binding) = tokio::join!(
                discover_sessions(agent, &cwd),
                active_session_binding(agent, foreground_group, &screen, &cwd)
            );
            (sessions, binding)
        });
        cx.spawn_in(window, async move |this, window| {
            let result = task.await;
            let _ = window.update(|_, cx| {
                let _ = this.update(cx, |panel, cx| {
                    panel.loading_sessions = false;
                    let (sessions, binding) = match result {
                        Ok(value) => value,
                        Err(error) => {
                            panel.error = Some(error.to_string());
                            cx.notify();
                            return;
                        }
                    };
                    let binding = match binding {
                        Ok(binding) => {
                            if binding.is_none() && agent == AgentKind::Codex {
                                panel.binding_warning =
                                    Some(binding_check::codex_binding_hint(None).into());
                            }
                            binding
                        }
                        Err(error) => {
                            log::warn!("Cannot bind running {} session: {error:#}", agent.label());
                            panel.binding_warning = Some(
                                if agent == AgentKind::Codex {
                                    binding_check::codex_binding_hint(Some(&error.to_string()))
                                } else {
                                    "Couldn't detect the current session; confirm it manually"
                                }
                                .into(),
                            );
                            None
                        }
                    };
                    if let Some(binding) = &binding
                        && binding.evidence.starts_with("most recently updated")
                    {
                        panel.binding_warning = Some(format!(
                            "Recent {} session — confirm it is current",
                            agent.label()
                        ));
                    }
                    panel.live_source_id = binding.as_ref().map(|value| value.id.clone());
                    panel.live_source_requires_confirmation = binding
                        .as_ref()
                        .is_some_and(|value| value.requires_confirmation);
                    panel.source_not_started =
                        panel.live_source_id.is_none() && session_not_started;
                    panel.single_session_confirmed = false;
                    panel.selected_session_id = None;
                    panel.visible_candidate_limit = view::CANDIDATE_PAGE;
                    match sessions {
                        Ok(sessions) => {
                            panel.sessions = sessions;
                            if let Some(source) = panel
                                .sessions
                                .iter()
                                .find(|s| Some(s.id.as_str()) == panel.live_source_id.as_deref())
                            {
                                panel.error = source.export_warning.clone();
                            }
                            if panel.live_source_id.is_some()
                                && !panel
                                    .sessions
                                    .iter()
                                    .any(|s| Some(s.id.as_str()) == panel.live_source_id.as_deref())
                            {
                                panel.error = Some(format!(
                                    "Running {} session has no exportable history yet",
                                    panel.source_agent.label()
                                ));
                            }
                        }
                        Err(error) => panel.error = Some(error.to_string()),
                    }
                    log::info!(
                        "handoff: sessions loaded, live={:?} requires_confirmation={}",
                        panel.live_source_id,
                        panel.live_source_requires_confirmation
                    );
                    // The binding may be the half that arrived second: a
                    // listed job bound to this Tab's session now promotes
                    // into the current-task card.
                    panel.promote_related_job(cx);
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub(super) fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(destination) = self.selected_destination.clone() else {
            log::info!("handoff: confirm ignored, no destination selected");
            return;
        };
        self.execute(destination, window, cx);
    }

    fn execute(&mut self, destination: Destination, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        log::info!("handoff: execute requested for {destination:?}");
        let Some(source) = self.selected_source().cloned() else {
            self.error = Some("Couldn't identify the current session in this Tab".into());
            cx.notify();
            return;
        };
        if !self.source_confirmed() {
            self.error = Some("Confirm that the displayed session is current".into());
            cx.notify();
            return;
        }
        if source.agent != self.source_agent {
            self.error = Some("Source Agent session does not match this Tab".into());
            cx.notify();
            return;
        }
        let target_agent = match &destination {
            Destination::Existing(tab) => tab.agent,
            Destination::NewTab(agent) => *agent,
        };
        // Only the new-tab path may carry a model, and only an explicitly
        // typed one; re-validate here so an invalid value can never reach a
        // persisted job even if the prepare button gating is bypassed.
        let target_model = self.requested_model(&destination, cx);
        if let Some(model) = &target_model
            && let Err(error) = validate_target_model_for_agent(target_agent, model)
        {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        let cwd = self.cwd.clone();
        let discover_cwd = cwd.clone();
        let foreground_group = self.source_foreground_group;
        let bound_id = self.live_source_id.clone();
        let confirmed = self.single_session_confirmed;
        let agent = self.source_agent;
        let selected_id = source.id.clone();
        let screen = self.source_terminal.content_lines(200, cx);
        let request_id = new_request_id();
        self.preparing_request_id = Some(request_id.clone());
        self.sent_job_id = None;
        let closed = self.closed.clone();
        let task = self.runtime.spawn(async move {
            if let Some(bound_id) = &bound_id {
                binding_check::check_binding(
                    active_session_binding(agent, foreground_group, &screen, &cwd).await,
                    bound_id,
                    confirmed,
                )
                .map_err(anyhow::Error::msg)?;
            } else {
                // A manually picked (or single-fallback) session ID must still
                // be exported by the source adapter before preparing.
                let sessions = discover_sessions(agent, &discover_cwd).await?;
                anyhow::ensure!(
                    sessions.iter().any(|session| session.id == selected_id),
                    "Source session no longer exists; reopen Handoff"
                );
            }
            let service = HandoffService::new()?;
            let job = service
                .prepare(PrepareRequest {
                    source_agent: source.agent,
                    target_agent,
                    request_id,
                    cwd,
                    source_session_id: source.id,
                    goal: String::new(),
                    target_model,
                })
                .await?;
            // The window may have closed while preparing; never leave a
            // unused Prepared job after it is gone.
            if closed.load(Ordering::SeqCst) {
                let _ = service.cancel(&job.id, job.revision);
                anyhow::bail!("Handoff panel closed");
            }
            // A bundle read failure after prepare must not leak the Prepared
            // job either — cancel it before reporting the error. The filtered
            // context is no longer rendered for review (prepare dispatches
            // straight into the route), but the read still proves the bundle
            // is intact before anything is delivered.
            let bundle = tokio::task::spawn_blocking({
                let service = service.clone();
                let job_id = job.id.clone();
                move || service.bundle(&job_id)
            })
            .await;
            match bundle {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    let _ = service.cancel(&job.id, job.revision);
                    return Err(error);
                }
                Err(error) => {
                    let _ = service.cancel(&job.id, job.revision);
                    return Err(anyhow::Error::from(error));
                }
            }
            Ok::<_, anyhow::Error>(job)
        });
        cx.spawn_in(window, async move |this, window| {
            let result = task
                .await
                .map_err(anyhow::Error::from)
                .and_then(|value| value);
            let _ = window.update(|window, cx| {
                let _ = this.update(cx, |panel, cx| {
                    match result {
                        // Prepare is immediately followed by the route: there
                        // is no separate review stage, so every prepared job
                        // leaves the panel as a dispatched handoff.
                        Ok(job) => panel.finish_prepare(job, destination, window, cx),
                        Err(error) => {
                            panel.busy = false;
                            panel.preparing_request_id = None;
                            panel.error = Some(error.to_string());
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
    }

    /// Prepare finished. Delivery starts immediately — there is no review
    /// stage between prepare and send — so before dispatching, the live
    /// session is re-bound with a fresh screen capture: the evidence
    /// collected before prepare is stale by now, and a drifted source
    /// session must not be handed off. Any doubt cancels the Prepared job
    /// and refuses.
    fn finish_prepare(
        &mut self,
        job: HandoffJob,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bound_id) = self.live_source_id.clone() else {
            self.preparing_request_id = None;
            self.dispatch_prepared(job, destination, cx);
            cx.notify();
            return;
        };
        let confirmed = self.single_session_confirmed;
        let job_id = job.id.clone();
        let screen = self.source_terminal.content_lines(200, cx);
        let agent = self.source_agent;
        let group = self.source_foreground_group;
        let runtime = self.runtime.clone();
        let cwd = self.cwd.clone();
        let task = self
            .runtime
            .spawn(async move { active_session_binding(agent, group, &screen, &cwd).await });
        cx.spawn_in(window, async move |this, window| {
            let binding = task
                .await
                .map_err(anyhow::Error::from)
                .and_then(|value| value);
            let checked = super::binding_check::check_binding(binding, &bound_id, confirmed);
            let still_bound = checked.is_ok();
            let shown = window.update(|_, cx| {
                this.update(cx, |panel, cx| {
                    panel.preparing_request_id = None;
                    if still_bound {
                        panel.dispatch_prepared(job, destination, cx);
                    } else {
                        panel.busy = false;
                        panel.error = checked.err().map(str::to_owned);
                    }
                    cx.notify();
                })
            });
            if !still_bound || !matches!(shown, Ok(Ok(()))) {
                // The panel can no longer dispatch this job; never leave it
                // an unused Prepared job.
                super::cancel_prepared_handoff(&runtime, job_id);
            }
        })
        .detach();
    }
}
