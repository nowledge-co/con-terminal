use super::*;

#[test]
fn manual_delivery_completes_on_the_explicit_sent_confirmation() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    let job = f
        .service
        .record_target(&job.id, job.revision, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let job = f
        .service
        .record_spawn(&job.id, job.revision, 42, Some(1_700_000_000))
        .unwrap();
    assert_eq!(job.state, HandoffState::AwaitingManualDelivery);
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
    // "I sent the instruction" is the final confirmation; no second receipt
    // confirmation step follows.
    let job = f
        .service
        .respond(&job.id, job.revision, HandoffResponse::ConfirmSent)
        .unwrap();
    assert_eq!(job.state, HandoffState::Active);
    assert_eq!(job.receipt.as_deref(), Some("user_confirmed_sent"));
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
}

#[test]
fn live_helper_cannot_be_released_or_started_twice() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let guard = f.service.launch_guard(&job.id).unwrap();
    assert!(f.service.launch_guard(&job.id).is_err());
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f.service.cancel(&job.id, job.revision).unwrap();
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmStopped)
            .is_err()
    );
    drop(guard);
    f.service
        .respond(&job.id, job.revision, HandoffResponse::ConfirmStopped)
        .unwrap();
}

#[test]
fn retention_never_removes_active_or_user_modified_artifacts() {
    let f = Fixture::new();
    let mut job = f.prepare(f.request()).unwrap();
    job.created_at = 0;
    job.updated_at = 0;
    let store = store::Store::new(f.dir.join("private")).unwrap();
    store.save(&job).unwrap();
    assert_eq!(store.cleanup_expired().unwrap(), 0);
    let mut job = f.service.cancel(&job.id, job.revision).unwrap();
    // Retention starts when a long-running handoff is released, not when prepared.
    assert_eq!(store.cleanup_expired().unwrap(), 0);
    job.updated_at = 0;
    store.save(&job).unwrap();
    let dir = store.directory(&job.id).unwrap();
    fs::write(dir.join("user-notes.txt"), "keep").unwrap();
    assert_eq!(store.cleanup_expired().unwrap(), 0);
    fs::remove_file(dir.join("user-notes.txt")).unwrap();
    assert_eq!(store.cleanup_expired().unwrap(), 1);
    assert!(!dir.exists());
}

#[test]
fn launch_timeout_cannot_race_a_live_helper_or_replay_delivery() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let guard = f.service.launch_guard(&job.id).unwrap();
    assert!(
        f.service
            .expire_pending_launch(&job.id, job.revision)
            .is_err()
    );
    drop(guard);
    let expired = f
        .service
        .expire_pending_launch(&job.id, job.revision)
        .unwrap();
    assert_eq!(expired.state, HandoffState::NeedsInteraction);
    assert!(
        f.service
            .begin_launch(&job.id, job.revision, &job.target)
            .is_err()
    );
    assert!(f.service.reserve_start(&job.id, expired.revision).is_err());
}

#[test]
fn created_target_identity_survives_workspace_drift_before_delivery() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    fs::write(f.repo.join("untracked.txt"), "new edit during create-chat").unwrap();
    let target = uuid::Uuid::new_v4().to_string();
    let job = f
        .service
        .record_target(&job.id, job.revision, &target)
        .unwrap();
    assert_eq!(job.target_session_id.as_deref(), Some(target.as_str()));
    assert!(f.service.validate_workspace(&job.id).is_err());
    assert!(f.service.reserve_start(&job.id, job.revision).is_err());
}

#[test]
fn agent_identity_cannot_be_changed_while_reusing_a_request_id() {
    let f = Fixture::new();
    let request = f.request();
    f.prepare(request.clone()).unwrap();
    let mut changed = request;
    changed.source_agent = AgentKind::Kimi;
    assert!(f.prepare(changed).is_err());
}

#[test]
fn unobserved_native_identity_completes_on_the_explicit_sent_confirmation() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    let job = f
        .service
        .record_launch(&job.id, job.revision, None)
        .unwrap();
    // Recording launch intent is not evidence that the target has started.
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmSent)
            .is_err()
    );
    let job = f
        .service
        .record_spawn(&job.id, job.revision, 123, Some(1_700_000_000))
        .unwrap();
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
    // The explicit "sent" confirmation finishes the manual delivery even
    // though the native target ID was never observed; the receipt names the
    // handoff UUID, not an inferred native ID.
    let job = f
        .service
        .respond(&job.id, job.revision, HandoffResponse::ConfirmSent)
        .unwrap();
    assert_eq!(job.state, HandoffState::Active);
    assert!(job.target_session_id.is_none());
    assert_eq!(job.receipt.as_deref(), Some("user_confirmed_sent"));
    assert!(f.service.reserve_start(&job.id, job.revision).is_err());
}

#[test]
fn automatic_delivery_completes_on_the_observed_spawn() {
    let f = Fixture::new();
    let job = f.prepare_automatic(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    let job = f
        .service
        .record_target(&job.id, job.revision, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    // The initial prompt rode the target's argv and the user's Send click was
    // the confirmation, so the observed spawn finishes delivery directly.
    let job = f
        .service
        .record_spawn(&job.id, job.revision, 42, Some(1_700_000_000))
        .unwrap();
    assert_eq!(job.state, HandoffState::Active);
    assert_eq!(job.target_pid, Some(42));
    assert_eq!(
        job.receipt.as_deref(),
        Some("automatic_delivery_spawn_observed")
    );
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
}

#[test]
fn legacy_awaiting_confirmation_jobs_still_complete_via_confirm_received() {
    let f = Fixture::new();
    let mut job = f.prepare(f.request()).unwrap();
    // Jobs persisted before the send-is-confirmation change can sit in
    // AwaitingConfirmation; they must still reach Active through the original
    // explicit receipt instead of being stranded.
    job.state = HandoffState::AwaitingConfirmation;
    let store = store::Store::new(f.dir.join("private")).unwrap();
    store.save(&job).unwrap();
    let job = f
        .service
        .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
        .unwrap();
    assert_eq!(job.state, HandoffState::Active);
    assert_eq!(
        job.receipt.as_deref(),
        Some("user_confirmed_handoff_native_id_unobserved")
    );
}

#[test]
fn native_target_identifiers_are_opaque_not_storage_paths() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    assert!(
        f.service
            .record_target(&job.id, job.revision, "--resume")
            .is_err()
    );
    let job = f
        .service
        .record_target(&job.id, job.revision, "ses_native123")
        .unwrap();
    assert_eq!(job.target_session_id.as_deref(), Some("ses_native123"));
}

#[test]
fn first_version_bundles_and_requests_keep_codex_cursor_defaults() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let mut legacy = serde_json::to_value(&job).unwrap();
    legacy["request"]
        .as_object_mut()
        .unwrap()
        .remove("source_agent");
    legacy["request"]
        .as_object_mut()
        .unwrap()
        .remove("target_agent");
    legacy["target"].as_object_mut().unwrap().remove("agent");
    legacy
        .as_object_mut()
        .unwrap()
        .remove("existing_target_tab_id");
    let restored: HandoffJob = serde_json::from_value(legacy).unwrap();
    assert_eq!(restored.request.source_agent, AgentKind::Codex);
    assert_eq!(restored.target.agent, AgentKind::Cursor);
    assert!(restored.existing_target_tab_id.is_none());
    assert_eq!(restored.request, job.request);
    let mut bundle = serde_json::to_value(f.service.bundle(&job.id).unwrap()).unwrap();
    let history = bundle["history"].as_object_mut().unwrap();
    let version = history.remove("agent_version").unwrap();
    history.insert("codex_version".into(), version);
    history["source"].as_object_mut().unwrap().remove("agent");
    let restored: HandoffBundle = serde_json::from_value(bundle).unwrap();
    assert_eq!(restored.history.source.agent, AgentKind::Codex);
    assert_eq!(restored.history.agent_version, "test");
}

#[test]
fn corrupt_job_record_is_retained_without_blocking_prepare() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let dir = f.dir.join("private").join(&job.id);
    // Truncating in place keeps the 0600 permissions the store requires.
    fs::write(dir.join("job.json"), b"{ not json").unwrap();
    // A single corrupt record no longer fails listing; its directory is kept.
    assert!(f.service.list(&f.repo).unwrap().is_empty());
    assert!(dir.join("job.json").exists());
    assert!(f.prepare(f.request()).is_ok());
    assert!(dir.join("job.json").exists());
}

#[test]
fn a_record_that_lost_its_job_file_is_retained_without_blocking_prepare() {
    let f = Fixture::new();
    let root = f.dir.join("private");
    // A crashed staged prepare is still a provable crash leftover: swept.
    let staging = root.join(".tmp-crashed");
    fs::create_dir(&staging).unwrap();
    fs::write(staging.join("bundle.json"), b"partial").unwrap();
    // A published job whose job.json is lost is indistinguishable from a
    // crashed legacy prepare on disk, so it is conservatively retained.
    let job = f.prepare(f.request()).unwrap();
    let dir = root.join(&job.id);
    fs::remove_file(dir.join("job.json")).unwrap();
    // The sweep runs inside the next locked prepare: the staging directory
    // goes, the job directory keeps every private record.
    assert!(f.prepare(f.request()).is_ok());
    assert!(!staging.exists());
    for file in ["bundle.json", "context.md", "evidence.json"] {
        assert!(dir.join(file).exists(), "{file} must be retained");
    }
}

#[test]
fn a_missing_target_process_cancels_the_active_job() {
    let f = Fixture::new();
    let job = f.prepare_automatic(f.request()).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    let job = f
        .service
        .record_target(&job.id, job.revision, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let job = f
        .service
        .record_spawn(&job.id, job.revision, 42, Some(1_700_000_000))
        .unwrap();
    assert_eq!(job.target_process_start, Some(1_700_000_000));
    let released = f
        .service
        .cancel_absent_target(&job.id, job.revision)
        .unwrap();
    assert_eq!(released.state, HandoffState::Cancelled);
    assert_eq!(released.receipt.as_deref(), Some("target_process_absent"));

    let pending = f.prepare(f.request()).unwrap();
    let delivering = f
        .service
        .begin_existing_delivery(&pending.id, pending.revision, 1)
        .unwrap();
    assert!(
        f.service
            .cancel_absent_target(&pending.id, delivering.revision)
            .is_err()
    );
    assert_eq!(
        f.service.get(&pending.id).unwrap().state,
        HandoffState::Delivering
    );
}

#[test]
fn launch_failure_keeps_a_visible_error_after_spawn() {
    let f = Fixture::new();
    let mut job = f.prepare_automatic(f.request()).unwrap();
    job.state = HandoffState::Active;
    f.service.store.save(&job).unwrap();
    let failed = f
        .service
        .launch_error(
            &job.id,
            "Codex exited unsuccessfully; check TUI bootstrap and network/proxy.
Handoff target=codex launch_env_keys=[OPENAI_API_KEY]",
        )
        .unwrap();
    assert_eq!(failed.state, HandoffState::NeedsInteraction);
    assert_eq!(
        failed.error.as_deref(),
        Some("Target failed to start — see target Tab")
    );
    assert_eq!(failed.fallback.unwrap().kind, FallbackKind::ArgvBootstrap);
    assert!(f.service.reserve_start(&job.id, failed.revision).is_err());
}
