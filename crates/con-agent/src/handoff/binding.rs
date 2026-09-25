//! Bind a running native TUI to a persisted source session using process-owned evidence.
use std::collections::HashSet;

use anyhow::{Result, ensure};

use super::AgentKind;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionBinding {
    pub id: String,
    pub evidence: &'static str,
    pub requires_confirmation: bool,
}

/// Identify a local foreground Agent launcher without relying on its screen
/// banner. Script-based CLIs often have the process name `node` or `bun`.
#[cfg(target_os = "macos")]
pub fn agent_from_process_argv(pid: u64) -> Option<AgentKind> {
    let pid = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let args = macos_process_args(pid).ok()?;
    agent_from_argv(&args)
}

#[cfg(not(target_os = "macos"))]
pub fn agent_from_process_argv(_pid: u64) -> Option<AgentKind> {
    None
}

/// Identify the live Agent even when a waiting launcher owns the foreground group.
#[cfg(target_os = "macos")]
pub fn agent_from_process_group(leader_pid: u64) -> Option<AgentKind> {
    let leader = i32::try_from(leader_pid).ok().filter(|pid| *pid > 0)?;
    let pgid = unsafe { libc::getpgid(leader) };
    if pgid <= 0 {
        return None;
    }
    agent_from_group_evidence(
        process_group_args(leader, pgid).map(|(pid, args)| (args.ok(), macos_process_name(pid))),
    )
}

#[cfg(not(target_os = "macos"))]
pub fn agent_from_process_group(_leader_pid: u64) -> Option<AgentKind> {
    None
}

#[cfg(any(test, target_os = "macos"))]
fn agent_from_group_evidence(
    members: impl IntoIterator<Item = (Option<Vec<String>>, Option<String>)>,
) -> Option<AgentKind> {
    members.into_iter().find_map(|(args, name)| {
        args.as_deref()
            .and_then(agent_from_argv)
            .or_else(|| name.as_deref().and_then(agent_from_executable))
    })
}

// Share the session-binding scan, preserving leader-first identification.
#[cfg(target_os = "macos")]
fn process_group_args(leader: i32, pgid: i32) -> impl Iterator<Item = (i32, Result<Vec<String>>)> {
    std::iter::once(leader)
        .chain(
            super::procinfo::process_group_pids(pgid)
                .into_iter()
                .filter(move |pid| *pid != leader),
        )
        .map(|pid| (pid, macos_process_args(pid)))
}

#[cfg(target_os = "macos")]
fn macos_process_name(pid: i32) -> Option<String> {
    let mut bytes = [0_u8; 256];
    // SAFETY: bytes is a writable buffer of the advertised size.
    let len = unsafe { libc::proc_name(pid, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
    if len <= 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[..(len as usize).min(bytes.len())]).into_owned())
}

#[cfg(any(test, target_os = "macos"))]
fn agent_from_argv(args: &[String]) -> Option<AgentKind> {
    let command = args.first()?;
    if let Some(agent) = agent_from_executable(command) {
        return Some(agent);
    }
    let command = std::path::Path::new(command).file_name()?.to_str()?;
    if !matches!(command, "node" | "bun" | "python" | "python3") {
        return None;
    }
    args.iter()
        .skip(1)
        .take_while(|arg| arg.as_str() != "--")
        .find(|arg| arg.contains('/'))
        .and_then(|arg| agent_from_executable(arg))
}

#[cfg(any(test, target_os = "macos"))]
fn agent_from_executable(path: &str) -> Option<AgentKind> {
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    match name {
        "codex" => Some(AgentKind::Codex),
        "cursor-agent" => Some(AgentKind::Cursor),
        "kimi" | "kimi-code" => Some(AgentKind::Kimi),
        _ => None,
    }
}

pub async fn active_session_binding(
    agent: AgentKind,
    pid: u64,
    visible_screen: &[String],
    cwd: &std::path::Path,
) -> Result<Option<SessionBinding>> {
    ensure!(agent != AgentKind::Unknown, "Unsupported source Agent");
    if agent == AgentKind::Codex {
        return super::codex::recent::binding(pid, cwd).await;
    }
    let args = match process_args(pid).await {
        Ok(args) => args,
        Err(error) if agent == AgentKind::Kimi => {
            log::warn!("Cannot inspect Kimi process arguments: {error}");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    // The group leader can be a wrapper script; the Agent process itself is
    // elsewhere in the window's foreground process group.
    let from_args = match explicit_session_arg(agent, &args)? {
        Some(id) => Some(id),
        None => group_session_arg(agent, pid).await?,
    };
    let from_screen = if agent == AgentKind::Kimi {
        match kimi_screen_session(visible_screen)? {
            KimiScreenSession::Id(id) => Some(id),
            KimiScreenSession::NotStarted | KimiScreenSession::Unknown => None,
        }
    } else {
        None
    };
    if let (Some(arg), Some(screen)) = (&from_args, &from_screen) {
        ensure!(
            arg == screen,
            "Agent process and visible session ID disagree"
        );
    }
    if let Some(id) = from_screen {
        return Ok(Some(SessionBinding {
            id,
            evidence: "session ID displayed by the running Kimi TUI",
            requires_confirmation: true,
        }));
    }
    if from_args.is_none() && agent == AgentKind::Cursor {
        let cwd = cwd.to_owned();
        return tokio::task::spawn_blocking(move || {
            super::sources::cursor_files::recent_binding(&cwd)
        })
        .await?;
    }
    Ok(from_args.map(|id| SessionBinding {
        id,
        evidence: "session ID in this Agent process's launch arguments",
        requires_confirmation: true,
    }))
}

fn explicit_session_arg(agent: AgentKind, argv: &[String]) -> Result<Option<String>> {
    let flags: &[&str] = match agent {
        AgentKind::Codex => &[],
        AgentKind::Cursor => &["--resume"],
        AgentKind::Kimi => &["--session", "-S", "--resume", "-r"],
        AgentKind::Unknown => &[],
    };
    let mut ids = HashSet::new();
    let mut iter = argv.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        // Continue is a boolean, never consume the following argument as an ID.
        if agent == AgentKind::Kimi && matches!(arg.as_str(), "-c" | "--continue") {
            continue;
        }
        for flag in flags {
            let value = if arg == flag {
                if iter.peek().is_some_and(|v| valid_id(v)) {
                    Some(iter.next().unwrap().as_str())
                } else {
                    None
                }
            } else {
                arg.strip_prefix(flag)
                    .and_then(|value| value.strip_prefix('='))
                    .filter(|value| valid_id(value))
            };
            if let Some(value) = value {
                ids.insert(value.to_owned());
                break;
            }
        }
    }
    ensure!(ids.len() <= 1, "Agent process names multiple session IDs");
    Ok(ids.into_iter().next())
}

#[cfg(target_os = "macos")]
async fn group_session_arg(agent: AgentKind, leader: u64) -> Result<Option<String>> {
    let Some(leader) = i32::try_from(leader).ok().filter(|pid| *pid > 0) else {
        return Ok(None);
    };
    tokio::task::spawn_blocking(move || {
        let pgid = unsafe { libc::getpgid(leader) };
        ensure!(pgid > 0, "Agent process group is unavailable");
        let mut ids = HashSet::new();
        for (pid, args) in process_group_args(leader, pgid) {
            if pid == leader {
                continue;
            }
            let Ok(args) = args else {
                continue;
            };
            if agent_from_argv(&args) != Some(agent) {
                continue;
            }
            if let Some(id) = explicit_session_arg(agent, &args)? {
                ids.insert(id);
            }
        }
        ensure!(ids.len() <= 1, "Agent processes name multiple session IDs");
        Ok(ids.into_iter().next())
    })
    .await?
}

#[cfg(not(target_os = "macos"))]
async fn group_session_arg(_agent: AgentKind, _leader: u64) -> Result<Option<String>> {
    Ok(None)
}

fn valid_id(value: &str) -> bool {
    value.len() >= 8
        && value.len() <= 160
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// What the visible Kimi banner proves about its session.
#[derive(Clone, Debug, PartialEq, Eq)]
enum KimiScreenSession {
    /// The banner shows an exact session ID.
    Id(String),
    /// The banner is up but its `Session:` line is empty: the TUI has not
    /// created a session yet (Kimi creates one on the first message).
    NotStarted,
    /// No banner on screen; the screen proves nothing.
    Unknown,
}

fn kimi_screen_session(lines: &[String]) -> Result<KimiScreenSession> {
    if !lines
        .iter()
        .any(|line| line.contains("Welcome to Kimi Code!"))
    {
        return Ok(KimiScreenSession::Unknown);
    }
    let mut ids = HashSet::new();
    let mut saw_session_line = false;
    for line in lines {
        let line = line.trim_start_matches([' ', '│']);
        if let Some(value) = line.strip_prefix("Session:") {
            saw_session_line = true;
            let id = value.split_whitespace().next().unwrap_or_default();
            if valid_id(id) && id.starts_with("session_") {
                ids.insert(id.to_owned());
            }
        }
    }
    ensure!(ids.len() <= 1, "Kimi screen displays multiple session IDs");
    Ok(match ids.into_iter().next() {
        Some(id) => KimiScreenSession::Id(id),
        None if saw_session_line => KimiScreenSession::NotStarted,
        None => KimiScreenSession::Unknown,
    })
}

/// True when the visible Kimi banner explicitly shows an empty `Session:`
/// line: the TUI has not created a session yet. Callers must pair this with
/// "no live binding" — a resumed session can outrun the banner's first
/// paint, in which case the process arguments still carry the ID.
pub fn kimi_session_not_started(visible_screen: &[String]) -> bool {
    matches!(
        kimi_screen_session(visible_screen),
        Ok(KimiScreenSession::NotStarted)
    )
}

#[cfg(target_os = "macos")]
async fn process_args(pid: u64) -> Result<Vec<String>> {
    ensure!(
        pid > 0 && pid <= i32::MAX as u64,
        "Invalid Agent process ID"
    );
    tokio::task::spawn_blocking(move || macos_process_args(pid as i32)).await?
}

#[cfg(not(target_os = "macos"))]
async fn process_args(_pid: u64) -> Result<Vec<String>> {
    Ok(Vec::new())
}

#[cfg(target_os = "macos")]
pub(super) fn macos_process_args(pid: i32) -> Result<Vec<String>> {
    use std::io;

    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut bytes = vec![0_u8; 1024 * 1024];
    let mut len = bytes.len();
    // SAFETY: mib and buffer are valid writable allocations for this call.
    let status = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error().into());
    }
    bytes.truncate(len);
    parse_macos_procargs(&bytes)
}

#[cfg(any(test, target_os = "macos"))]
fn parse_macos_procargs(bytes: &[u8]) -> Result<Vec<String>> {
    ensure!(bytes.len() >= 4, "Agent process arguments are truncated");
    let argc = i32::from_ne_bytes(bytes[..4].try_into().unwrap());
    ensure!((1..=4096).contains(&argc), "Invalid Agent argument count");
    let mut pos = 4;
    let path_end = bytes[pos..]
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| anyhow::anyhow!("Missing executable path"))?
        + pos;
    pos = path_end + 1;
    while pos < bytes.len() && bytes[pos] == 0 {
        pos += 1;
    }
    let mut args = Vec::with_capacity(argc as usize);
    for _ in 0..argc {
        let end = bytes[pos..]
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| anyhow::anyhow!("Incomplete Agent argv"))?
            + pos;
        args.push(String::from_utf8(bytes[pos..end].to_vec())?);
        pos = end + 1;
    }
    Ok(args)
}

#[cfg(test)]
mod tests;
