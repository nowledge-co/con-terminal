use std::{future::Future, path::Path, time::Duration};

use anyhow::{Context, Result, ensure};

use super::{
    ThreadProbe, codex_home, evidence::ThreadEvidence, evidence::parse_open_thread_evidence, lsof,
    peers::codex_app_server_peer_pids,
};
use crate::handoff::procinfo::{process_group_pids, process_open_paths, process_start_secs};

pub(super) async fn probe_thread(pid: u64) -> Result<ThreadProbe> {
    ensure!(
        pid > 0 && pid <= i32::MAX as u64,
        "Invalid Codex process ID"
    );
    tokio::time::timeout(Duration::from_secs(15), inspect(pid as i32))
        .await
        .context("Codex process inspection timed out")?
}

async fn inspect(pid: i32) -> Result<ThreadProbe> {
    let home = codex_home()?;
    let start = process_start_secs(pid).context("Codex process is unavailable")?;
    let pgid = unsafe { libc::getpgid(pid) };
    ensure!(pgid > 0, "Codex process group is unavailable");
    let mut members = process_group_pids(pgid);
    members.push(pid);
    members.sort_unstable();
    members.dedup();
    // Wrapper launchers can leave the actual TUI elsewhere in this group.
    let mut peers = Vec::new();
    for member in &members {
        peers.extend(codex_app_server_peer_pids(*member).await?);
    }
    peers.sort_unstable();
    peers.dedup();
    let probe = collect_evidence(pid, &members, &peers, |owner| scan(owner, &home)).await?;
    // Don't accept files collected from a disconnected/replaced peer.
    let mut current_peers = Vec::new();
    for member in &members {
        current_peers.extend(codex_app_server_peer_pids(*member).await?);
    }
    current_peers.sort_unstable();
    current_peers.dedup();
    ensure!(
        peers == current_peers,
        "Codex app-server connection changed during inspection"
    );
    ensure!(
        process_start_secs(pid) == Some(start) && unsafe { libc::getpgid(pid) } == pgid,
        "Codex process changed during inspection"
    );
    Ok(probe)
}

async fn collect_evidence<F, Fut>(
    pid: i32,
    members: &[i32],
    peers: &[i32],
    mut scan: F,
) -> Result<ThreadProbe>
where
    F: FnMut(i32) -> Fut,
    Fut: Future<Output = Result<ThreadEvidence>>,
{
    let mut group = scan(pid).await?;
    if group.is_empty() {
        for member in members.iter().filter(|member| **member != pid) {
            group.extend(scan(*member).await?);
        }
    }
    let mut peer_evidence = Vec::new();
    for peer in peers {
        peer_evidence.push(scan(*peer).await?);
    }
    Ok(ThreadProbe {
        id: resolve(group, peer_evidence)?,
        peer_connected: !peers.is_empty(),
    })
}

async fn scan(pid: i32, home: &Path) -> Result<ThreadEvidence> {
    let start = process_start_secs(pid).context("Codex evidence process is unavailable")?;
    let owned_home = home.to_owned();
    let fast = tokio::task::spawn_blocking(move || {
        let mut found = ThreadEvidence::default();
        for path in process_open_paths(pid)? {
            found.add(&path, &owned_home);
        }
        Ok(found)
    })
    .await?;
    let found = with_fallback(fast, || async {
        let output = lsof::inspect(&["-nP", "-a", "-p", &pid.to_string(), "-Fn"]).await?;
        Ok(parse_open_thread_evidence(&output, home))
    })
    .await?;
    ensure!(
        process_start_secs(pid) == Some(start),
        "Codex evidence process changed during inspection"
    );
    Ok(found)
}

async fn with_fallback<F, Fut>(fast: Result<ThreadEvidence>, fallback: F) -> Result<ThreadEvidence>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<ThreadEvidence>>,
{
    match fast {
        Ok(found) if !found.is_empty() => return Ok(found),
        Ok(_) => {}
        Err(error) => log::warn!("Fast Codex lock scan unavailable: {error}"),
    }
    fallback().await
}

fn resolve(group: ThreadEvidence, peers: Vec<ThreadEvidence>) -> Result<Option<String>> {
    // Rollout priority applies within each evidence source. A peer must never
    // override a different thread resolved from the foreground group.
    let mut ids = Vec::new();
    ids.extend(group.current()?);
    for peer in peers {
        ids.extend(peer.current()?);
    }
    ids.sort();
    ids.dedup();
    ensure!(
        ids.len() <= 1,
        "Codex process holds multiple process/peer thread IDs"
    );
    Ok(ids.pop())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "01a0cd0f-5fcb-7200-88c2-b09fc036c5a3";
    const OTHER: &str = "01a0cd10-22ff-7444-a11a-43185ca6eb21";

    fn lock(id: &str) -> ThreadEvidence {
        parse_open_thread_evidence(
            format!("n/tmp/codex/thread-writer-locks/{id}.lock\n").as_bytes(),
            Path::new("/tmp/codex"),
        )
    }

    #[tokio::test]
    async fn tui_without_files_resolves_rollout_from_mock_lsof_peer_in_another_group() {
        let tcp = b"p10\nccodex\nf3\nn127.0.0.1:5000->127.0.0.1:6000\nTST=ESTABLISHED\np20\nccodex\nf4\nn127.0.0.1:6000->127.0.0.1:5000\nTST=ESTABLISHED\n";
        let peers = super::super::peers::parse_peer_candidates(10, tcp).unwrap();
        assert_eq!(peers, [20]);
        let result = collect_evidence(10, &[10], &peers, |pid| async move {
            with_fallback(Ok(ThreadEvidence::default()), || async move {
                let output = match pid {
                    10 => "p10\nn/tmp/unrelated\n".to_owned(),
                    20 => format!("p20\nn/tmp/codex/sessions/rollout-2026-09-25-{ID}.jsonl\n"),
                    _ => panic!("unrelated process was scanned"),
                };
                Ok(parse_open_thread_evidence(
                    output.as_bytes(),
                    Path::new("/tmp/codex"),
                ))
            })
            .await
        })
        .await
        .unwrap();
        assert_eq!(result.id.as_deref(), Some(ID));
        assert!(result.peer_connected);
    }

    #[tokio::test]
    async fn empty_libproc_and_error_both_reach_lsof() {
        for fast in [
            Ok(ThreadEvidence::default()),
            Err(anyhow::anyhow!("libproc failed")),
        ] {
            assert_eq!(
                with_fallback(fast, || async { Ok(lock(ID)) })
                    .await
                    .unwrap()
                    .current()
                    .unwrap()
                    .as_deref(),
                Some(ID)
            );
        }
        assert!(
            with_fallback(Ok(ThreadEvidence::default()), || async {
                anyhow::bail!("lsof failed")
            })
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn nonempty_libproc_does_not_call_lsof() {
        let found = with_fallback(Ok(lock(ID)), || async { panic!("unexpected lsof") })
            .await
            .unwrap();
        assert_eq!(found.current().unwrap().as_deref(), Some(ID));
    }

    #[test]
    fn empty_tui_uses_peer_rollout_and_peer_conflicts_fail_closed() {
        let rollout = || {
            parse_open_thread_evidence(
                format!("n/tmp/codex/sessions/rollout-2026-09-25-{ID}.jsonl\n").as_bytes(),
                Path::new("/tmp/codex"),
            )
        };
        assert_eq!(
            resolve(ThreadEvidence::default(), vec![rollout()])
                .unwrap()
                .as_deref(),
            Some(ID)
        );
        assert!(resolve(lock(OTHER), vec![rollout()]).is_err());
        assert!(resolve(ThreadEvidence::default(), vec![lock(ID), lock(OTHER)]).is_err());
        assert_eq!(
            resolve(lock(ID), vec![rollout()]).unwrap().as_deref(),
            Some(ID)
        );
    }
}
