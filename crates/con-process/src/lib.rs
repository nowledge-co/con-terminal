//! Best-effort OS process facts for presentation, never authority for agent control.
//!
//! Call these blocking queries on a background worker. Birth times are opaque,
//! platform-local values; a PID alone is not a process identity. Executable paths
//! are refreshed on every observation because exec preserves PID and birth time.

use std::path::PathBuf;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(target_os = "macos")]
pub use macos::observe_pty_process;

const MAX_PROCESS_CANDIDATES: usize = 256;
#[cfg(any(target_os = "linux", target_os = "windows", test))]
const MAX_PROCESS_ENTRIES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started_at: u64,
    pub executable: PathBuf,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcessInfo {
    pub identity: ProcessIdentity,
    pub parent_pid: u32,
    pub process_group_id: Option<u32>,
}

/// Permission failures and processes that exit during a query yield no facts.
pub fn read_process(pid: u32) -> Option<ProcessInfo> {
    platform::read_process(pid)
}

/// Foreground job membership, not just the possibly exited group leader.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn group_members(pgid: u32) -> Vec<ProcessInfo> {
    platform::group_members(pgid)
}

/// Collect many jobs without rescanning Linux's process table per terminal.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn group_members_batch(pgids: &[u32]) -> Vec<Vec<ProcessInfo>> {
    #[cfg(target_os = "linux")]
    return linux::group_members_batch(pgids);
    #[cfg(target_os = "macos")]
    return pgids
        .iter()
        .map(|pgid| macos::group_members(*pgid))
        .collect();
}

/// Candidates only: Windows process ancestry does not establish foreground status.
#[cfg(target_os = "windows")]
pub fn descendants_batch(roots: &[ProcessIdentity]) -> Vec<Vec<ProcessInfo>> {
    platform::descendants_batch(roots)
}

/// Check a live Windows process incarnation against the current account.
#[cfg(target_os = "windows")]
pub fn is_current_user_process(identity: &ProcessIdentity) -> bool {
    windows::is_current_user_process(identity).unwrap_or(false)
}

#[cfg(target_os = "windows")]
pub fn current_process_is_unelevated() -> bool {
    windows::current_process_is_unelevated().unwrap_or(false)
}

/// True only when OS evidence proves the process exited or its PID was
/// reused. Inaccessible or temporarily unobservable owners stay unknown.
pub fn process_incarnation_exited(pid: u32, started_at: u64) -> bool {
    #[cfg(target_os = "windows")]
    return windows::process_incarnation_exited(pid, started_at);

    #[cfg(unix)]
    {
        if let Some(process) = read_process(pid) {
            return process.identity.started_at != started_at;
        }
        let Ok(posix_pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: a zero signal queries existence without sending a signal.
        posix_pid > 0
            && unsafe { libc::kill(posix_pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

/// Resolve an independently observed process under a known live PTY root.
/// This establishes ancestry only. Callers must still check foreground
/// membership where available and obtain launch consent separately.
#[cfg(unix)]
pub fn read_process_under_root(pid: u32, root: &ProcessIdentity) -> Option<ProcessInfo> {
    process_under_root(pid, root, read_process)
}

#[cfg(any(unix, test))]
fn process_under_root(
    pid: u32,
    root: &ProcessIdentity,
    mut read: impl FnMut(u32) -> Option<ProcessInfo>,
) -> Option<ProcessInfo> {
    const MAX_ANCESTRY_DEPTH: usize = 64;
    let leaf = read(pid)?;
    let mut current = leaf.clone();
    for _ in 0..MAX_ANCESTRY_DEPTH {
        if current.identity.pid == root.pid {
            return (current.identity.started_at == root.started_at).then_some(leaf);
        }
        if current.parent_pid == 0 || current.parent_pid == current.identity.pid {
            return None;
        }
        let parent = read(current.parent_pid)?;
        // A parent born later than the child is a reused PID. Never follow
        // that process tree even if it eventually reaches the intended root.
        if parent.identity.started_at > current.identity.started_at {
            return None;
        }
        current = parent;
    }
    None
}

#[cfg(test)]
mod resume_ancestry_tests {
    use super::*;

    fn process(pid: u32, parent_pid: u32, started_at: u64) -> ProcessInfo {
        ProcessInfo {
            identity: ProcessIdentity {
                pid,
                started_at,
                executable: PathBuf::from("/fixture"),
                name: "fixture".into(),
            },
            parent_pid,
            process_group_id: None,
        }
    }

    #[test]
    fn requires_the_same_root_incarnation_and_rejects_reused_parent_pids() {
        let root = process(1, 0, 1);
        let child = process(2, 1, 2);
        let leaf = process(3, 2, 3);
        let replacement = process(2, 1, 4);
        let unrelated = process(4, 0, 2);
        let read = |pid| {
            [&root, &child, &leaf, &unrelated]
                .into_iter()
                .find(|p| p.identity.pid == pid)
                .cloned()
        };
        assert_eq!(
            process_under_root(3, &root.identity, read),
            Some(leaf.clone())
        );
        assert!(process_under_root(4, &root.identity, read).is_none());
        let other_root = process(1, 0, 2);
        assert!(process_under_root(3, &other_root.identity, read).is_none());
        assert!(
            process_under_root(3, &root.identity, |pid| {
                if pid == 2 {
                    Some(replacement.clone())
                } else {
                    read(pid)
                }
            })
            .is_none()
        );
    }
}
