use super::*;
use con_core::handoff::{HandoffRpc, HandoffService};

impl ConWorkspace {
    pub(crate) fn handle_handoff_command(
        &mut self,
        command: HandoffRpc,
        response_tx: oneshot::Sender<ControlResult>,
        cx: &mut Context<Self>,
    ) {
        if !cfg!(target_os = "macos") {
            Self::send_control_result(
                response_tx,
                Err(ControlError::invalid_params(
                    "Agent Handoff currently supports macOS only",
                )),
            );
            return;
        }
        if matches!(command, HandoffRpc::Open) {
            let handle = self.window_handle;
            cx.spawn(async move |this, cx| {
                let result = handle.update(cx, |_, window, cx| {
                    this.update(cx, |workspace, cx| {
                        workspace.open_agent_handoff(&crate::HandoffAgent, window, cx);
                        json!({"opened":workspace.handoff_window.is_some()})
                    })
                });
                let result = result
                    .map_err(|e| ControlError::internal(e.to_string()))
                    .and_then(|v| v.map_err(|e| ControlError::internal(e.to_string())));
                Self::send_control_result(response_tx, result);
            })
            .detach();
            return;
        }
        let HandoffRpc::Start {
            job_id,
            expected_revision,
            tab_index,
            source,
        } = command
        else {
            self.harness.spawn_detached(async move {
                // Same split as the Start gate below: request/state conflicts
                // are client errors, but storage and helper faults from
                // List/Get/Respond/Cancel/Prepare are server errors — a valid
                // request must never surface as invalid params.
                let result = command.execute().await.map_err(|error| {
                    match route::classify_reserve_error(error) {
                        route::StartGateError::Request(error) => {
                            ControlError::invalid_params(error.to_string())
                        }
                        route::StartGateError::Helper(error) => {
                            ControlError::internal(error.to_string())
                        }
                    }
                });
                let _ = response_tx.send(result);
            });
            return;
        };
        let binding = self
            .resolve_control_tab_index(tab_index)
            .and_then(|tab_idx| {
                let resolved = self.resolve_surface_target_for_tab(tab_idx, source)?;
                Ok((self.tabs[tab_idx].summary_id, resolved.terminal.entity_id()))
            });
        let (tab_id, source_id) = match binding {
            Ok(binding) => binding,
            Err(error) => {
                Self::send_control_result(response_tx, Err(error));
                return;
            }
        };
        let task = self.harness.runtime_handle().spawn_blocking(move || {
            // A store that cannot even open is a server-side environment
            // fault, never a bad request.
            let service = HandoffService::new().map_err(route::StartGateError::Helper)?;
            // Same reserve + helper-protocol gate as the UI route: a model
            // job must never reach an older con-cli that would silently
            // launch with the target's default model. A refused job is left
            // in a reviewable launch-error state by the gate.
            let job = route::reserve_start_checked(&service, &job_id, expected_revision)?;
            Ok::<_, route::StartGateError>((service, job))
        });
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|e| route::StartGateError::Helper(anyhow::anyhow!(e.to_string())))
                .and_then(|v| v);
            let response = match result {
                Ok((service, job)) => {
                    let result = handle.update(cx, |_, window, cx| {
                        this.update(cx, |workspace, cx| {
                            workspace.start_handoff_surface(tab_id, source_id, &job, window, cx)
                        })
                    });
                    match result {
                        Ok(Ok(Ok(()))) => Ok(json!({"job":job,"launch_requested":true})),
                        other => {
                            let error =
                                format!("Target surface launch needs reconciliation: {other:?}");
                            let _ = cx
                                .background_executor()
                                .spawn(async move { service.launch_error(&job.id, &error) })
                                .await;
                            Err(ControlError::internal(
                                "Target surface launch failed; inspect the handoff before retrying",
                            ))
                        }
                    }
                }
                Err(error) => Err(match error {
                    // Request/state conflicts (unknown job, stale revision,
                    // wrong state) stay client errors; helper environment
                    // faults (missing/stuck/too-old con-cli, unreadable
                    // store) are server errors — the request itself was
                    // valid, so clients must not treat them as bad params.
                    route::StartGateError::Request(error) => {
                        ControlError::invalid_params(error.to_string())
                    }
                    route::StartGateError::Helper(error) => {
                        ControlError::internal(error.to_string())
                    }
                }),
            };
            Self::send_control_result(response_tx, response);
        })
        .detach();
    }
}
