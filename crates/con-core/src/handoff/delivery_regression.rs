use super::*;
use con_agent::handoff::{
    AgentKind, HistoryExport, HistoryRecord, SourceSession, TargetCapabilities,
};

#[test]
fn codex_without_preallocated_id_completes_only_after_spawn() {
    let root = std::env::temp_dir().join(format!("con-codex-delivery-{}", uuid::Uuid::new_v4()));
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    let repo = repo.canonicalize().unwrap();
    let service = HandoffService::with_root(root.join("store")).unwrap();
    let request = PrepareRequest {
        source_agent: AgentKind::Cursor,
        target_agent: AgentKind::Codex,
        request_id: new_request_id(),
        cwd: repo.clone(),
        source_session_id: "source-id".into(),
        goal: "Continue".into(),
        target_model: None,
    };
    let history = HistoryExport {
        source: SourceSession {
            export_warning: None,
            agent: AgentKind::Cursor,
            id: "source-id".into(),
            store_identity: "fixture".into(),
            title: "Work".into(),
            cwd: repo.clone(),
            updated_at: 1,
        },
        agent_version: "fixture".into(),
        last_turn_id: "turn".into(),
        records: vec![HistoryRecord {
            turn_id: "turn".into(),
            item_id: "item".into(),
            role: "user".into(),
            text: "Continue".into(),
        }],
        omissions: vec![],
        digest: "fixture".into(),
    };
    let target = TargetCapabilities {
        agent: AgentKind::Codex,
        executable: "/fixture/codex".into(),
        version: "codex-cli 0.156.1".into(),
        automatic_delivery: true,
    };
    let job = service
        .prepare_export(request, history, target, snapshot::capture(&repo).unwrap())
        .unwrap();
    let job = service.reserve_start(&job.id, job.revision).unwrap();
    let job = service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    let job = service.record_launch(&job.id, job.revision, None).unwrap();
    assert_eq!(job.state, HandoffState::Delivering);
    let job = service
        .record_spawn(&job.id, job.revision, 42, Some(1))
        .unwrap();
    assert_eq!(job.state, HandoffState::Active);
    assert!(job.target_session_id.is_none());
    assert_eq!(
        job.receipt.as_deref(),
        Some("automatic_delivery_spawn_observed")
    );
    std::fs::remove_dir_all(root).unwrap();
}
