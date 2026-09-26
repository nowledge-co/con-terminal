use super::protocol;

/// Why `reserve_start_checked` refused a dispatch. The control plane maps
/// request/state conflicts to a client error and helper environment faults
/// to a server error, so a perfectly valid request (e.g. when con-cli is
/// missing, stuck, or too old) is never reported as invalid params.
#[derive(Debug)]
pub(in crate::workspace::agent_handoff) enum StartGateError {
    /// The reserve step rejected the request itself: unknown job, stale
    /// revision, wrong state, or a workspace that changed since prepare.
    Request(anyhow::Error),
    /// The environment failed: con-cli missing, stuck, or older than the
    /// protocol floor — or the reserve step hit a storage/snapshot I/O fault
    /// (store lock, record read/save, corrupt record). Never a bad request.
    Helper(anyhow::Error),
}

impl std::fmt::Display for StartGateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(error) | Self::Helper(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StartGateError {}

/// Mirror of the store's `MISSING_RECORD` tag (not exported by con-core): a
/// record file missing under a surviving job directory is a
/// storage-integrity fault, never the "unknown job" conflict its inner
/// `NotFound` suggests. Keep both copies in sync.
const MISSING_RECORD_MARKER: &str = "Handoff record is missing";
/// Reserve the launch and run the pre-dispatch protocol gate as one step, so
/// the UI route and the control-plane start dispatch through identical
/// checks. A failed gate records a reviewable `launch_error` on the reserved
/// job (→ `NeedsInteraction`) instead of stranding it in `LaunchPending` or
/// silently downgrading the model. Runs on the blocking pool.
pub(in crate::workspace::agent_handoff) fn reserve_start_checked(
    service: &con_core::handoff::HandoffService,
    job_id: &str,
    revision: u64,
) -> Result<con_core::handoff::HandoffJob, StartGateError> {
    let pending = service
        .reserve_start(job_id, revision)
        .map_err(classify_reserve_error)?;
    if let Err(error) = protocol::check_helper_protocol() {
        let _ = service.launch_error(&pending.id, &error.to_string());
        return Err(StartGateError::Helper(error));
    }
    Ok(pending)
}

/// Classify a `reserve_start` failure for the control plane: request/state
/// conflicts (unknown job, stale revision, wrong state, changed workspace)
/// stay client errors, while storage and snapshot I/O faults (lock
/// acquisition, record reads, state saves, corrupt records) are server-side
/// environment faults. The store reports plain `anyhow` errors, so the split
/// inspects the error chain: I/O errors are storage faults except `NotFound`
/// (the "unknown job" conflict), JSON errors are corrupt records, and the
/// remaining string checks match the store's conflict messages. The store
/// tags a record missing under a surviving job directory
/// (`MISSING_RECORD_MARKER`) to keep it off the "unknown job" side. Anything
/// unrecognized defaults to a server fault so a valid request is never
/// misreported as invalid params. Shared by the Start gate and the generic
/// control-plane branch (List/Get/Respond/Cancel/Prepare), whose store errors
/// need the same split.
pub(in crate::workspace::agent_handoff) fn classify_reserve_error(
    error: anyhow::Error,
) -> StartGateError {
    if error
        .chain()
        .any(|cause| cause.to_string().contains(MISSING_RECORD_MARKER))
    {
        return StartGateError::Helper(error);
    }
    let mut request_conflict = None;
    for cause in error.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            request_conflict = Some(io.kind() == std::io::ErrorKind::NotFound);
            break;
        }
        if cause.downcast_ref::<serde_json::Error>().is_some() {
            request_conflict = Some(false);
            break;
        }
    }
    let request_conflict = request_conflict.unwrap_or_else(|| {
        const CONFLICTS: &[&str] = &[
            "Conflict:",
            "Handoff has already been started",
            "Invalid handoff ID",
            "Handoff ID must be a canonical UUID",
            "Workspace changed",
        ];
        let message = error.to_string();
        CONFLICTS.iter().any(|needle| message.contains(needle))
    });
    if request_conflict {
        StartGateError::Request(error)
    } else {
        StartGateError::Helper(error)
    }
}

/// Cancel the unused job only while it never left `Prepared`; later
/// states may have a persisted launch/delivery intent and are left for the
/// user to reconcile.
pub(in crate::workspace::agent_handoff) fn cancel_prepared_handoff(
    runtime: &std::sync::Arc<tokio::runtime::Runtime>,
    job_id: String,
) {
    runtime.spawn_blocking(move || {
        if let Ok(service) = con_core::handoff::HandoffService::new()
            && let Ok(current) = service.get(&job_id)
            && current.state == con_core::handoff::HandoffState::Prepared
        {
            let _ = service.cancel(&job_id, current.revision);
        }
    });
}

#[cfg(all(test, unix))]
mod tests {
    use super::{StartGateError, classify_reserve_error, reserve_start_checked};

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "con-handoff-gate-classify-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// A real store lookup for a job that does not exist must stay a client
    /// error (-32602), not a server fault.
    #[test]
    fn an_unknown_job_is_a_request_conflict() {
        let root = temp_root("unknown");
        let service = con_core::handoff::HandoffService::with_root(root.clone()).unwrap();
        let error =
            reserve_start_checked(&service, &con_core::handoff::new_request_id(), 1).unwrap_err();
        assert!(matches!(error, StartGateError::Request(_)), "{error:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A real store whose record cannot be parsed is a storage-integrity
    /// fault: the request was valid, so it must map to a server error
    /// (-32000), never to invalid params.
    #[test]
    fn a_corrupt_record_is_a_helper_fault() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("corrupt");
        let service = con_core::handoff::HandoffService::with_root(root.clone()).unwrap();
        let id = con_core::handoff::new_request_id();
        let dir = root.join(&id);
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let record = dir.join("job.json");
        std::fs::write(&record, b"not json").unwrap();
        std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = reserve_start_checked(&service, &id, 1).unwrap_err();
        assert!(matches!(error, StartGateError::Helper(_)), "{error:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    /// An existing job whose `bundle.json` is gone is a storage-integrity
    /// fault, not the "unknown job" conflict its inner `NotFound` suggests:
    /// it maps to a server error (-32000), never to invalid params.
    #[test]
    fn a_missing_bundle_of_an_existing_job_is_a_helper_fault() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("missing-bundle");
        let service = con_core::handoff::HandoffService::with_root(root.clone()).unwrap();
        let id = con_core::handoff::new_request_id();
        let dir = root.join(&id);
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let job = serde_json::json!({
            "id": id, "revision": 1, "created_at": 1,
            "request": {"request_id": id, "cwd": root, "source_session_id": "s"},
            "state": "prepared",
            "target": {"executable": root, "version": "v", "automatic_delivery": false},
            "target_session_id": null, "target_pid": null, "receipt": null, "error": null,
        });
        let record = dir.join("job.json");
        std::fs::write(&record, serde_json::to_vec(&job).unwrap()).unwrap();
        std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        let error = reserve_start_checked(&service, &id, 1).unwrap_err();
        assert!(matches!(error, StartGateError::Helper(_)), "{error:?}");
        assert!(
            error.to_string().contains(super::MISSING_RECORD_MARKER),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reserve_errors_classify_into_request_and_helper() {
        // Request/state conflicts stay client errors.
        for message in [
            "Conflict: handoff changed; refresh its state",
            "Handoff has already been started",
            "Invalid handoff ID",
            "Handoff ID must be a canonical UUID",
            "Workspace changed; send a new handoff",
        ] {
            let error = classify_reserve_error(anyhow::anyhow!(message.to_owned()));
            assert!(matches!(error, StartGateError::Request(_)), "{message}");
        }
        let not_found: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::NotFound, "no job record").into();
        assert!(matches!(
            classify_reserve_error(not_found),
            StartGateError::Request(_)
        ));
        // Storage / snapshot I/O faults are server-side environment faults,
        // including when a context layer wraps the I/O error.
        let denied: anyhow::Error =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "store locked").into();
        assert!(matches!(
            classify_reserve_error(denied),
            StartGateError::Helper(_)
        ));
        let wrapped = anyhow::Context::context(
            Err::<(), std::io::Error>(std::io::Error::other("disk full")),
            "save job",
        )
        .unwrap_err();
        assert!(matches!(
            classify_reserve_error(wrapped),
            StartGateError::Helper(_)
        ));
        // A corrupt record (JSON) is a store-integrity fault, not bad params.
        let corrupt: anyhow::Error = serde_json::from_str::<serde_json::Value>("{")
            .unwrap_err()
            .into();
        assert!(matches!(
            classify_reserve_error(corrupt),
            StartGateError::Helper(_)
        ));
        // Unrecognized failures default to a server fault, never bad params.
        let unknown = classify_reserve_error(anyhow::anyhow!("Unsafe handoff file"));
        assert!(matches!(unknown, StartGateError::Helper(_)));
    }
}
