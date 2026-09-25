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

const MAX_PROCESS_CANDIDATES: usize = 256;
#[cfg(any(target_os = "linux", target_os = "windows", test))]
const MAX_PROCESS_ENTRIES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started_at: u64,
    pub executable: PathBuf,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
