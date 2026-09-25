use con_agent::handoff::SessionBinding;

/// A failed inspection is not proof that the user switched sessions.
/// Both outcomes refuse dispatch, with distinct recovery messages.
pub(super) fn check_binding(
    binding: anyhow::Result<Option<SessionBinding>>,
    expected: &str,
    confirmed: bool,
) -> Result<(), &'static str> {
    match binding {
        Ok(Some(binding)) if binding.id == expected => {
            if binding.requires_confirmation && !confirmed {
                Err("Source session needs confirmation; reopen Handoff")
            } else {
                Ok(())
            }
        }
        Ok(Some(_)) | Ok(None) => Err("Source Agent session changed; reopen Handoff"),
        Err(error) => {
            log::warn!("Cannot recheck source session before handoff: {error:#}");
            Err("Couldn't verify the source session; reopen Handoff")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionBinding, check_binding};

    #[test]
    fn inspection_failure_refuses_dispatch_without_claiming_session_changed() {
        let failed = check_binding(
            Err(anyhow::anyhow!("argv and screen IDs conflict")),
            "a",
            false,
        );
        assert_eq!(
            failed,
            Err("Couldn't verify the source session; reopen Handoff")
        );
        assert_ne!(failed, check_binding(Ok(None), "a", false));
    }

    #[test]
    fn only_the_expected_live_session_can_dispatch() {
        let binding = || SessionBinding {
            id: "a".into(),
            evidence: "fixture",
            requires_confirmation: false,
        };
        assert!(check_binding(Ok(Some(binding())), "a", false).is_ok());
        assert_eq!(
            check_binding(Ok(Some(binding())), "b", false),
            check_binding(Ok(None), "a", false)
        );
    }

    #[test]
    fn loss_of_process_evidence_requires_confirmation_even_for_same_id() {
        let recent = || SessionBinding {
            id: "a".into(),
            evidence: "recent",
            requires_confirmation: true,
        };
        assert!(check_binding(Ok(Some(recent())), "a", false).is_err());
        assert!(check_binding(Ok(Some(recent())), "a", true).is_ok());
    }
}

/// Missing process evidence is distinct from ambiguous locks or probe failure.
pub(super) fn codex_binding_hint(error: Option<&str>) -> &'static str {
    match error {
        None => {
            "No Codex writer lock detected — session may be starting or idle. Select it manually."
        }
        Some(error) if error.contains("multiple thread locks") => {
            "Multiple Codex writer locks detected — select the current session."
        }
        Some(error) if error.contains("multiple rollout files") => {
            "Multiple Codex rollout files detected — select the current session."
        }
        Some(error) if error.contains("multiple process/peer thread IDs") => {
            "Conflicting Codex sessions detected — select the current session."
        }
        Some(error) if error.contains("app-server is not connected") => {
            "Codex app-server isn't connected — wait or select a session."
        }
        Some(_) => "Couldn't inspect Codex session — select it manually.",
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::codex_binding_hint;
    #[test]
    fn distinguishes_missing_ambiguous_and_failed_evidence() {
        assert!(codex_binding_hint(None).starts_with("No Codex writer lock"));
        assert!(
            codex_binding_hint(Some("Codex process holds multiple thread locks"))
                .starts_with("Multiple Codex writer locks")
        );
        assert!(
            codex_binding_hint(Some("Codex process holds multiple rollout files"))
                .starts_with("Multiple Codex rollout files")
        );
        assert!(
            codex_binding_hint(Some("Codex process inspection timed out"))
                .starts_with("Couldn't inspect")
        );
        assert!(
            codex_binding_hint(Some("Codex app-server is not connected"))
                .contains("isn't connected")
        );
        assert!(
            codex_binding_hint(Some("Codex process holds multiple process/peer thread IDs"))
                .starts_with("Conflicting")
        );
    }
}
