//! Refresh persisted evidence off the render path, notifying only on changes.
use super::*;
use con_core::handoff::{FallbackEvidence, FallbackKind, HandoffService, HandoffState};

impl HandoffDestinationPanel {
    pub(super) fn observe_job_card(&self, cx: &mut Context<Self>) {
        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(500))
                    .await;
                let Ok(id) = this.update(cx, |panel, _| {
                    panel.active_job.as_ref().map(|job| job.id.clone())
                }) else {
                    break;
                };
                let Some(id) = id else { continue };
                let result = runtime
                    .spawn_blocking(move || -> anyhow::Result<_> {
                        let service = HandoffService::new()?;
                        let mut job = service.get(&id)?;
                        let available = service.bundle(&id).is_ok();
                        // A missing pid is uncertainty. Only a recorded process identity
                        // that has disappeared/reused may produce target_dead.
                        if job.state == HandoffState::NeedsInteraction
                            && job
                                .fallback
                                .as_ref()
                                .is_none_or(|g| g.kind != FallbackKind::TargetDead)
                            && job.target_pid.zip(job.target_process_start).is_some_and(
                                |(pid, start)| {
                                    con_agent::handoff::process_start_secs(pid as i32)
                                        != Some(start)
                                },
                            )
                        {
                            job = service.record_fallback_observation(
                                &id,
                                job.revision,
                                FallbackKind::TargetDead,
                                FallbackEvidence::Idle,
                            )?;
                        }
                        Ok((job, available))
                    })
                    .await;
                if let Ok(Ok((job, available))) = result {
                    let _ = this.update(cx, |panel, cx| {
                        if panel
                            .tracked_job(&job.id)
                            .is_none_or(|cached| cached.revision > job.revision)
                        {
                            return;
                        }
                        let available = available.then(|| job.id.clone());
                        if panel
                            .tracked_job(&job.id)
                            .is_some_and(|cached| cached.revision != job.revision)
                            || panel.instruction_available != available
                        {
                            panel.instruction_available = available;
                            panel.absorb_job_update(job);
                            cx.notify();
                        }
                    });
                }
            }
        })
        .detach();
    }
}
