mod delivery;
mod fallback;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, ensure};
use con_agent::handoff::{
    HistoryExport, MAX_EXPORT_BYTES, TargetCapabilities, export_session, probe_target, sanitize,
};

use super::{
    context, snapshot,
    store::{self, Store},
    *,
};

#[derive(Clone)]
pub struct HandoffService {
    pub(super) store: Store,
}

impl HandoffService {
    pub fn new() -> Result<Self> {
        let parent = con_paths::app_data_dir();
        fs::create_dir_all(&parent)?;
        let service = Self::with_root(parent.join("handoffs"))?;
        if let Err(error) = service.store.cleanup_expired() {
            log::warn!("Could not clean expired handoffs: {error}");
        }
        Ok(service)
    }

    pub fn with_root(root: PathBuf) -> Result<Self> {
        Ok(Self {
            store: Store::new(root)?,
        })
    }

    /// Held by the visible helper until the target exits. Serializes this job's launch and stop confirmation.
    pub fn launch_guard(&self, id: &str) -> Result<HandoffLock> {
        let _lock = self.store.lock()?;
        self.store.job(id)?;
        self.store.launch_guard(id)
    }

    pub fn get(&self, id: &str) -> Result<HandoffJob> {
        let _lock = self.store.lock()?;
        self.store.job(id)
    }

    pub fn bundle(&self, id: &str) -> Result<HandoffBundle> {
        let _lock = self.store.lock()?;
        self.store.bundle(id)
    }

    pub fn list(&self, cwd: &Path) -> Result<Vec<HandoffJob>> {
        let cwd = cwd.canonicalize()?;
        let _lock = self.store.lock()?;
        Ok(self
            .store
            .jobs()?
            .into_iter()
            .filter(|j| j.request.cwd == cwd)
            .collect())
    }

    pub async fn prepare(&self, mut request: PrepareRequest) -> Result<HandoffJob> {
        ensure!(
            cfg!(target_os = "macos"),
            "Agent Handoff currently supports macOS only"
        );
        ensure_supported_agents(&request)?;
        store::validate_id(&request.request_id)?;
        ensure!(
            request.goal.len() <= 4096,
            "Goal is limited to 4 KiB; keep additional context in the source history"
        );
        if let Some(model) = &request.target_model {
            validate_target_model_for_agent(request.target_agent, model)?;
        }
        request.cwd = request.cwd.canonicalize()?;
        request.goal = sanitize(&request.goal);
        let service = self.clone();
        let req = request.clone();
        if let Some(job) = tokio::task::spawn_blocking(move || service.existing(&req)).await?? {
            return Ok(job);
        }
        let cwd = request.cwd.clone();
        let before = tokio::task::spawn_blocking(move || snapshot::capture(&cwd)).await??;
        let history = export_session(
            request.source_agent,
            &request.cwd,
            &request.source_session_id,
        )
        .await?;
        let target = probe_target(request.target_agent).await?;
        let service = self.clone();
        tokio::task::spawn_blocking(move || {
            service.prepare_export(request, history, target, before)
        })
        .await?
    }

    fn existing(&self, request: &PrepareRequest) -> Result<Option<HandoffJob>> {
        let _lock = self.store.lock()?;
        if self
            .store
            .directory(&request.request_id)?
            .join("job.json")
            .exists()
        {
            let job = self.store.job(&request.request_id)?;
            ensure!(
                job.request == *request,
                "Conflict: request ID was used with different input"
            );
            Ok(Some(job))
        } else {
            Ok(None)
        }
    }

    pub(super) fn prepare_export(
        &self,
        request: PrepareRequest,
        history: HistoryExport,
        target: TargetCapabilities,
        before: WorkspaceSnapshot,
    ) -> Result<HandoffJob> {
        ensure_supported_agents(&request)?;
        if let Some(model) = &request.target_model {
            validate_target_model_for_agent(request.target_agent, model)?;
        }
        ensure!(
            history.source.agent == request.source_agent
                && target.agent == request.target_agent
                && history.source.id == request.source_session_id
                && history.source.cwd.canonicalize()? == request.cwd,
            "Source identity does not match the requested session and directory"
        );
        let _lock = self.store.lock()?;
        // Crash leftovers can only exist while no live prepare holds the lock.
        self.store.sweep_incomplete()?;
        if self
            .store
            .directory(&request.request_id)?
            .join("job.json")
            .exists()
        {
            let job = self.store.job(&request.request_id)?;
            ensure!(
                job.request == request,
                "Conflict: request ID has different input"
            );
            return Ok(job);
        }
        snapshot::unchanged(&before)?;
        // Diagnose damaged records without blocking independent jobs.
        let _ = self.store.corrupt_jobs()?;
        let created_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let context = context::render(&request.request_id, &request.goal, &history, &before);
        let bundle = HandoffBundle {
            schema_version: 1,
            handoff_id: request.request_id.clone(),
            created_at,
            history,
            workspace: before,
            goal: request.goal.clone(),
            context,
        };
        let bytes = serde_json::to_vec_pretty(&bundle)?;
        ensure!(
            bytes.len() <= MAX_EXPORT_BYTES,
            "Filtered handoff exceeds 10 MiB; select a smaller source session"
        );
        let dir = self.store.directory(&request.request_id)?;
        ensure!(
            !dir.exists(),
            "An incomplete handoff already uses this ID; retry with a new request ID"
        );
        let job = HandoffJob {
            id: request.request_id.clone(),
            revision: 1,
            created_at,
            updated_at: created_at,
            request,
            state: HandoffState::Prepared,
            target,
            existing_target_tab_id: None,
            target_session_id: None,
            target_pid: None,
            target_process_start: None,
            receipt: None,
            error: None,
            fallback: None,
        };
        // Publish atomically: stage every file, then a single rename makes the
        // job visible. A crash before the rename leaves only a staging
        // directory, which the next locked sweep removes.
        let staging = self.store.staging_dir()?;
        let publish = || -> Result<()> {
            store::write_new(&staging.join("bundle.json"), &bytes)?;
            store::write_new(&staging.join("context.md"), bundle.context.as_bytes())?;
            store::write_new(
                &staging.join("evidence.json"),
                &serde_json::to_vec_pretty(&bundle.history)?,
            )?;
            store::write_new(&staging.join("job.json"), &serde_json::to_vec_pretty(&job)?)?;
            fs::rename(&staging, &dir)?;
            // The rename already published the job, so a fsync failure must
            // not contradict the now-visible state.
            store::fsync_dir(&self.store.root);
            Ok(())
        };
        if let Err(error) = publish() {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        Ok(job)
    }

    fn update(
        &self,
        id: &str,
        revision: u64,
        action: impl FnOnce(&mut HandoffJob) -> Result<()>,
    ) -> Result<HandoffJob> {
        let _lock = self.store.lock()?;
        let mut job = self.store.job(id)?;
        ensure!(
            job.revision == revision,
            "Conflict: handoff changed; refresh its state"
        );
        action(&mut job)?;
        job.revision += 1;
        job.updated_at = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        self.store.save(&job)?;
        Ok(job)
    }

    fn verify_snapshot(&self, id: &str) -> Result<HandoffBundle> {
        let bundle = self.store.bundle(id)?;
        snapshot::unchanged(&bundle.workspace)?;
        Ok(bundle)
    }
    fn stage_and_verify(&self, bundle: &HandoffBundle) -> Result<()> {
        store::stage(bundle)?;
        snapshot::unchanged(&bundle.workspace)
    }
}

fn ensure_supported_agents(request: &PrepareRequest) -> Result<()> {
    use con_agent::handoff::AgentKind;
    ensure!(
        request.source_agent != AgentKind::Unknown && request.target_agent != AgentKind::Unknown,
        "Unsupported Agent in this handoff; cancel it and confirm the target stopped"
    );
    Ok(())
}
