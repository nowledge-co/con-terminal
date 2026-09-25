use super::{
    HandoffBundle, HandoffJob, HandoffState,
    store::{self, Store},
};
use anyhow::{Result, ensure};
use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;

impl Store {
    pub fn cleanup_expired(&self) -> Result<usize> {
        let _lock = self.lock()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        // Holding the lock proves no prepare is in flight, so staging
        // directories are crash leftovers. A sweep failure must not block
        // the per-record cleanup below.
        let mut removed = match self.sweep_incomplete() {
            Ok(removed) => removed,
            Err(error) => {
                log::warn!("Could not sweep incomplete handoffs: {error}");
                0
            }
        };
        for job in self.jobs()? {
            if !matches!(job.state, HandoffState::Cancelled | HandoffState::Failed)
                || now.saturating_sub(job.created_at.max(job.updated_at)) < RETENTION_SECONDS
            {
                continue;
            }
            // One damaged, incomplete, or unreadable record must never abort
            // the whole pass; conservatively retain it and continue.
            match self.remove_job_checked(&job) {
                Ok(true) => removed += 1,
                Ok(false) => {}
                Err(error) => {
                    log::warn!(
                        "Retaining handoff {} after a cleanup error: {error}",
                        job.id
                    )
                }
            }
        }
        Ok(removed)
    }

    /// Validate both the project export and private record, then remove them
    /// together. A live launch helper is reported as an error (its own
    /// message names the fix); any other doubt — missing files, user edits,
    /// symlinks — returns `Ok(false)` without deleting anything.
    pub(super) fn remove_job_checked(&self, job: &HandoffJob) -> Result<bool> {
        let _guard = self.launch_guard(&job.id)?;
        let bundle = self.bundle(&job.id)?;
        let project = bundle.workspace.cwd.join(".con/handoffs").join(&job.id);
        // Only `NotFound` proves the export is absent. Any other error
        // (permissions, transient I/O) means its presence is unknown, so
        // retain both directories rather than risk a partial delete.
        let staged = match fs::symlink_metadata(&project) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => return Ok(false),
        };
        // Validate both directories completely — names, types, and contents —
        // before deleting anything. A partial delete would desynchronize the
        // project export from the private record, so any doubt retains both.
        if staged {
            let con = bundle.workspace.cwd.join(".con");
            if fs::symlink_metadata(&con)?.file_type().is_symlink() {
                return Ok(false);
            }
            if store::private_dir(&con.join("handoffs")).is_err() {
                return Ok(false);
            }
            if !registered_files(&project, &["context.md", "evidence.json"])?
                || !export_matches(&project, &bundle)?
            {
                return Ok(false);
            }
        }
        let private = self.directory(&job.id)?;
        if !registered_files(
            &private,
            &[
                "job.json",
                "bundle.json",
                "context.md",
                "evidence.json",
                "launch.lock",
            ],
        )? || !export_matches(&private, &bundle)?
        {
            return Ok(false);
        }
        if staged {
            fs::remove_dir_all(&project)?;
        }
        fs::remove_dir_all(private)?;
        Ok(true)
    }
}

fn registered_files(path: &Path, names: &[&str]) -> Result<bool> {
    store::private_dir(path)?;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if !names.iter().any(|name| entry.file_name() == *name) {
            return Ok(false);
        }
        let meta = fs::symlink_metadata(entry.path())?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Unsafe retained handoff file"
        );
    }
    Ok(true)
}

/// The export files must still match the immutable bundle. Leave user-edited,
/// replaced, or incomplete exports intact, in the project and private
/// directories alike.
fn export_matches(dir: &Path, bundle: &HandoffBundle) -> Result<bool> {
    let Ok(context) = fs::read(dir.join("context.md")) else {
        return Ok(false);
    };
    let Ok(evidence) = fs::read(dir.join("evidence.json")) else {
        return Ok(false);
    };
    Ok(context == bundle.context.as_bytes()
        && evidence == serde_json::to_vec_pretty(&bundle.history)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{HandoffBundle, PrepareRequest, WorkspaceSnapshot};
    use con_agent::handoff::{AgentKind, HistoryExport, SourceSession, TargetCapabilities};
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        project: PathBuf,
        store: Store,
    }

    impl Fixture {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("handoff-cleanup-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&dir).unwrap();
            let dir = dir.canonicalize().unwrap();
            let project = dir.join("project");
            fs::create_dir(&project).unwrap();
            let store = Store::new(dir.join("private")).unwrap();
            Self {
                root: dir,
                project,
                store,
            }
        }

        /// An expired, released job with a complete private record. When
        /// `stage_export` is set, a matching project export is written too;
        /// `missing_evidence` models a partial export left by a failed stage.
        fn expired_job(
            &self,
            corrupt_bundle: bool,
            stage_export: bool,
            missing_evidence: bool,
        ) -> String {
            let id = uuid::Uuid::new_v4().to_string();
            let history = HistoryExport {
                source: SourceSession {
                    export_warning: None,
                    agent: AgentKind::Codex,
                    id: "source".into(),
                    store_identity: "test".into(),
                    title: "Title".into(),
                    cwd: self.project.clone(),
                    updated_at: 1,
                },
                agent_version: "test".into(),
                last_turn_id: "turn".into(),
                records: vec![],
                omissions: vec![],
                digest: "digest".into(),
            };
            let bundle = HandoffBundle {
                schema_version: 1,
                handoff_id: id.clone(),
                created_at: 0,
                history,
                workspace: WorkspaceSnapshot {
                    cwd: self.project.clone(),
                    root: self.project.clone(),
                    git_dir: self.project.join(".git"),
                    git_common_dir: self.project.join(".git"),
                    head: String::new(),
                    index_digest: String::new(),
                    worktree_digest: String::new(),
                    status: String::new(),
                    untracked: vec![],
                },
                goal: "goal".into(),
                context: "context".into(),
            };
            let job = HandoffJob {
                id: id.clone(),
                revision: 2,
                created_at: 0,
                updated_at: 0,
                request: PrepareRequest {
                    source_agent: AgentKind::Codex,
                    target_agent: AgentKind::Cursor,
                    request_id: id.clone(),
                    cwd: self.project.clone(),
                    source_session_id: "source".into(),
                    goal: "goal".into(),
                    target_model: None,
                },
                state: HandoffState::Cancelled,
                target: TargetCapabilities {
                    agent: AgentKind::Cursor,
                    executable: "/test/cursor-agent".into(),
                    version: "test".into(),
                    automatic_delivery: false,
                },
                existing_target_tab_id: None,
                target_session_id: None,
                target_pid: None,
                target_process_start: None,
                receipt: None,
                error: None,
                fallback: None,
            };
            let private = self.store.directory(&id).unwrap();
            store::private_dir(&private).unwrap();
            store::write_new(
                &private.join("job.json"),
                &serde_json::to_vec_pretty(&job).unwrap(),
            )
            .unwrap();
            if corrupt_bundle {
                store::write_new(&private.join("bundle.json"), b"{ not json").unwrap();
            } else {
                store::write_new(
                    &private.join("bundle.json"),
                    &serde_json::to_vec_pretty(&bundle).unwrap(),
                )
                .unwrap();
            }
            store::write_new(&private.join("context.md"), b"context").unwrap();
            store::write_new(
                &private.join("evidence.json"),
                &serde_json::to_vec_pretty(&bundle.history).unwrap(),
            )
            .unwrap();
            if stage_export {
                let export = self.project.join(".con/handoffs").join(&id);
                fs::create_dir_all(self.project.join(".con")).unwrap();
                store::private_dir(&self.project.join(".con/handoffs")).unwrap();
                store::private_dir(&export).unwrap();
                store::write_new(&export.join("context.md"), b"context").unwrap();
                if !missing_evidence {
                    store::write_new(
                        &export.join("evidence.json"),
                        &serde_json::to_vec_pretty(&bundle.history).unwrap(),
                    )
                    .unwrap();
                }
            }
            id
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_damaged_record_is_retained_without_blocking_the_rest() {
        let f = Fixture::new();
        let healthy = f.expired_job(false, true, false);
        let partial = f.expired_job(false, true, true);
        let corrupt = f.expired_job(true, false, false);
        // The healthy record is cleaned even though the other two fail.
        assert_eq!(f.store.cleanup_expired().unwrap(), 1);
        assert!(!f.store.root.join(&healthy).exists());
        assert!(!f.project.join(".con/handoffs").join(&healthy).exists());
        // An incomplete export keeps both its directories.
        assert!(f.store.root.join(&partial).join("job.json").exists());
        assert!(
            f.project
                .join(".con/handoffs")
                .join(&partial)
                .join("context.md")
                .exists()
        );
        // A corrupt bundle keeps its private record.
        assert!(f.store.root.join(&corrupt).join("job.json").exists());
    }

    #[test]
    fn an_extra_or_modified_private_file_keeps_both_directories() {
        let f = Fixture::new();
        let extra = f.expired_job(false, true, false);
        fs::write(
            f.store.directory(&extra).unwrap().join("user-notes.txt"),
            "keep",
        )
        .unwrap();
        let edited = f.expired_job(false, true, false);
        // Overwrite in place so the required 0600 permissions are preserved.
        fs::write(
            f.store.directory(&edited).unwrap().join("context.md"),
            b"user edited",
        )
        .unwrap();
        // Neither record is cleaned, and the project exports stay in sync
        // with their retained private records — no partial deletion.
        assert_eq!(f.store.cleanup_expired().unwrap(), 0);
        for id in [&extra, &edited] {
            assert!(f.store.root.join(id).join("job.json").exists());
            assert!(
                f.project
                    .join(".con/handoffs")
                    .join(id)
                    .join("context.md")
                    .exists()
            );
        }
    }

    #[test]
    fn a_locked_launch_guard_retains_record_and_cleanup_succeeds() {
        let f = Fixture::new();
        let id = f.expired_job(false, true, false);
        // Hold the launch guard as if a helper is running.
        let _guard = f.store.launch_guard(&id).unwrap();
        // cleanup_expired must not abort; it retains the guarded record
        // and removes nothing.
        assert_eq!(f.store.cleanup_expired().unwrap(), 0);
        assert!(f.store.root.join(&id).join("job.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_export_probe_retains_both_directories() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new();
        let id = f.expired_job(false, true, false);
        // Make the export's presence undecidable: probing it must fail with
        // something other than NotFound (here, permission denied).
        let handoffs = f.project.join(".con/handoffs");
        fs::set_permissions(&handoffs, fs::Permissions::from_mode(0o000)).unwrap();
        let result = f.store.cleanup_expired();
        fs::set_permissions(&handoffs, fs::Permissions::from_mode(0o700)).unwrap();
        // The pass still succeeds, but nothing was deleted — an unknown
        // export state must never trigger a partial delete.
        assert_eq!(result.unwrap(), 0);
        assert!(f.store.root.join(&id).join("job.json").exists());
        assert!(handoffs.join(&id).join("context.md").exists());
    }
}
