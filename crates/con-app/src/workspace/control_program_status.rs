use super::*;
use con_core::program_observation::{ReportedRecord, ReportedState};

impl ConWorkspace {
    pub(super) fn control_program_status_value(
        &self,
        tab_idx: usize,
        resolved: &ResolvedSurfaceTarget,
        status: &str,
        record: Option<&ReportedRecord>,
        cx: &App,
    ) -> ControlResult {
        let observation = self
            .explicit_program_observation(&resolved.terminal)
            .ok_or_else(|| {
                ControlError::invalid_params(
                    "Terminal status is not initialized; wait for terminal readiness first",
                )
            })?;
        Ok(json!({
            "status": status,
            "tab_index": tab_idx + 1,
            "pane_id": resolved.pane_id,
            "surface_id": resolved.surface_id,
            "is_alive": resolved.terminal.is_alive(cx),
            "program_status": observation,
            "record": record,
        }))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_control_program_wait(
        &mut self,
        tab_index: Option<usize>,
        target: con_core::SurfaceTarget,
        after: String,
        record_id: String,
        states: Vec<ReportedState>,
        timeout_secs: Option<u64>,
        response_tx: tokio::sync::oneshot::Sender<ControlResult>,
        cx: &mut Context<Self>,
    ) {
        let timeout = timeout_secs.unwrap_or(30);
        let initial = (|| {
            if !(1..=300).contains(&timeout) || states.is_empty() {
                return Err(ControlError::invalid_params(
                    "Timeout must be 1..300 seconds and states must not be empty",
                ));
            }
            let tab_idx = self.resolve_control_tab_index(tab_index)?;
            let resolved = self.resolve_surface_target_for_tab(tab_idx, target)?;
            let observation = self
                .explicit_program_observation(&resolved.terminal)
                .ok_or_else(|| {
                    ControlError::invalid_params("Terminal status is not initialized")
                })?;
            observation
                .match_after(&after, &record_id, &states)
                .map_err(ControlError::invalid_params)?;
            Ok((self.tabs[tab_idx].summary_id, resolved.surface_id))
        })();
        let (tab_id, surface_id) = match initial {
            Ok(target) => target,
            Err(error) => {
                Self::send_control_result(response_tx, Err(error));
                return;
            }
        };
        let started = std::time::Instant::now();
        cx.spawn(async move |this, cx| {
            loop {
                if response_tx.is_closed() {
                    return;
                }
                let result = this.update(
                    cx,
                    |workspace, cx| -> Result<Option<serde_json::Value>, ControlError> {
                        // Capture the tab's identity once; tab reordering cannot
                        // redirect an outstanding wait to another workspace.
                        let tab_idx =
                            workspace.tab_index_for_summary_id(tab_id).ok_or_else(|| {
                                ControlError::invalid_params("The target tab has closed")
                            })?;
                        workspace.refresh_terminal_presentation(cx);
                        let resolved = workspace.resolve_surface_target_for_tab(
                            tab_idx,
                            con_core::SurfaceTarget::new(None, None, Some(surface_id)),
                        )?;
                        let observation = workspace
                            .explicit_program_observation(&resolved.terminal)
                            .ok_or_else(|| {
                                ControlError::invalid_params(
                                    "The terminal incarnation is no longer available",
                                )
                            })?;
                        let record = observation
                            .match_after(&after, &record_id, &states)
                            .map_err(ControlError::invalid_params)?;
                        let status = if record.is_some() {
                            "matched"
                        } else if !resolved.terminal.is_alive(cx) {
                            "exited"
                        } else if started.elapsed() >= Duration::from_secs(timeout) {
                            "timeout"
                        } else {
                            return Ok(None);
                        };
                        workspace
                            .control_program_status_value(tab_idx, &resolved, status, record, cx)
                            .map(Some)
                    },
                );
                match result {
                    Ok(Ok(Some(value))) => {
                        Self::send_control_result(response_tx, Ok(value));
                        return;
                    }
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => {
                        Self::send_control_result(response_tx, Err(error));
                        return;
                    }
                    Err(_) => return,
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    }
}
