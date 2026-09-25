//! Recent history is a suggestion requiring confirmation, never live evidence.
use std::{future::Future, path::Path};

use anyhow::{Result, ensure};

use super::{ThreadProbe, discover_codex, probe_thread};
use crate::handoff::{SessionBinding, SourceSession};

pub(crate) async fn binding(pid: u64, cwd: &Path) -> Result<Option<SessionBinding>> {
    bind_or_suggest(probe_thread(pid).await, || recent_binding(cwd)).await
}

async fn recent_binding(cwd: &Path) -> Result<Option<SessionBinding>> {
    Ok(select_recent(discover_codex(cwd).await?))
}

async fn bind_or_suggest<F, Fut>(
    probe: Result<ThreadProbe>,
    recent: F,
) -> Result<Option<SessionBinding>>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Option<SessionBinding>>>,
{
    let probe = probe?;
    if let Some(id) = probe.id {
        return Ok(Some(SessionBinding {
            id,
            evidence: "Codex rollout or writer lock held by this process group or its app-server peer",
            requires_confirmation: false,
        }));
    }
    let suggestion = recent().await?;
    // Preserve the missing-connection reason for the panel when there is no
    // suggestion. This is reached only AFTER a successful empty probe.
    ensure!(
        suggestion.is_some() || probe.peer_connected,
        "Codex app-server is not connected"
    );
    Ok(suggestion)
}

fn select_recent(mut sessions: Vec<SourceSession>) -> Option<SessionBinding> {
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));
    let first = sessions.first()?;
    if first.updated_at <= 0
        || sessions
            .get(1)
            .is_some_and(|s| s.updated_at == first.updated_at)
    {
        return None;
    }
    Some(SessionBinding {
        id: first.id.clone(),
        evidence: "most recently updated Codex session in this directory; confirm it is current",
        requires_confirmation: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::AgentKind;

    fn session(id: &str, updated_at: i64) -> SourceSession {
        SourceSession {
            agent: AgentKind::Codex,
            id: id.into(),
            cwd: "/tmp".into(),
            updated_at,
            title: id.into(),
            store_identity: "fixture".into(),
            export_warning: None,
        }
    }

    #[test]
    fn unique_latest_is_always_a_confirmation_only_suggestion() {
        let binding = select_recent(vec![session("old", 1), session("new", 2)]).unwrap();
        assert_eq!(binding.id, "new");
        assert!(binding.requires_confirmation);
        assert!(binding.evidence.contains("confirm it is current"));
    }

    #[test]
    fn tied_missing_or_unknown_timestamps_do_not_suggest() {
        for sessions in [
            vec![],
            vec![session("a", 0)],
            vec![session("a", -1)],
            vec![session("a", 2), session("b", 2)],
        ] {
            assert!(select_recent(sessions).is_none());
        }
    }

    #[tokio::test]
    async fn lock_wins_and_probe_errors_never_discover_recent() {
        let bound = bind_or_suggest(
            Ok(ThreadProbe {
                id: Some("lock".into()),
                peer_connected: false,
            }),
            || async { panic!("must not discover") },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(bound.id, "lock");
        assert!(!bound.requires_confirmation);
        assert!(
            bind_or_suggest(Err(anyhow::anyhow!("multiple locks")), || async {
                panic!("must not discover")
            })
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn successful_empty_probes_suggest_even_without_a_peer() {
        for peer_connected in [false, true] {
            let suggestion = bind_or_suggest(
                Ok(ThreadProbe {
                    id: None,
                    peer_connected,
                }),
                || async { Ok(select_recent(vec![session("a", 1)])) },
            )
            .await
            .unwrap()
            .unwrap();
            assert!(suggestion.requires_confirmation);
        }
    }

    #[tokio::test]
    async fn empty_recent_preserves_connection_reason_and_discovery_failure() {
        let absent = bind_or_suggest(
            Ok(ThreadProbe {
                id: None,
                peer_connected: false,
            }),
            || async { Ok(None) },
        )
        .await
        .unwrap_err();
        assert!(absent.to_string().contains("app-server is not connected"));
        assert!(
            bind_or_suggest(
                Ok(ThreadProbe {
                    id: None,
                    peer_connected: true
                }),
                || async { Ok(None) }
            )
            .await
            .unwrap()
            .is_none()
        );
        assert!(
            bind_or_suggest(
                Ok(ThreadProbe {
                    id: None,
                    peer_connected: true
                }),
                || async { anyhow::bail!("discover failed") }
            )
            .await
            .is_err()
        );
    }
}
