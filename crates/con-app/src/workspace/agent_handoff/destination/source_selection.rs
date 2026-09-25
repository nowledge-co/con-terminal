use con_agent::handoff::SourceSession;

/// A missing banner ID disables automatic selection, not discovered history.
pub(super) fn selected_source<'a>(
    sessions: &'a [SourceSession],
    live: Option<&str>,
    selected: Option<&str>,
    missing_banner_id: bool,
) -> Option<&'a SourceSession> {
    if let Some(id) = live.or(selected) {
        return sessions
            .iter()
            .find(|session| session.id == id && session.export_warning.is_none());
    }
    (!missing_banner_id && sessions.len() == 1 && sessions[0].export_warning.is_none())
        .then(|| &sessions[0])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_banner_keeps_discovered_sessions_available_for_explicit_selection() {
        let sessions = vec![SourceSession {
            export_warning: None,
            agent: con_agent::handoff::AgentKind::Kimi,
            id: "session_previous".into(),
            store_identity: "fixture".into(),
            title: "Previous work".into(),
            cwd: "/tmp".into(),
            updated_at: 1,
        }];
        assert!(selected_source(&sessions, None, None, true).is_none());
        assert_eq!(
            selected_source(&sessions, None, Some("session_previous"), true)
                .unwrap()
                .id,
            "session_previous"
        );
        assert!(selected_source(&[], None, None, true).is_none());
    }

    #[test]
    fn unavailable_session_stays_visible_but_cannot_be_selected_or_preselected() {
        let sessions = vec![SourceSession {
            export_warning: Some("Interrupted — no completed turns".into()),
            agent: con_agent::handoff::AgentKind::Cursor,
            id: "aborted".into(),
            store_identity: "fixture".into(),
            title: "Interrupted work".into(),
            cwd: "/tmp".into(),
            updated_at: 1,
        }];
        assert!(selected_source(&sessions, None, None, false).is_none());
        assert!(selected_source(&sessions, Some("aborted"), None, false).is_none());
        assert!(selected_source(&sessions, None, Some("aborted"), false).is_none());
        assert_eq!(sessions.len(), 1);
    }
}
