use super::*;

fn kimi_job(f: &Fixture) -> HandoffJob {
    let mut request = f.request();
    request.target_agent = AgentKind::Kimi;
    f.prepare(request).unwrap()
}

#[test]
fn kimi_spawn_waits_for_submit_receipt_and_cannot_replay() {
    let f = Fixture::new();
    let job = kimi_job(&f);
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    let job = f
        .service
        .record_launch(&job.id, job.revision, None)
        .unwrap();
    let job = f
        .service
        .record_spawn(&job.id, job.revision, 42, Some(100))
        .unwrap();
    assert_eq!(job.state, HandoffState::Delivering);
    assert!(job.receipt.is_none());
    let active = f
        .service
        .record_kimi_submit(&job.id, job.revision, 42, 100)
        .unwrap();
    assert_eq!(active.state, HandoffState::Active);
    assert_eq!(
        active.receipt.as_deref(),
        Some("pty_injection_submit_observed")
    );
    assert!(
        f.service
            .record_kimi_submit(&job.id, job.revision, 42, 100)
            .is_err()
    );
}

#[test]
fn kimi_existing_and_fallback_keep_identity() {
    let f = Fixture::new();
    let job = kimi_job(&f);
    let job = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 8)
        .unwrap();
    assert!(
        f.service
            .record_kimi_submit(&job.id, job.revision, 0, 0)
            .is_err()
    );
    let job = f
        .service
        .launch_error(&job.id, "Trust pending; instruction copied")
        .unwrap();
    assert_eq!(job.state, HandoffState::NeedsInteraction);
    assert!(
        f.service
            .record_kimi_submit(&job.id, job.revision, 42, 100)
            .is_err()
    );
    assert_eq!(job.fallback.as_ref().unwrap().kind, FallbackKind::Trust);
    assert_eq!(
        job.error.as_deref(),
        Some("Approve folder trust in Kimi first")
    );
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
    let job = f
        .service
        .record_fallback_observation(
            &job.id,
            job.revision,
            FallbackKind::PastePending,
            FallbackEvidence::Submitted,
        )
        .unwrap();
    assert_eq!(job.state, HandoffState::NeedsInteraction);
    assert!(job.receipt.is_none());
    let job = f
        .service
        .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
        .unwrap();
    assert_eq!(job.state, HandoffState::Active);
}

#[test]
fn kimi_receipt_rejects_other_agents() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let job = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 8)
        .unwrap();
    assert!(
        f.service
            .record_kimi_submit(&job.id, job.revision, 42, 100)
            .is_err()
    );
}

#[test]
fn stale_failure_cannot_overwrite_cancellation_or_submit() {
    let f = Fixture::new();
    let job = kimi_job(&f);
    let job = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 8)
        .unwrap();
    assert!(
        f.service
            .record_existing_delivery(&job.id, job.revision, 42, 100)
            .is_err()
    );
    f.service.cancel(&job.id, job.revision).unwrap();
    assert!(
        f.service
            .record_kimi_failure(&job.id, job.revision, "timeout")
            .is_err()
    );
    assert!(
        f.service
            .record_kimi_submit(&job.id, job.revision, 42, 100)
            .is_err()
    );
}

#[test]
fn fallback_observation_never_overwrites_abandon_or_active() {
    for evidence in [FallbackEvidence::PasteDetected, FallbackEvidence::Submitted] {
        let f = Fixture::new();
        let job = kimi_job(&f);
        let job = f
            .service
            .begin_existing_delivery(&job.id, job.revision, 8)
            .unwrap();
        let job = f
            .service
            .record_kimi_failure(&job.id, job.revision, "trust")
            .unwrap();
        assert!(job.fallback.as_ref().unwrap().clipboard_copied);
        let detected = f
            .service
            .record_fallback_observation(
                &job.id,
                job.revision,
                FallbackKind::PastePending,
                evidence,
            )
            .unwrap();
        assert_eq!(detected.state, HandoffState::NeedsInteraction);
        assert!(detected.receipt.is_none());
        let cancelled = f
            .service
            .respond(&job.id, detected.revision, HandoffResponse::Abandon)
            .unwrap();
        for revision in [job.revision, cancelled.revision] {
            assert!(
                f.service
                    .record_fallback_observation(
                        &job.id,
                        revision,
                        FallbackKind::PastePending,
                        evidence
                    )
                    .is_err()
            );
        }
        assert_eq!(
            f.service.get(&job.id).unwrap().receipt.as_deref(),
            Some("user_abandoned")
        );
    }
    let f = Fixture::new();
    let job = kimi_job(&f);
    let job = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 8)
        .unwrap();
    let job = f
        .service
        .record_kimi_submit(&job.id, job.revision, 42, 100)
        .unwrap();
    assert!(
        f.service
            .record_fallback_observation(
                &job.id,
                job.revision,
                FallbackKind::PastePending,
                FallbackEvidence::Submitted
            )
            .is_err()
    );
}

#[test]
fn clipboard_failure_and_dead_target_do_not_claim_completion() {
    let f = Fixture::new();
    let job = kimi_job(&f);
    let job = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 8)
        .unwrap();
    let job = f
        .service
        .record_kimi_failure(&job.id, job.revision, "clipboard_failed")
        .unwrap();
    assert!(!job.fallback.as_ref().unwrap().clipboard_copied);
    let job = f
        .service
        .record_fallback_observation(
            &job.id,
            job.revision,
            FallbackKind::TargetDead,
            FallbackEvidence::Idle,
        )
        .unwrap();
    assert!(!job.fallback_confirmation_ready());
    assert!(job.can_abandon());
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
}

#[test]
fn existing_fallback_persists_checked_identity_without_a_delivery_receipt() {
    let f = Fixture::new();
    let job = kimi_job(&f);
    let job = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 8)
        .unwrap();
    assert!(job.target_pid.is_none());
    let job = f
        .service
        .record_kimi_fallback(&job.id, job.revision, "trust", Some((42, 100)))
        .unwrap();
    assert_eq!(
        (job.target_pid, job.target_process_start),
        (Some(42), Some(100))
    );
    assert_eq!(job.state, HandoffState::NeedsInteraction);
    assert!(job.receipt.is_none());
    assert!(
        f.service
            .record_kimi_fallback(&job.id, job.revision, "trust", Some((43, 100)))
            .is_err()
    );
}
