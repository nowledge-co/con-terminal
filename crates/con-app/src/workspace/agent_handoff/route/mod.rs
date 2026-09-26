mod reserve;
pub(super) use reserve::{
    StartGateError, cancel_prepared_handoff, classify_reserve_error, reserve_start_checked,
};

mod protocol;

use super::destination::Destination;
use super::*;

impl ConWorkspace {
    /// Delivery runs in three phases so disk, Git and protocol work stay off
    /// the GPUI thread: (1) re-check the live source/target identity here,
    /// (2) re-bind the native source session and run the handoff service
    /// (snapshot checks, history re-validation, state transactions) on the
    /// shared Tokio runtime, (3) back on the GPUI thread re-check the live
    /// identity once more and perform the terminal actions. Returns only the
    /// synchronous phase-1 result; later failures are reported to the panel.
    pub(super) fn route_handoff(
        &mut self,
        binding: SourceAgentTab,
        route: ExecuteHandoff,
        panel: &Entity<HandoffDestinationPanel>,
        dialog: &AnyWindowHandle,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let source_index = self.check_source_binding(&binding, &route.job, cx)?;
        // Fresh screen evidence for the send-time session re-bind; the
        // capture from panel open/prepare time is stale by now.
        let screen = self.tabs[source_index]
            .pane_tree
            .surface_terminals()
            .into_iter()
            .find(|(_, _, terminal)| terminal.entity_id() == binding.terminal_id)
            .map(|(_, _, terminal)| terminal.content_lines(200, cx))
            .ok_or_else(|| anyhow::anyhow!("Source terminal was replaced"))?;
        let runtime = self.harness.runtime_handle();
        let handle = self.window_handle;
        let job = route.job;
        let live_binding = route.live_source_binding;
        let source_confirmed = route.source_session_confirmed;
        let destination = route.destination;
        let panel = panel.clone();
        let dialog = *dialog;
        match destination {
            Destination::NewTab(agent) => {
                anyhow::ensure!(agent == job.target.agent, "Selected target Agent changed");
                let task = runtime.spawn({
                    let job = job.clone();
                    async move {
                        revalidate_source_session(
                            binding,
                            &job,
                            live_binding,
                            source_confirmed,
                            screen,
                        )
                        .await?;
                        tokio::task::spawn_blocking(move || {
                            let service = con_core::handoff::HandoffService::new()?;
                            let pending = reserve_start_checked(&service, &job.id, job.revision)?;
                            Ok::<_, anyhow::Error>((service, pending))
                        })
                        .await?
                    }
                });
                cx.spawn(async move |this, cx| {
                    let result = task
                        .await
                        .map_err(|error| anyhow::anyhow!(error.to_string()))
                        .and_then(|value| value);
                    let (service, pending) = match result {
                        Ok(value) => value,
                        Err(error) => {
                            panel.update(cx, |panel, cx| panel.report_error(error.to_string(), cx));
                            cancel_prepared_handoff(&runtime, job.id.clone());
                            return;
                        }
                    };
                    let launched = handle.update(cx, |_, window, cx| {
                        this.update(cx, |workspace, cx| {
                            let source_index =
                                workspace.check_source_binding(&binding, &pending, cx)?;
                            workspace.activate_tab(source_index, window, cx);
                            workspace.start_handoff_tab(&pending, window, cx)
                        })
                    });
                    match launched {
                        Ok(Ok(Ok(()))) => {
                            let _ = dialog.update(cx, |_, window, _| window.remove_window());
                        }
                        other => {
                            // Persist the launch failure BEFORE the panel
                            // refreshes the job: reading first could cache a
                            // stale LaunchPending revision whose expiry task
                            // then conflicts with the NeedsInteraction write,
                            // leaving the card stale until the window reopens.
                            let error = format!("Handoff needs review: {other:?}");
                            let persisted = runtime.spawn_blocking({
                                let error = error.clone();
                                move || service.launch_error(&pending.id, &error)
                            });
                            let job = match persisted.await {
                                Ok(Ok(job)) => Some(job),
                                Ok(Err(error)) => {
                                    log::warn!("handoff: could not persist launch error: {error}");
                                    None
                                }
                                Err(error) => {
                                    log::warn!("handoff: launch error task failed: {error}");
                                    None
                                }
                            };
                            panel.update(cx, |panel, cx| {
                                panel.report_launch_failure(error, job, cx)
                            });
                        }
                    }
                })
                .detach();
            }
            Destination::Existing(target) => {
                self.check_existing_target(source_index, &target, &job, cx)?;
                let task = runtime.spawn({
                    let job_id = job.id.clone();
                    let revision = job.revision;
                    let tab_id = target.tab_id;
                    let job = job.clone();
                    async move {
                        revalidate_source_session(
                            binding,
                            &job,
                            live_binding,
                            source_confirmed,
                            screen,
                        )
                        .await?;
                        let service =
                            tokio::task::spawn_blocking(con_core::handoff::HandoffService::new)
                                .await??;
                        // The source may have produced another turn after the
                        // preview without touching the worktree; a stale bundle
                        // must not be delivered.
                        service.validate_source(&job_id).await?;
                        let delivery = tokio::task::spawn_blocking({
                            let service = service.clone();
                            let job_id = job_id.clone();
                            move || {
                                protocol::check_helper_protocol()?;
                                // Atomically reject another in-flight delivery
                                // to this tab before this route can write its PTY.
                                service.begin_existing_delivery(&job_id, revision, tab_id)
                            }
                        })
                        .await??;
                        Ok::<_, anyhow::Error>((service, delivery))
                    }
                });
                cx.spawn(async move |this, cx| {
                    let result = task
                        .await
                        .map_err(|error| anyhow::anyhow!(error.to_string()))
                        .and_then(|value| value);
                    let (service, delivery) = match result {
                        Ok(value) => value,
                        Err(error) => {
                            panel
                                .update(cx, |panel, cx| panel.report_error(error.to_string(), cx));
                            // Cancels unused Prepared jobs; a persisted Delivering
                            // intent is left for the user to reconcile.
                            cancel_prepared_handoff(&runtime, job.id.clone());
                            return;
                        }
                    };
                    if job.target.agent == con_agent::handoff::AgentKind::Kimi {
                        let started = handle.update(cx, |_, window, cx| {
                            this.update(cx, |workspace, cx| -> anyhow::Result<()> {
                                let source = workspace.check_source_binding(&binding, &job, cx)?;
                                let index = workspace.check_existing_target(source, &target, &job, cx)?;
                                let terminal = workspace.tabs[index].pane_tree.try_focused_terminal()
                                    .cloned().ok_or_else(|| anyhow::anyhow!("Target terminal unavailable"))?;
                                let mut delivery = delivery.clone();
                                let pid = u32::try_from(target.foreground_group)?;
                                delivery.target_pid = Some(pid);
                                delivery.target_process_start = con_agent::handoff::process_start_secs(pid as i32);
                                workspace.activate_tab(index, window, cx);
                                workspace.deliver_kimi_via_pty(delivery, target.tab_id, terminal, cx);
                                Ok(())
                            })
                        });
                        match started {
                            Ok(Ok(Ok(()))) => {
                                let _ = dialog.update(cx, |_, window, _| window.remove_window());
                            }
                            other => {
                                // The delivery intent is already persisted as
                                // Delivering; never replay automatically.
                                // Persist the launch error AND report it back
                                // to the panel — otherwise the dialog stays
                                // busy on "Sending handoff…" and the user
                                // never sees the failure.
                                let error = match &other {
                                    Ok(Ok(Err(inner))) => {
                                        format!("Kimi delivery needs review: {inner}")
                                    }
                                    _ => format!("Kimi delivery needs review: {other:?}"),
                                };
                                let persisted = runtime
                                    .spawn_blocking({
                                        let job_id = job.id.clone();
                                        let message = error.clone();
                                        move || service.launch_error(&job_id, &message)
                                    })
                                    .await
                                    .map_err(|error| anyhow::anyhow!(error.to_string()))
                                    .and_then(|value| value);
                                match persisted {
                                    Ok(updated) => panel.update(cx, |panel, cx| {
                                        panel.report_launch_failure(error, Some(updated), cx)
                                    }),
                                    Err(write_error) => panel.update(cx, |panel, cx| {
                                        panel.report_launch_failure(
                                            format!(
                                                "{error}; could not record it: {write_error}"
                                            ),
                                            None,
                                            cx,
                                        )
                                    }),
                                };
                            }
                        }
                        return;
                    }
                    let written = handle.update(cx, |_, window, cx| {
                        this.update(cx, |workspace, cx| {
                            let source_index =
                                workspace.check_source_binding(&binding, &job, cx)?;
                            let index =
                                workspace.check_existing_target(source_index, &target, &job, cx)?;
                            let terminal = workspace.tabs[index]
                                .pane_tree
                                .try_focused_terminal()
                                .cloned()
                                .ok_or_else(|| anyhow::anyhow!("Target terminal unavailable"))?;
                            workspace.activate_tab(index, window, cx);
                            // Kimi uses the shared bracketed-paste + submit observer above.
                            // PTY acceptance is not a native-TUI receipt: raw-mode key
                            // handling (notably Enter vs newline) differs by Agent.
                            // A write receipt is not proof of submission; desktop acceptance
                            // must cover Codex/Kimi as well as Cursor.
                            let instruction =
                                format!("{}\n", con_core::handoff::instruction(&job.id));
                            anyhow::ensure!(
                                terminal.write_observed(instruction.as_bytes(), cx),
                                "Delivery was not observed; the handoff stays in Delivering — inspect the target Tab, then confirm or cancel it from Handoff"
                            );
                            window.activate_window();
                            let pid = u32::try_from(target.foreground_group).map_err(|_| {
                                anyhow::anyhow!("Target process identity is incomplete")
                            })?;
                            let start = con_agent::handoff::process_start_secs(pid as i32)
                                .ok_or_else(|| {
                                    anyhow::anyhow!("Target process identity is incomplete")
                                })?;
                            Ok::<_, anyhow::Error>((pid, start))
                        })
                    });
                    match written {
                        Ok(Ok(Ok((pid, start)))) => {
                            // The write was observed; record the attempted
                            // delivery before closing, so a persistence
                            // failure surfaces instead of silently leaving the
                            // job in Delivering. Never replayed automatically.
                            let recorded = runtime
                                .spawn_blocking(move || {
                                    service.record_existing_delivery(
                                        &job.id,
                                        delivery.revision,
                                        pid,
                                        start,
                                    )
                                })
                                .await
                                .map_err(|error| anyhow::anyhow!(error.to_string()))
                                .and_then(|value| value);
                            match recorded {
                                Ok(_) => {
                                    let _ = dialog.update(cx, |_, window, _| window.remove_window());
                                }
                                Err(error) => {
                                    panel.update(cx, |panel, cx| {
                                        panel.report_error(
                                            format!(
                                                "Could not record the delivery: {error}. The handoff stays in Delivering — inspect the target Tab, then confirm or cancel it from Handoff"
                                            ),
                                            cx,
                                        )
                                    });
                                }
                            }
                        }
                        other => {
                            // The delivery intent is already persisted as
                            // Delivering; never replay automatically — the user
                            // reconciles the visible target instead.
                            let error = format!("Handoff needs review: {other:?}");
                            panel.update(cx, |panel, cx| panel.report_error(error, cx));
                        }
                    }
                })
                .detach();
            }
        }
        Ok(())
    }
}

/// Re-bind the native source session at send time: the process-level identity
/// checks cannot see a TUI that switched to another session inside the same
/// Agent process. With live binding evidence, the job's source session must
/// still be the one bound to the foreground Agent; without it (a manual pick),
/// the session must still be discoverable by the adapter. Any doubt refuses
/// the send and asks the user to reopen the panel.
async fn revalidate_source_session(
    binding: SourceAgentTab,
    job: &con_core::handoff::HandoffJob,
    live_binding: Option<String>,
    source_confirmed: bool,
    screen: Vec<String>,
) -> anyhow::Result<()> {
    if let Some(live_id) = live_binding {
        anyhow::ensure!(
            live_id == job.request.source_session_id,
            "Source session does not match the prepared handoff; reopen Handoff"
        );
        let current = con_agent::handoff::active_session_binding(
            binding.agent,
            binding.foreground_group,
            &screen,
            &job.request.cwd,
        )
        .await?;
        anyhow::ensure!(
            !current
                .as_ref()
                .is_some_and(|value| value.requires_confirmation)
                || source_confirmed,
            "Source session needs confirmation; reopen Handoff"
        );
        anyhow::ensure!(
            current.as_ref().map(|value| value.id.as_str()) == Some(live_id.as_str()),
            "Source Agent session changed; reopen Handoff"
        );
    } else {
        let sessions =
            con_agent::handoff::discover_sessions(binding.agent, &job.request.cwd).await?;
        anyhow::ensure!(
            sessions
                .iter()
                .any(|session| session.id == job.request.source_session_id),
            "Source session no longer exists; reopen Handoff"
        );
    }
    Ok(())
}
