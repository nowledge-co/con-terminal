//! Product-specific history readers. Export is a bounded, read-only replay;
//! it never stops or controls the source Agent.
mod acp;
mod cli_support;
mod cursor;
pub(super) mod cursor_files;
mod file_support;
mod kimi;

use super::{AgentKind, HistoryExport, SourceSession};
use anyhow::{Result, ensure};
use std::path::Path;

pub async fn discover_sessions(agent: AgentKind, cwd: &Path) -> Result<Vec<SourceSession>> {
    let cwd = cwd.canonicalize()?;
    let sessions = match agent {
        AgentKind::Unknown => anyhow::bail!("Unsupported source Agent"),
        AgentKind::Codex => super::codex::discover_codex(&cwd).await?,
        AgentKind::Cursor => cursor::discover(&cwd).await?,
        AgentKind::Kimi => kimi::discover(&cwd).await?,
    };
    ensure!(
        sessions.len() <= 2_000,
        "Too many matching sessions; use an explicit session ID"
    );
    ensure!(
        sessions
            .iter()
            .all(|s| s.agent == agent && s.cwd.canonicalize().ok().as_ref() == Some(&cwd)),
        "Agent returned a session belonging to another agent or directory"
    );
    Ok(sessions)
}

/// The caller must explicitly confirm the source and its background commands are stopped.
/// Cursor may load persisted history in a separate ACP reader; no prompt is sent.
pub async fn export_session(agent: AgentKind, cwd: &Path, id: &str) -> Result<HistoryExport> {
    ensure!(
        !id.is_empty()
            && id.len() <= 512
            && !id.starts_with('-')
            && !id.chars().any(char::is_control),
        "Invalid source session ID"
    );
    let cwd = cwd.canonicalize()?;
    let export = match agent {
        AgentKind::Unknown => anyhow::bail!("Unsupported source Agent"),
        AgentKind::Codex => super::codex::export_codex(&cwd, id).await?,
        AgentKind::Cursor => cursor::export(&cwd, id).await?,
        AgentKind::Kimi => kimi::export(&cwd, id).await?,
    };
    ensure!(
        export.source.agent == agent
            && export.source.id == id
            && export.source.cwd.canonicalize()? == cwd,
        "Source identity does not match selection"
    );
    ensure!(
        export.records.iter().any(|r| r.role == "user"),
        "No exportable user goal in source history"
    );
    ensure!(
        serde_json::to_vec(&export)?.len() <= super::MAX_EXPORT_BYTES,
        "History exceeds 10 MiB export limit"
    );
    Ok(export)
}
