use super::*;
use con_agent::handoff::{
    AgentKind, HistoryExport, HistoryRecord, SourceSession, TargetCapabilities,
};
use std::{fs, path::PathBuf, process::Command};

mod abandon;
mod existing;
mod kimi;
mod legacy;
mod lifecycle;

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
    service: HandoffService,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("con-handoff-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let repo = dir.join("中文 project ' quote");
        fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&repo)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q"]);
        fs::write(repo.join("tracked.txt"), "staged").unwrap();
        git(&["add", "tracked.txt"]);
        fs::write(repo.join("tracked.txt"), "unstaged").unwrap();
        fs::write(repo.join("untracked.txt"), "keep me").unwrap();
        let service = HandoffService::with_root(dir.join("private")).unwrap();
        Self { dir, repo, service }
    }
    fn request(&self) -> PrepareRequest {
        PrepareRequest {
            source_agent: AgentKind::Codex,
            target_agent: AgentKind::Cursor,
            request_id: uuid::Uuid::new_v4().to_string(),
            cwd: self.repo.clone(),
            source_session_id: "exact-source".into(),
            goal: "修复 remaining test".into(),
            target_model: None,
        }
    }
    fn prepare(&self, request: PrepareRequest) -> anyhow::Result<HandoffJob> {
        self.prepare_with_delivery(request, false)
    }
    fn prepare_automatic(&self, request: PrepareRequest) -> anyhow::Result<HandoffJob> {
        self.prepare_with_delivery(request, true)
    }
    fn prepare_with_delivery(
        &self,
        request: PrepareRequest,
        automatic_delivery: bool,
    ) -> anyhow::Result<HandoffJob> {
        let history = HistoryExport {
            source: SourceSession {
                export_warning: None,
                agent: AgentKind::Codex,
                id: "exact-source".into(),
                store_identity: "test".into(),
                title: "Fix".into(),
                cwd: self.repo.clone(),
                updated_at: 1,
            },
            agent_version: "test".into(),
            last_turn_id: "turn".into(),
            records: vec![HistoryRecord {
                turn_id: "turn".into(),
                item_id: "item".into(),
                role: "user".into(),
                text: "Preserve all changes".into(),
            }],
            omissions: vec![],
            digest: "fixture".into(),
        };
        let caps = TargetCapabilities {
            agent: request.target_agent,
            executable: "/test/cursor-agent".into(),
            version: "test".into(),
            automatic_delivery,
        };
        self.service
            .prepare_export(request, history, caps, snapshot::capture(&self.repo)?)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn prepare_is_idempotent_and_independent_jobs_survive_reopen() {
    let f = Fixture::new();
    let request = f.request();
    let job = f.prepare(request.clone()).unwrap();
    assert_eq!(f.prepare(request).unwrap().id, job.id);
    assert!(f.prepare(f.request()).is_ok());
    let reopened = HandoffService::with_root(f.dir.join("private")).unwrap();
    assert_eq!(reopened.get(&job.id).unwrap().state, HandoffState::Prepared);
    reopened.cancel(&job.id, job.revision).unwrap();
    assert!(f.prepare(f.request()).is_ok());
}

#[test]
fn same_request_id_with_different_content_is_rejected() {
    let f = Fixture::new();
    let mut request = f.request();
    f.prepare(request.clone()).unwrap();
    request.goal = "different".into();
    assert!(f.prepare(request).is_err());
}

#[test]
fn target_model_must_be_a_safe_standalone_identifier() {
    validate_target_model("gpt-5-codex").unwrap();
    validate_target_model("claude-sonnet-4.5").unwrap();
    validate_target_model(&"m".repeat(MAX_TARGET_MODEL_LEN)).unwrap();
    for bad in ["", "   ", "-m", "--model", "mod\nel", "mod\0el"] {
        assert!(validate_target_model(bad).is_err(), "accepted {bad:?}");
    }
    assert!(validate_target_model(&"m".repeat(MAX_TARGET_MODEL_LEN + 1)).is_err());
}

#[test]
fn legacy_request_without_target_model_defaults_to_unspecified() {
    // Jobs written before the source-stop confirmation was removed still
    // carry `source_stopped`; the unknown field must stay readable.
    let json = serde_json::json!({
        "source_agent": "codex",
        "target_agent": "cursor",
        "request_id": "legacy",
        "cwd": "/tmp",
        "source_session_id": "s",
        "source_stopped": true
    });
    let request: PrepareRequest = serde_json::from_value(json).unwrap();
    assert_eq!(request.target_model, None);
}

#[test]
fn target_model_is_validated_and_joins_request_idempotency() {
    let f = Fixture::new();
    let mut request = f.request();
    request.target_model = Some("-looks-like-a-flag".into());
    assert!(f.prepare(request).is_err());

    let mut request = f.request();
    request.target_model = Some("gpt-5".into());
    let job = f.prepare(request.clone()).unwrap();
    assert!(f.prepare(request.clone()).is_ok());
    request.target_model = Some("other-model".into());
    assert!(f.prepare(request).is_err());

    // The visible helper reads the job from the store across a process
    // boundary: the model — its refuse-or-apply decision basis — must survive
    // the round-trip, and the frozen protocol revision covers the override.
    const { assert!(LAUNCH_HELPER_PROTOCOL >= 2) };
    let reopened = HandoffService::with_root(f.dir.join("private")).unwrap();
    assert_eq!(
        reopened
            .get(&job.id)
            .unwrap()
            .request
            .target_model
            .as_deref(),
        Some("gpt-5")
    );
}

#[test]
fn edits_and_wrong_source_block_delivery_without_touching_changes() {
    let f = Fixture::new();
    let mut request = f.request();
    request.source_session_id = "other".into();
    assert!(f.prepare(request).is_err());
    let job = f.prepare(f.request()).unwrap();
    fs::write(f.repo.join("untracked.txt"), "new edit").unwrap();
    assert!(f.service.reserve_start(&job.id, job.revision).is_err());
    assert_eq!(
        fs::read_to_string(f.repo.join("tracked.txt")).unwrap(),
        "unstaged"
    );
}

#[test]
fn creation_and_delivery_are_not_replayed_after_crashes() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let reserved = f.service.reserve_start(&job.id, job.revision).unwrap();
    assert!(f.service.reserve_start(&job.id, job.revision).is_err());
    let started = f
        .service
        .begin_launch(&job.id, reserved.revision, &job.target)
        .unwrap();
    assert!(
        f.service
            .begin_launch(&job.id, started.revision, &job.target)
            .is_err()
    );
    let target = uuid::Uuid::new_v4().to_string();
    let delivering = f
        .service
        .record_target(&job.id, started.revision, &target)
        .unwrap();
    assert_eq!(delivering.state, HandoffState::Delivering);
    assert!(
        f.service
            .record_existing_delivery(&job.id, delivering.revision, 7, 1)
            .is_err()
    );
    assert!(
        f.service
            .record_target(&job.id, delivering.revision, &target)
            .is_err()
    );
    let cancel = f.service.cancel(&job.id, delivering.revision).unwrap();
    assert_eq!(cancel.state, HandoffState::NeedsInteraction);
    assert!(f.prepare(f.request()).is_ok());
    f.service
        .respond(&job.id, cancel.revision, HandoffResponse::ConfirmStopped)
        .unwrap();
    assert!(f.prepare(f.request()).is_ok());
}

#[test]
fn stage_preserves_index_untracked_work_and_exclude_rules() {
    let f = Fixture::new();
    let exclude = f.repo.join(".git/info/exclude");
    fs::write(&exclude, "# existing user rule\nkeep-local\n").unwrap();
    let before = snapshot::capture(&f.repo).unwrap();
    let job = f.prepare(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    f.service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    assert_eq!(snapshot::capture(&f.repo).unwrap(), before);
    assert!(
        fs::read_to_string(exclude)
            .unwrap()
            .starts_with("# existing user rule\nkeep-local\n")
    );
    assert!(
        fs::read_to_string(f.repo.join(".git/info/exclude"))
            .unwrap()
            .contains(&format!("/.con/handoffs/{}/", job.id))
    );
    let bundle = f.service.bundle(&job.id).unwrap();
    let staged = fs::read_to_string(
        f.repo
            .join(".con/handoffs")
            .join(&job.id)
            .join("context.md"),
    )
    .unwrap();
    assert_eq!(bundle.context, staged);
    assert!(!f.repo.join(".gitignore").exists());
}

#[cfg(unix)]
#[test]
fn symlink_staging_and_path_traversal_are_rejected() {
    let f = Fixture::new();
    assert!(f.service.get("../../other").is_err());
    std::os::unix::fs::symlink(&f.dir, f.repo.join(".con")).unwrap();
    let job = f.prepare(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    assert!(
        f.service
            .begin_launch(&job.id, job.revision, &job.target)
            .is_err()
    );
    assert!(!f.dir.join("handoffs").exists());
}
