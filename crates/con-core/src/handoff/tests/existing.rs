use super::*;

#[test]
fn existing_tab_delivery_is_staged_once_and_completes_on_the_observed_write() {
    let f = Fixture::new();
    let before = snapshot::capture(&f.repo).unwrap();
    let job = f.prepare(f.request()).unwrap();
    let delivering = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 42)
        .unwrap();
    assert_eq!(delivering.state, HandoffState::Delivering);
    assert_eq!(delivering.existing_target_tab_id, Some(42));
    assert!(delivering.target_session_id.is_none());
    assert!(delivering.target_pid.is_none());
    assert_eq!(snapshot::capture(&f.repo).unwrap(), before);
    assert!(
        f.repo
            .join(".con/handoffs")
            .join(&job.id)
            .join("context.md")
            .is_file()
    );

    let reopened = HandoffService::with_root(f.dir.join("private")).unwrap();
    assert_eq!(
        reopened.get(&job.id).unwrap().existing_target_tab_id,
        Some(42)
    );
    assert!(
        reopened
            .begin_existing_delivery(&job.id, job.revision, 42)
            .is_err()
    );
    assert!(
        reopened
            .begin_existing_delivery(&job.id, delivering.revision, 42)
            .is_err()
    );
    // The user's Send click was the explicit confirmation; an observed PTY
    // write finishes the delivery without a second confirmation step.
    let active = reopened
        .record_existing_delivery(&job.id, delivering.revision, 99, 1_700_000_000)
        .unwrap();
    assert_eq!(active.state, HandoffState::Active);
    assert_eq!(active.target_pid, Some(99));
    assert_eq!(active.target_process_start, Some(1_700_000_000));
    assert_eq!(active.receipt.as_deref(), Some("delivery_write_observed"));
    assert!(active.target_session_id.is_none());
    assert!(
        reopened
            .record_existing_delivery(&job.id, active.revision, 99, 1_700_000_000)
            .is_err()
    );
    assert!(
        reopened
            .respond(&job.id, active.revision, HandoffResponse::ConfirmReceived)
            .is_err()
    );
}

#[test]
fn existing_tab_unknown_write_can_be_explicitly_confirmed_after_restart() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    let delivering = f
        .service
        .begin_existing_delivery(&job.id, job.revision, 7)
        .unwrap();
    let reopened = HandoffService::with_root(f.dir.join("private")).unwrap();
    assert_eq!(
        reopened.get(&job.id).unwrap().state,
        HandoffState::Delivering
    );
    // The user has inspected the target and confirmed the visible handoff ID.
    let active = reopened
        .respond(
            &job.id,
            delivering.revision,
            HandoffResponse::ConfirmReceived,
        )
        .unwrap();
    assert_eq!(active.state, HandoffState::Active);
    assert_eq!(active.existing_target_tab_id, Some(7));
    assert!(active.target_session_id.is_none());
    assert!(
        reopened
            .record_existing_delivery(&job.id, active.revision, 99, 1_700_000_000)
            .is_err()
    );
}

#[test]
fn existing_tab_delivery_rejects_workspace_changes_before_staging() {
    let f = Fixture::new();
    let job = f.prepare(f.request()).unwrap();
    fs::write(f.repo.join("untracked.txt"), "changed").unwrap();
    assert!(
        f.service
            .begin_existing_delivery(&job.id, job.revision, 7)
            .is_err()
    );
    assert_eq!(
        f.service.get(&job.id).unwrap().state,
        HandoffState::Prepared
    );
    assert!(!f.repo.join(".con/handoffs").exists());
}

#[test]
fn existing_tab_in_flight_guard_is_atomic() {
    let f = Fixture::new();
    let jobs = [
        f.prepare(f.request()).unwrap(),
        f.prepare(f.request()).unwrap(),
    ];
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let attempts: Vec<_> = jobs
            .iter()
            .map(|job| {
                let service = &f.service;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    service.begin_existing_delivery(&job.id, job.revision, 42)
                })
            })
            .collect();
        attempts
            .into_iter()
            .map(|attempt| attempt.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let rejected = outcomes.iter().position(|result| result.is_err()).unwrap();
    assert!(
        outcomes[rejected]
            .as_ref()
            .unwrap_err()
            .to_string()
            .contains("Target tab is receiving")
    );
    let job = &jobs[rejected];
    let unchanged = f.service.get(&job.id).unwrap();
    assert_eq!(unchanged.state, HandoffState::Prepared);
    assert_eq!(unchanged.revision, job.revision);
    assert!(!f.repo.join(".con/handoffs").join(&job.id).exists());
    // The rejected job can deliver to a different tab in the same worktree.
    f.service
        .begin_existing_delivery(&job.id, job.revision, 43)
        .unwrap();
}

#[test]
fn existing_tab_guard_only_blocks_pending_delivery_to_that_tab() {
    for state in [
        HandoffState::LaunchPending,
        HandoffState::Delivering,
        HandoffState::Prepared,
        HandoffState::StartingTarget,
        HandoffState::Active,
        HandoffState::NeedsInteraction,
        HandoffState::AwaitingConfirmation,
        HandoffState::AwaitingManualDelivery,
        HandoffState::Cancelled,
        HandoffState::Failed,
    ] {
        let f = Fixture::new();
        let mut prior = f.prepare(f.request()).unwrap();
        prior.state = state;
        prior.existing_target_tab_id = Some(42);
        // Scope is the destination tab, regardless of the previous source cwd.
        prior.request.cwd = f.dir.join("other-source");
        f.service.store.save(&prior).unwrap();
        let job = f.prepare(f.request()).unwrap();
        let result = f.service.begin_existing_delivery(&job.id, job.revision, 42);
        let blocked = matches!(
            state,
            HandoffState::LaunchPending | HandoffState::Delivering
        );
        assert_eq!(result.is_err(), blocked, "{state:?}: {result:?}");
        if blocked {
            let prior = f.service.cancel(&prior.id, prior.revision).unwrap();
            assert_eq!(prior.state, HandoffState::NeedsInteraction);
            f.service
                .begin_existing_delivery(&job.id, job.revision, 42)
                .unwrap();
        }
    }
}
