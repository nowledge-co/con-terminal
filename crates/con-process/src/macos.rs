//! macOS libproc queries for foreground process groups.
use std::mem::{size_of, zeroed};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use super::{MAX_PROCESS_CANDIDATES, ProcessIdentity, ProcessInfo};

// sys/proc_info.h, proc_listpids selector (not exposed by libc).
const PROC_PGRP_ONLY: u32 = 2;

fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }
    // SAFETY: proc_bsdinfo is a C structure of integers and integer arrays.
    let mut info: libc::proc_bsdinfo = unsafe { zeroed() };
    // SAFETY: the buffer has exactly the size declared to libproc.
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    (bytes == size_of::<libc::proc_bsdinfo>() as i32 && info.pbi_pid == pid).then_some(info)
}

/// Observe the same local account and controlling terminal on both sides
/// of the cwd query. A PID or a process group number alone is insufficient.
pub fn observe_pty_process(
    pid: u32,
    tty_device: u32,
    foreground: Option<u32>,
) -> Option<(u64, Option<PathBuf>)> {
    let uid = unsafe { libc::geteuid() };
    let matches_pty = |info: &libc::proc_bsdinfo| {
        info.pbi_uid == uid
            && info.pbi_ruid == uid
            && info.pbi_svuid == uid
            && info.e_tdev == tty_device
            && foreground.is_none_or(|group| info.pbi_pgid == group && info.e_tpgid == group)
    };
    let before = bsd_info(pid)?;
    if !matches_pty(&before) {
        return None;
    }

    // SAFETY: the zero-initialized C layout contains only plain data.
    let mut paths = unsafe { zeroed::<libc::proc_vnodepathinfo>() };
    let size = size_of::<libc::proc_vnodepathinfo>() as i32;
    // SAFETY: bsd_info validated pid and the output buffer matches the flavor.
    let count = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut paths as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    let cwd = (count == size)
        .then(|| {
            paths
                .pvi_cdir
                .vip_path
                .as_flattened()
                .iter()
                .take_while(|byte| **byte != 0)
                .map(|byte| *byte as u8)
                .collect()
        })
        .filter(|bytes: &Vec<u8>| !bytes.is_empty())
        .map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes)));

    let after = bsd_info(pid)?;
    if !matches_pty(&after)
        || before.pbi_start_tvsec != after.pbi_start_tvsec
        || before.pbi_start_tvusec != after.pbi_start_tvusec
    {
        return None;
    }
    let started_at = after
        .pbi_start_tvsec
        .checked_mul(1_000_000)?
        .checked_add(after.pbi_start_tvusec)?;
    Some((started_at, cwd))
}

pub(super) fn read_process(pid: u32) -> Option<ProcessInfo> {
    let before = bsd_info(pid)?;
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: pid has been range checked; path is a writable byte buffer.
    let count =
        unsafe { libc::proc_pidpath(pid as i32, path.as_mut_ptr().cast(), path.len() as u32) };
    if count <= 0 {
        return None;
    }
    path.truncate(path.iter().position(|byte| *byte == 0)?);
    let executable = PathBuf::from(std::ffi::OsString::from_vec(path));
    let after = bsd_info(pid)?;
    if before.pbi_start_tvsec != after.pbi_start_tvsec
        || before.pbi_start_tvusec != after.pbi_start_tvusec
        || before.pbi_pgid != after.pbi_pgid
    {
        return None;
    }
    let name_bytes: Vec<u8> = after
        .pbi_name
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    let name = if name_bytes.is_empty() {
        executable.file_name()?.to_string_lossy().into_owned()
    } else {
        String::from_utf8_lossy(&name_bytes).into_owned()
    };
    Some(ProcessInfo {
        identity: ProcessIdentity {
            pid,
            started_at: after
                .pbi_start_tvsec
                .checked_mul(1_000_000)?
                .checked_add(after.pbi_start_tvusec)?,
            executable,
            name,
        },
        parent_pid: after.pbi_ppid,
        process_group_id: Some(after.pbi_pgid),
    })
}

pub(super) fn group_members(pgid: u32) -> Vec<ProcessInfo> {
    if pgid == 0 || pgid > i32::MAX as u32 {
        return Vec::new();
    }
    // One extra slot distinguishes a bounded complete result from truncation.
    let mut pids = [0i32; MAX_PROCESS_CANDIDATES + 1];
    // SAFETY: the output is aligned for pid_t and the size matches its storage.
    let bytes = unsafe {
        libc::proc_listpids(
            PROC_PGRP_ONLY,
            pgid,
            pids.as_mut_ptr().cast(),
            size_of_val(&pids) as i32,
        )
    };
    if bytes <= 0 || bytes as usize >= size_of_val(&pids) {
        return Vec::new();
    }
    let mut members: Vec<_> = pids[..bytes as usize / size_of::<i32>()]
        .iter()
        .filter_map(|pid| read_process(*pid as u32))
        .filter(|process| process.process_group_id == Some(pgid))
        .collect();
    members.sort_unstable_by_key(|process| process.identity.pid);
    members
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observes_current_process_and_group() {
        let process = read_process(std::process::id()).unwrap();
        assert_eq!(
            process.identity.executable,
            std::env::current_exe().unwrap()
        );
        assert!(process.identity.started_at > 0);
        assert!(group_members(process.process_group_id.unwrap()).contains(&process));
        assert!(read_process(0).is_none());
        assert!(read_process(u32::MAX).is_none());
    }
}
