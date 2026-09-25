#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::fs;
use std::path::PathBuf;

use super::{MAX_PROCESS_CANDIDATES, MAX_PROCESS_ENTRIES, ProcessIdentity, ProcessInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stat {
    pid: u32,
    parent_pid: u32,
    process_group_id: u32,
    started_at: u64,
}

pub(super) fn read_process(pid: u32) -> Option<ProcessInfo> {
    if pid == 0 || pid > i32::MAX as u32 {
        return None;
    }

    let proc_dir = PathBuf::from("/proc").join(pid.to_string());
    let before = read_stat(&proc_dir.join("stat"), pid)?;
    let executable = fs::read_link(proc_dir.join("exe")).ok()?;
    let name = fs::read_to_string(proc_dir.join("comm"))
        .ok()?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    let after = read_stat(&proc_dir.join("stat"), pid)?;

    process_info_if_same_identity(before, after, executable, name)
}

pub(super) fn group_members(pgid: u32) -> Vec<ProcessInfo> {
    if pgid == 0 || pgid > i32::MAX as u32 {
        return Vec::new();
    }

    let entries = match fs::read_dir("/proc") {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut members = Vec::new();
    for (index, entry) in entries.flatten().enumerate() {
        if index == MAX_PROCESS_ENTRIES {
            return Vec::new();
        }
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .filter(|pid| *pid != 0 && *pid <= i32::MAX as u32)
        else {
            continue;
        };
        let Some(stat) = read_stat(&entry.path().join("stat"), pid) else {
            continue;
        };
        if stat.process_group_id != pgid {
            continue;
        }
        if members.len() == MAX_PROCESS_CANDIDATES {
            return Vec::new();
        }
        if let Some(process) = read_process(pid)
            && process.process_group_id == Some(pgid)
        {
            members.push(process);
        }
    }

    members.sort_unstable_by_key(|process| process.identity.pid);
    members
}

fn read_stat(path: &std::path::Path, expected_pid: u32) -> Option<Stat> {
    parse_stat(&fs::read_to_string(path).ok()?, expected_pid)
}

fn parse_stat(input: &str, expected_pid: u32) -> Option<Stat> {
    let open = input.find('(')?;
    let close = input.rfind(')')?;
    if close <= open || input[..open].trim().parse::<u32>().ok()? != expected_pid {
        return None;
    }

    let fields: Vec<_> = input[close + 1..].split_whitespace().collect();
    if fields.len() < 20 || fields[0].len() != 1 {
        return None;
    }
    Some(Stat {
        pid: expected_pid,
        parent_pid: fields[1].parse().ok()?,
        process_group_id: fields[2].parse().ok()?,
        started_at: fields[19].parse().ok()?,
    })
}

fn process_info_if_same_identity(
    before: Stat,
    after: Stat,
    executable: PathBuf,
    name: String,
) -> Option<ProcessInfo> {
    if before != after {
        return None;
    }
    Some(ProcessInfo {
        identity: ProcessIdentity {
            pid: after.pid,
            started_at: after.started_at,
            executable,
            name,
        },
        parent_pid: after.parent_pid,
        process_group_id: Some(after.process_group_id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat_line(pid: u32, comm: &str, ppid: u32, pgid: u32, start: u64) -> String {
        // Fields 4 through 21 are represented by ppid, pgid, and seventeen zeroes.
        format!("{pid} ({comm}) S {ppid} {pgid} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {start} 0")
    }

    #[test]
    fn parses_comm_with_asymmetric_parentheses_and_spaces() {
        let parsed = parse_stat(&stat_line(42, "a (strange) name)", 7, 9, 1234), 42).unwrap();
        assert_eq!(
            parsed,
            Stat {
                pid: 42,
                parent_pid: 7,
                process_group_id: 9,
                started_at: 1234,
            }
        );
    }

    #[test]
    fn rejects_malformed_or_mismatched_stat() {
        assert!(parse_stat("42 (unterminated S 1 2", 42).is_none());
        assert!(parse_stat("42 (name) S 1 2", 42).is_none());
        assert!(parse_stat(&stat_line(43, "name", 1, 2, 3), 42).is_none());
        assert!(parse_stat(&stat_line(42, "name", 1, 2, 3).replace(" S ", " SS "), 42).is_none());
    }

    #[test]
    fn rejects_identity_change_during_read() {
        let before = parse_stat(&stat_line(42, "old", 1, 2, 100), 42).unwrap();
        let after = parse_stat(&stat_line(42, "new", 3, 4, 101), 42).unwrap();
        assert!(
            process_info_if_same_identity(before, after, PathBuf::from("/bin/test"), "test".into())
                .is_none()
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn reads_own_process() {
        let pid = std::process::id();
        let process = read_process(pid).expect("the test process should be readable through /proc");
        assert_eq!(process.identity.pid, pid);
        assert!(process.identity.started_at > 0);
        assert!(!process.identity.executable.as_os_str().is_empty());
        assert!(!process.identity.name.is_empty());
    }
}
