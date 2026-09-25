use super::*;

#[test]
fn removed_agents_remain_readable_without_blocking_prepare() {
    check_removed_agent("opencode", "pi");
}

#[test]
fn removed_dimagent_remains_readable_without_blocking_prepare() {
    check_removed_agent("dimagent", "dimagent");
}

fn check_removed_agent(source: &str, target: &str) {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let dir = f.service.store.directory(&job.id).unwrap();
    let mut record = serde_json::to_value(&job).unwrap();
    record["request"]["source_agent"] = source.into();
    record["request"]["target_agent"] = target.into();
    record["target"]["agent"] = target.into();
    store::atomic(&dir.join("job.json"), &record).unwrap();
    let mut bundle = serde_json::to_value(f.service.bundle(&job.id).unwrap()).unwrap();
    bundle["history"]["source"]["agent"] = source.into();
    store::atomic(&dir.join("bundle.json"), &bundle).unwrap();

    let reopened = HandoffService::with_root(f.dir.join("private")).unwrap();
    let loaded = reopened.get(&job.id).unwrap();
    assert_eq!(loaded.request.source_agent, AgentKind::Unknown);
    assert_eq!(loaded.target.agent, AgentKind::Unknown);
    assert_eq!(reopened.list(&f.repo).unwrap().len(), 1);
    assert_eq!(
        reopened.bundle(&job.id).unwrap().history.source.agent,
        AgentKind::Unknown
    );
    assert!(reopened.reserve_start(&job.id, loaded.revision).is_err());
    assert!(
        reopened
            .begin_existing_delivery(&job.id, loaded.revision, 42)
            .is_err()
    );
    assert!(f.prepare(f.request()).is_ok());
    // A never-launched legacy job can still be explicitly cancelled.
    reopened.cancel(&job.id, loaded.revision).unwrap();
    assert!(f.prepare(f.request()).is_ok());
}

#[test]
fn removed_active_agent_requires_explicit_stop_confirmation() {
    let f = Fixture::new();
    let mut job = f.prepare(f.request()).unwrap();
    job.state = HandoffState::Active;
    // Exercise actual persisted data from the removed adapter.
    let mut record = serde_json::to_value(&job).unwrap();
    record["target"]["agent"] = "dimagent".into();
    job = serde_json::from_value(record).unwrap();
    assert_eq!(job.target.agent, AgentKind::Unknown);
    f.service.store.save(&job).unwrap();
    assert!(
        f.service
            .cancel_absent_target(&job.id, job.revision)
            .is_err()
    );
    let cancelled = f.service.cancel(&job.id, job.revision).unwrap();
    assert_eq!(cancelled.state, HandoffState::NeedsInteraction);
    let released = f
        .service
        .respond(&job.id, cancelled.revision, HandoffResponse::ConfirmStopped)
        .unwrap();
    assert_eq!(released.state, HandoffState::Cancelled);
}
