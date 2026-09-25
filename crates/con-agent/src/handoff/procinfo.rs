//! Fast per-process inspection via libproc: no subprocess, no timeout.
//!
//! Handoff binding works on the terminal's *foreground process group* — the
//! set of processes the user is actually watching in this window — because
//! wrapper launchers mean the group leader is not always the Agent process.
use std::path::PathBuf;

use anyhow::{Result, ensure};

// libc 0.2's vnode_info_path is 16 bytes short of the kernel's
// vnode_fdinfowithpath, which rejects an undersized buffer with ENOMEM.
// The vip_path tail is always the struct's last MAXPATHLEN bytes, so read
// the path from the end of whatever the kernel actually wrote.
const PROC_PIDFDVNODEPATHINFO: i32 = 2;
const PROC_ALL_PIDS: u32 = 1;
const MAXPATHLEN: usize = 1024;

/// Vnode paths a process currently holds open.
pub fn process_open_paths(pid: i32) -> Result<Vec<PathBuf>> {
    use std::mem;

    ensure!(pid > 0, "Invalid process ID");
    let size =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
    ensure!(size > 0, "Cannot list process files");
    let capacity = size as usize / mem::size_of::<libc::proc_fdinfo>();
    let mut fds = vec![
        libc::proc_fdinfo {
            proc_fd: 0,
            proc_fdtype: 0
        };
        capacity
    ];
    let written =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), size) };
    ensure!(written > 0, "Cannot read process files");
    let mut paths = Vec::new();
    let mut buffer = vec![0_u8; 2048];
    for fd in fds
        .iter()
        .take(written as usize / mem::size_of::<libc::proc_fdinfo>())
    {
        if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
            continue;
        }
        let read = unsafe {
            libc::proc_pidfdinfo(
                pid,
                fd.proc_fd,
                PROC_PIDFDVNODEPATHINFO,
                buffer.as_mut_ptr().cast(),
                buffer.len() as i32,
            )
        };
        if read as usize <= MAXPATHLEN {
            continue;
        }
        let tail = read as usize - MAXPATHLEN;
        let path = unsafe { std::ffi::CStr::from_ptr(buffer[tail..].as_ptr().cast()) };
        paths.push(PathBuf::from(path.to_string_lossy().into_owned()));
    }
    Ok(paths)
}

/// Unix start time of `pid`, paired with the pid to detect reuse.
pub fn process_start_secs(pid: i32) -> Option<u64> {
    use std::mem;

    if pid <= 0 {
        return None;
    }
    let mut info = unsafe { mem::zeroed::<libc::proc_bsdinfo>() };
    let size = mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut libc::proc_bsdinfo as *mut libc::c_void,
            size,
        )
    };
    (read > 0).then_some(info.pbi_start_tvsec)
}

/// Every process in the given foreground process group.
pub fn process_group_pids(pgid: i32) -> Vec<i32> {
    use std::mem;

    let size = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    if size <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0_i32; size as usize / mem::size_of::<i32>()];
    let written = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, pids.as_mut_ptr().cast(), size) };
    if written <= 0 {
        return Vec::new();
    }
    pids.truncate(written as usize / mem::size_of::<i32>());
    pids.into_iter()
        .filter(|pid| *pid > 0)
        .filter(|pid| unsafe { libc::getpgid(*pid) } == pgid)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_own_open_files_and_group() {
        let path = std::env::temp_dir().join(format!("con-handoff-fd-{}", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        let wanted = path.canonicalize().unwrap();
        let paths = process_open_paths(std::process::id() as i32).unwrap();
        drop(file);
        std::fs::remove_file(&path).ok();
        assert!(
            paths.iter().any(|path| path == &wanted),
            "open file missing from {paths:?}"
        );
        let pgid = unsafe { libc::getpgid(0) };
        assert!(process_group_pids(pgid).contains(&(std::process::id() as i32)));
    }
}
