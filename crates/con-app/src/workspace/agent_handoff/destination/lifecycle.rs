use std::{
    sync::atomic::Ordering,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use con_core::handoff::{HandoffResponse, HandoffService, HandoffState};

use super::*;

impl HandoffDestinationPanel {
    /// Expire a `LaunchPending` job on its original 30-second deadline even
    /// when no new Tab was created from this panel (e.g. the window vanished
    /// before `start_handoff_tab` armed its own timer). The service refuses
    /// the transition once the launch has progressed or a live helper holds
    /// the launch guard, so arming late is safe.
    pub(super) fn arm_launch_expiry(&mut self, job: &HandoffJob, cx: &mut Context<Self>) {
        const LAUNCH_PENDING_TIMEOUT_SECS: u64 = 30;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let remaining =
            LAUNCH_PENDING_TIMEOUT_SECS.saturating_sub(now.saturating_sub(job.updated_at));
        let job_id = job.id.clone();
        let revision = job.revision;
        let task = self.runtime.spawn(async move {
            if remaining > 0 {
                tokio::time::sleep(Duration::from_secs(remaining)).await;
            }
            tokio::task::spawn_blocking(move || {
                HandoffService::new()?.expire_pending_launch(&job_id, revision)
            })
            .await
        });
        cx.spawn(async move |this, cx| match task.await {
            Ok(Ok(Ok(job))) => {
                let _ = this.update(cx, |panel, cx| {
                    panel.absorb_job_update(job);
                    cx.notify();
                });
            }
            Ok(Ok(Err(error))) => log::info!("handoff: launch expiry skipped: {error}"),
            Ok(Err(error)) | Err(error) => {
                log::warn!("handoff: launch expiry task failed: {error}")
            }
        })
        .detach();
    }

    /// Hand a freshly prepared job to the router. This runs straight out of
    /// `finish_prepare` — prepare and send are one step now, with no review
    /// stage between them — so the panel stays busy while the router
    /// re-checks identity and starts the delivery: a success closes the
    /// dialog, a failure comes back as an error or a job card.
    /// `sent_job_id` is what lets `report_error` find and reconcile the job.
    pub(super) fn dispatch_prepared(
        &mut self,
        job: HandoffJob,
        destination: Destination,
        cx: &mut Context<Self>,
    ) {
        log::info!("handoff: prepared job {} dispatched", job.id);
        self.busy = true;
        self.error = None;
        self.sent_job_id = Some(job.id.clone());
        cx.emit(ExecuteHandoff {
            job,
            destination,
            live_source_binding: self.live_source_id.clone(),
            source_session_confirmed: self.single_session_confirmed,
        });
    }

    /// Respond to or cancel the displayed job.
    pub(super) fn respond(
        &mut self,
        job_id: String,
        response: Option<HandoffResponse>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = self.tracked_job(&job_id).cloned() else {
            return;
        };
        self.busy = true;
        self.error = None;
        let task = self.runtime.spawn_blocking(move || {
            let service = HandoffService::new()?;
            let result = if let Some(response) = response {
                service.respond(&job.id, job.revision, response)
            } else {
                service.cancel(&job.id, job.revision)
            };
            Ok::<_, anyhow::Error>((result, service.get(&job.id)?))
        });
        cx.spawn_in(window, async move |this, window| {
            let result = task
                .await
                .map_err(anyhow::Error::from)
                .and_then(|value| value);
            let _ = window.update(|_, cx| {
                let _ = this.update(cx, |panel, cx| {
                    panel.busy = false;
                    match result {
                        Ok((result, job)) => {
                            panel.absorb_job_update(job);
                            if let Err(error) = result {
                                panel.error = Some(error.to_string());
                            }
                        }
                        Err(error) => panel.error = Some(error.to_string()),
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    pub fn report_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        self.busy = false;
        // Refresh a persisted launch/delivery intent after failure so the
        // job can be reconciled alongside the form for a new send.
        let Some(job_id) = self.sent_job_id.take() else {
            cx.notify();
            return;
        };
        let task = self.runtime.spawn_blocking(move || {
            let service = HandoffService::new()?;
            let job = service.get(&job_id)?;
            // Cancel an unused Prepared job; the router's own cancellation
            // is an idempotent safety net.
            if job.state == HandoffState::Prepared {
                let _ = service.cancel(&job.id, job.revision);
                return Ok(None);
            }
            Ok::<_, anyhow::Error>((!job.state.is_terminal()).then_some(job))
        });
        cx.spawn(async move |this, cx| match task.await {
            Ok(Ok(Some(job))) => {
                let _ = this.update(cx, |panel, cx| {
                    if job.state == HandoffState::LaunchPending {
                        // The launch outcome is unresolved; keep the original
                        // 30-second expiry armed so the card cannot stall.
                        panel.arm_launch_expiry(&job, cx);
                    }
                    panel.active_job = Some(job);
                    cx.notify();
                });
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => log::warn!("handoff: job refresh after error failed: {error}"),
            Err(error) => log::warn!("handoff: job refresh task failed: {error}"),
        })
        .detach();
        cx.notify();
    }

    /// Report a failed send whose launch error the router has already
    /// persisted, handing back the post-write job so the card refresh uses
    /// the latest revision instead of racing the write and caching a stale
    /// one. Falls back to a re-read when the write itself failed.
    pub fn report_launch_failure(
        &mut self,
        error: String,
        job: Option<HandoffJob>,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = job else {
            self.report_error(error, cx);
            return;
        };
        self.error = Some(error);
        self.busy = false;
        self.sent_job_id = None;
        if job.state == HandoffState::LaunchPending {
            // The launch outcome is unresolved; keep the original 30-second
            // expiry armed so the card cannot stall.
            self.arm_launch_expiry(&job, cx);
        }
        self.active_job = (!job.state.is_terminal()).then_some(job);
        cx.notify();
    }

    /// Unified teardown: the panel window closed (or the app tore down) with
    /// a prepare still in flight. Cancel only while the job never
    /// left `Prepared`; a persisted launch/delivery intent (`Delivering` and
    /// beyond) is left for the user to reconcile and is never replayed
    /// automatically. Once `dispatch_prepared` has emitted, the router owns
    /// the job and teardown must not cancel it.
    pub fn release_undelivered(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(request_id) = self.preparing_request_id.take() {
            super::cancel_prepared_handoff(&self.runtime, request_id);
        }
    }
}
