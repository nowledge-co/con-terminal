//! Best-effort process-name lookup for a PID.
//!
//! Tab presentation uses this to recognize interactive TUIs that leave
//! no stable text marker on their visible screen — Herdr is the first
//! one: its UI never prints "herdr", so screen-text classification can
//! never see it. The caller passes the foreground process-group ID
//! reported by the terminal backend; for a job-control foreground
//! process the group leader *is* that TUI, so its command name is the
//! signal we need.
//!
//! Failures are always soft: `None` just means "no name-based
//! classification", and callers fall back to screen-text detection.

/// Return the short command name for `pid`, when the platform allows it.
#[cfg(target_os = "macos")]
pub fn process_name(pid: u64) -> Option<String> {
    use std::ffi::c_void;

    // `proc_name` lives in libproc, which is part of libSystem. Declared
    // directly so this stays dependency-free.
    unsafe extern "C" {
        fn proc_name(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
    }

    const BUF_LEN: usize = 256;
    let mut buf = [0u8; BUF_LEN];
    let len = unsafe { proc_name(pid as i32, buf.as_mut_ptr().cast(), BUF_LEN as u32) };
    if len <= 0 {
        return None;
    }
    let len = (len as usize).min(BUF_LEN);
    let name = String::from_utf8_lossy(&buf[..len]).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Linux: the command name is the first field of `/proc/<pid>/comm`.
#[cfg(target_os = "linux")]
pub fn process_name(pid: u64) -> Option<String> {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = raw.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Other platforms: no cheap, dependency-free lookup. Callers fall back
/// to screen-text detection.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn process_name(_pid: u64) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn own_process_has_a_name() {
        let name = super::process_name(std::process::id() as u64);
        assert!(
            name.is_some_and(|n| !n.is_empty()),
            "the current process should resolve to a non-empty name"
        );
    }

    #[test]
    fn bogus_pid_resolves_to_none() {
        // Beyond any real pid_max on the supported platforms.
        assert_eq!(super::process_name(999_999_999), None);
    }
}
