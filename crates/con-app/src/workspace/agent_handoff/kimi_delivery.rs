//! Shared, bounded Kimi PTY delivery for both destination types.
use super::kimi_delivery_state::{Action, Delivery};
use super::*;
use con_core::handoff::{HandoffJob, HandoffService, HandoffState};
use std::io::Write;

fn backup_instruction(instruction: &str) -> anyhow::Result<()> {
    let mut child = std::process::Command::new("/usr/bin/pbcopy")
        .env_clear()
        .envs(con_agent::handoff::launch_environment(
            con_agent::handoff::AgentKind::Kimi,
        ))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let written = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Clipboard unavailable"))?
        .write_all(instruction.as_bytes());
    let status = child.wait()?;
    written?;
    anyhow::ensure!(status.success(), "Clipboard backup failed");
    Ok(())
}

impl ConWorkspace {
    pub(super) fn deliver_kimi_via_pty(
        &mut self,
        job: HandoffJob,
        tab_id: u64,
        terminal: TerminalPane,
        cx: &mut Context<Self>,
    ) {
        let runtime = self.harness.runtime_handle();
        let handle = self.window_handle;
        cx.spawn(async move |this, cx| {
            let instruction = con_core::handoff::instruction(&job.id);
            let backup = runtime.spawn_blocking({
                let instruction = instruction.clone();
                move || backup_instruction(&instruction)
            }).await;
            let mut error = "not-ready".to_string();
            let mut delivery = Delivery::default();
            let started = std::time::Instant::now();
            let mut submitted = false;
            let mut activated = false;

            if matches!(backup, Ok(Ok(()))) {
                for _ in 0..120 {
                    // Re-read durable intent: cancellation or helper failure must
                    // stop the injector, even while waiting for trust approval.
                    let current = runtime.spawn_blocking({
                        let id = job.id.clone();
                        move || HandoffService::new()?.get(&id)
                    }).await;
                    if !matches!(current, Ok(Ok(ref current)) if current.state == HandoffState::Delivering && current.revision == job.revision) {
                        return;
                    }
                    let observed = handle.update(cx, |_, window, cx| {
                        this.update(cx, |workspace, cx| -> anyhow::Result<bool> {
                            let index = workspace.tabs.iter().position(|tab| tab.summary_id == tab_id)
                                .ok_or_else(|| anyhow::anyhow!("Target Tab closed"))?;
                            anyhow::ensure!(workspace.tabs[index].pane_tree.all_surface_terminals().iter()
                                .any(|item| item.entity_id() == terminal.entity_id()) && terminal.is_alive(cx),
                                "Target terminal replaced or stopped");
                            let pid = job.target_pid.ok_or_else(|| anyhow::anyhow!("Target identity unavailable"))?;
                            anyhow::ensure!(job.target_process_start.is_some()
                                && con_agent::handoff::process_start_secs(pid as i32) == job.target_process_start,
                                "Target process changed");
                            if job.existing_target_tab_id.is_some() {
                                anyhow::ensure!(terminal.foreground_process_group_id(cx) == Some(u64::from(pid)),
                                    "Target foreground process changed");
                            }
                            if !activated {
                                workspace.activate_tab(index, window, cx);
                                activated = true;
                            }
                            let screen = terminal.content_lines(200, cx);
                            error = match super::kimi_screen::wait_kimi_ready(&screen) {
                                super::kimi_screen::Readiness::TrustPending => "trust",
                                _ if delivery.baseline().is_some() => "submit_uncertain",
                                super::kimi_screen::Readiness::Ready => "paste_pending",
                                _ => "not-ready",
                            }.into();
                            match delivery.observe(&screen, &instruction, started.elapsed()) {
                                Action::Confirmed => return Ok(true),
                                Action::Paste => {
                                    anyhow::ensure!(terminal.write_raw_observed(
                                        &con_agent::handoff::kimi_delivery_payload(&instruction), cx),
                                        "PTY write not observed");
                                }
                                Action::Submit => {
                                    anyhow::ensure!(terminal.write_raw_observed(b"\r", cx),
                                        "Submit retry was not observed");
                                }
                                Action::Wait => {}
                            }
                            Ok(false)
                        })
                    });
                    match observed {
                        Ok(Ok(Ok(true))) => { submitted = true; break; }
                        Ok(Ok(Ok(false))) => {}
                        _ => { error = "submit_uncertain".into(); break; }
                    }
                    // Polling cadence only: every write is gated by fresh screen evidence.
                    cx.background_executor().timer(std::time::Duration::from_millis(250)).await;
                }
            } else {
                error = "clipboard_failed".into();
            }
            let baseline = delivery.baseline();
            let recorded = runtime.spawn_blocking(move || -> anyhow::Result<HandoffJob> {
                let service = HandoffService::new()?;
                if submitted {
                    if let Err(err) = service.record_kimi_submit(&job.id, job.revision, job.target_pid.unwrap_or(0), job.target_process_start.unwrap_or(0)) {
                        service.record_kimi_failure(&job.id, job.revision, &format!("Could not record Kimi submission: {err}"))?;
                    }
                } else {
                    service.record_kimi_fallback(&job.id, job.revision, &error, job.target_pid.zip(job.target_process_start))?;
                }
                service.get(&job.id)
            }).await;
            match recorded {
                Ok(Ok(job)) if job.state == HandoffState::NeedsInteraction => {
                    let _ = this.update(cx, |workspace, cx| {
                        workspace.observe_kimi_fallback(job, tab_id, terminal, baseline, cx);
                    });
                }
                Ok(Ok(_)) => {}
                error => log::warn!("handoff: could not persist Kimi delivery outcome: {error:?}"),
            }
        }).detach();
    }
}
