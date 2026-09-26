//! Windows process identity and bounded descendant discovery.

use std::collections::{BTreeMap, HashSet};
use std::mem::size_of;
use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, ERROR_NO_MORE_FILES, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

use super::{MAX_PROCESS_CANDIDATES, MAX_PROCESS_ENTRIES, ProcessIdentity, ProcessInfo};

const MAX_IMAGE_PATH_UTF16: usize = 32_768;

struct OwnedHandle(HANDLE);

impl OwnedHandle {
    fn new(handle: HANDLE) -> Self {
        Self(handle)
    }

    fn get(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: this wrapper has unique ownership and is never cloned.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn filetime_ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

pub(super) fn read_process(pid: u32) -> Option<ProcessInfo> {
    // SAFETY: no handle inheritance is requested and `pid` is supplied by the caller.
    let handle = OwnedHandle::new(unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?
    });

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: all output pointers refer to initialized, writable FILETIME values.
    unsafe {
        GetProcessTimes(
            handle.get(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }

    if filetime_ticks(exit) != 0 {
        return None;
    }

    // QueryFullProcessImageNameW accepts the buffer size in UTF-16 code units and
    // replaces it with the number written (excluding the trailing NUL).
    let mut image = vec![0u16; MAX_IMAGE_PATH_UTF16];
    let mut image_len = u32::try_from(image.len()).ok()?;
    // SAFETY: `image` is writable for `image_len` UTF-16 code units.
    unsafe {
        QueryFullProcessImageNameW(
            handle.get(),
            Default::default(),
            PWSTR(image.as_mut_ptr()),
            &mut image_len,
        )
        .ok()?;
    }
    image.truncate(image_len as usize);
    let executable = PathBuf::from(String::from_utf16_lossy(&image));
    let name = executable.file_name()?.to_string_lossy().into_owned();

    Some(ProcessInfo {
        identity: ProcessIdentity {
            pid,
            started_at: filetime_ticks(creation),
            executable,
            name,
        },
        // Filled from the Toolhelp snapshot by `descendants`; the query APIs
        // above intentionally do not inspect the PEB.
        parent_pid: 0,
        process_group_id: None,
    })
}

/// Return conservatively validated ancestry candidates, not foreground identity.
///
/// Inaccessible processes are omitted. Creation times reject reused PIDs and
/// processes created after enumeration began. This remains best-effort metadata.
pub(super) fn descendants_batch(roots: &[ProcessIdentity]) -> Vec<Vec<ProcessInfo>> {
    if roots.is_empty() {
        return Vec::new();
    }
    let Some((children, cutoff)) = snapshot() else {
        return vec![Vec::new(); roots.len()];
    };
    roots
        .iter()
        .map(|root| {
            let same_root = || {
                read_process(root.pid)
                    .is_some_and(|process| process.identity.started_at == root.started_at)
            };
            if !same_root() {
                return Vec::new();
            }
            let result = resolve_descendants(root, &children, cutoff, read_process);
            if same_root() { result } else { Vec::new() }
        })
        .collect()
}

fn snapshot() -> Option<(BTreeMap<u32, Vec<u32>>, u64)> {
    // SAFETY: no pointer arguments. Processes newer than this cannot be matched
    // to an old snapshot entry when its original PID has since been reused.
    let cutoff = filetime_ticks(unsafe { GetSystemTimeAsFileTime() });
    // SAFETY: process snapshots ignore the process-id argument.
    let snapshot = match unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        Ok(handle) => OwnedHandle::new(handle),
        Err(_) => return None,
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // Keep only immutable snapshot relationships here; live identities are read
    // after enumeration so every accepted edge has creation-time validation.
    let mut children = BTreeMap::<u32, Vec<u32>>::new();
    let mut count = 0;
    // SAFETY: `entry` has the required dwSize and remains writable throughout.
    if unsafe { Process32FirstW(snapshot.get(), &mut entry) }.is_err() {
        return None;
    }
    loop {
        if count == MAX_PROCESS_ENTRIES {
            return None;
        }
        count += 1;
        children
            .entry(entry.th32ParentProcessID)
            .or_default()
            .push(entry.th32ProcessID);
        // SAFETY: same initialized PROCESSENTRY32W as above. Reject partial
        // snapshots on errors other than the documented end of enumeration.
        if let Err(error) = unsafe { Process32NextW(snapshot.get(), &mut entry) } {
            return (error.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0))
                .then_some((children, cutoff));
        }
    }
}

fn resolve_descendants(
    root: &ProcessIdentity,
    children: &BTreeMap<u32, Vec<u32>>,
    cutoff: u64,
    mut read: impl FnMut(u32) -> Option<ProcessInfo>,
) -> Vec<ProcessInfo> {
    let mut accepted = vec![(root.pid, root.started_at)];
    let mut seen = HashSet::from([root.pid]);
    let mut result = Vec::new();
    let mut index = 0;
    while let Some(&(parent_pid, parent_started_at)) = accepted.get(index) {
        index += 1;
        for &pid in children.get(&parent_pid).into_iter().flatten() {
            if !seen.insert(pid) {
                continue;
            }
            // Query each reachable candidate once, including inaccessible ones.
            if seen.len() > MAX_PROCESS_CANDIDATES + 1 {
                return Vec::new();
            }
            let Some(mut process) = read(pid) else {
                continue;
            };
            // A child cannot predate its parent. If the snapshot's parent PID
            // has been reused, reject this edge and therefore its whole branch.
            if process.identity.started_at < parent_started_at
                || process.identity.started_at > cutoff
            {
                continue;
            }
            process.parent_pid = parent_pid;
            accepted.push((pid, process.identity.started_at));
            result.push(process);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, start: u64) -> ProcessInfo {
        ProcessInfo {
            identity: ProcessIdentity {
                pid,
                started_at: start,
                executable: "test.exe".into(),
                name: "test.exe".into(),
            },
            parent_pid: 0,
            process_group_id: None,
        }
    }

    #[test]
    fn rejects_reused_parent_and_child_pids_without_losing_valid_descendants() {
        let root = process(10, 100).identity;
        let entries = BTreeMap::from([(12, vec![13]), (10, vec![11, 12, 14]), (11, vec![15])]);
        let result = resolve_descendants(&root, &entries, 200, |pid| {
            Some(process(
                pid,
                match pid {
                    11 => 90,
                    14 => 201,
                    _ => 150,
                },
            ))
        });
        assert_eq!(
            result.iter().map(|p| p.identity.pid).collect::<Vec<_>>(),
            vec![12, 13]
        );
        assert_eq!(result[1].parent_pid, 12);
    }
}
