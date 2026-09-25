use super::*;

fn delivering(f: &Fixture) -> HandoffJob {
    let mut request = f.request();
    request.target_agent = AgentKind::Kimi;
    let job = f.prepare(request).unwrap();
    let job = f.service.reserve_start(&job.id, job.revision).unwrap();
    let job = f
        .service
        .begin_launch(&job.id, job.revision, &job.target)
        .unwrap();
    f.service
        .record_launch(&job.id, job.revision, None)
        .unwrap()
}

#[test]
fn abandon_failure_cancels_job_even_with_live_helper() {
    let f = Fixture::new();
    let job = delivering(&f);
    let job = f
        .service
        .record_kimi_failure(&job.id, job.revision, "not ready")
        .unwrap();
    let _guard = f.service.launch_guard(&job.id).unwrap();
    assert!(job.can_abandon());
    // Non-PTY receipts do not block explicit abandonment of a failed job.
    let mut non_pty = job.clone();
    for receipt in ["automatic_delivery_spawn_observed", "user_confirmed_sent"] {
        non_pty.receipt = Some(receipt.into());
        assert!(non_pty.can_abandon());
        non_pty.state = HandoffState::Active;
        assert!(!non_pty.can_abandon());
        non_pty.state = HandoffState::NeedsInteraction;
    }
    let done = f
        .service
        .respond(&job.id, job.revision, HandoffResponse::Abandon)
        .unwrap();
    assert_eq!(done.state, HandoffState::Cancelled);
    assert_eq!(done.receipt.as_deref(), Some("user_abandoned"));
    assert!(f.service.launch_error(&job.id, "late exit").is_err());
    assert!(
        f.service
            .record_kimi_submit(&job.id, job.revision, 42, 100)
            .is_err()
    );
}

#[test]
fn cancel_new_kimi_delivery_releases_in_one_step() {
    let f = Fixture::new();
    let job = delivering(&f);
    let job = f.service.cancel(&job.id, job.revision).unwrap();
    assert_eq!(job.state, HandoffState::Cancelled);
}

#[test]
fn pty_submitted_jobs_never_offer_or_accept_abandon() {
    let f = Fixture::new();
    let job = delivering(&f);
    let job = f
        .service
        .record_kimi_submit(&job.id, job.revision, 42, 100)
        .unwrap();
    assert!(!job.can_abandon());
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::Abandon)
            .is_err()
    );
    let job = f.service.cancel(&job.id, job.revision).unwrap();
    assert_eq!(job.state, HandoffState::NeedsInteraction);
    assert!(!job.can_abandon());
    assert!(
        f.service
            .respond(&job.id, job.revision, HandoffResponse::Abandon)
            .is_err()
    );
    f.service
        .respond(&job.id, job.revision, HandoffResponse::ConfirmStopped)
        .unwrap();
}
