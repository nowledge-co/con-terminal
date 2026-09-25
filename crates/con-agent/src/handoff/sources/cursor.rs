//! Cursor ACP session/list and stopped-source session/load replay.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{
    acp::Reader,
    cli_support::{acp_update, field, finish, identity, native_id, timestamp, title},
    cursor_files,
};
use crate::handoff::{AgentKind, HistoryExport, SourceSession, protocol};

pub(super) fn config_root() -> Result<PathBuf> {
    // Verified in cursor-config/dist/paths.js and src/state/index.ts.
    Ok(std::env::var_os("CURSOR_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .filter(|v| !v.is_empty())
                .map(|v| PathBuf::from(v).join("cursor"))
        })
        .unwrap_or(
            dirs::home_dir()
                .context("Home directory unavailable")?
                .join(".cursor"),
        ))
}

fn store_identity() -> Result<String> {
    identity(AgentKind::Cursor, &config_root()?.join("chats"))
}

async fn list(reader: &mut Reader, cwd: &Path) -> Result<Vec<SourceSession>> {
    ensure!(
        reader.capabilities["sessionCapabilities"]
            .get("list")
            .is_some(),
        "Cursor does not advertise ACP session listing"
    );
    let store = store_identity()?;
    let mut found = Vec::new();
    let mut cursor = Value::Null;
    for _ in 0..20 {
        let mut params = json!({"cwd":cwd});
        if !cursor.is_null() {
            params["cursor"] = cursor.clone();
        }
        let (result, _) = reader.request("session/list", params, None).await?;
        for value in result["sessions"]
            .as_array()
            .context("Unsupported Cursor session list")?
        {
            let id = field(value, "sessionId")?;
            native_id(id)?;
            let directory = PathBuf::from(field(value, "cwd")?);
            if directory.canonicalize().ok().as_deref() != Some(cwd) {
                continue;
            }
            ensure!(
                !found.iter().any(|s: &SourceSession| s.id == id),
                "Duplicate Cursor session identity"
            );
            found.push(SourceSession {
                export_warning: None,
                agent: AgentKind::Cursor,
                id: id.into(),
                store_identity: store.clone(),
                title: title(value["title"].as_str(), "Cursor session"),
                cwd: directory,
                updated_at: timestamp(&value["updatedAt"]),
            });
            ensure!(
                found.len() <= 2000,
                "More than 2,000 matching Cursor sessions"
            );
        }
        let next = result["nextCursor"].clone();
        if next.is_null() {
            return Ok(found);
        }
        ensure!(next != cursor, "Cursor session pagination did not advance");
        cursor = next;
    }
    anyhow::bail!("Cursor session pagination exceeds the history limit")
}

pub async fn discover(cwd: &Path) -> Result<Vec<SourceSession>> {
    let cwd = cwd.canonicalize()?;
    let file_cwd = cwd.clone();
    let local = tokio::task::spawn_blocking(move || cursor_files::discover(&file_cwd)).await??;
    let exe = protocol::executable("cursor-agent")?;
    let acp = match Reader::open(&exe, &cwd).await {
        Ok(mut reader) => list(&mut reader, &cwd).await,
        Err(error) => Err(error),
    };
    let mut found = match acp {
        Ok(sessions) => sessions,
        Err(error) if !local.is_empty() => {
            log::warn!("Cursor ACP listing unavailable; using local transcripts: {error}");
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    for session in local {
        if let Some(existing) = found.iter_mut().find(|existing| existing.id == session.id) {
            // ACP listing alone does not establish exportability.
            *existing = session;
        } else {
            found.push(session);
        }
    }
    Ok(found)
}

/// Load replays persisted history; it does not prove the other native TUI is
/// idle. Con does not require a source-stop confirmation before export.
pub async fn export(cwd: &Path, id: &str) -> Result<HistoryExport> {
    native_id(id)?;
    let cwd = cwd.canonicalize()?;
    let exe = protocol::executable("cursor-agent")?;
    let version = protocol::output(&exe, &["--version"]).await?;
    let file_cwd = cwd.clone();
    let file_id = id.to_owned();
    let file_version = version.clone();
    let local = tokio::task::spawn_blocking(move || {
        cursor_files::export(&file_cwd, &file_id, &file_version)
    })
    .await??;
    let (source, history) = match local {
        Some(local) => (Some(local.source), Some(local.history)),
        None => (None, None),
    };
    with_acp_fallback(history, || export_acp(&exe, &cwd, id, version, source)).await
}

async fn with_acp_fallback<F, Fut>(
    local: Option<Result<HistoryExport>>,
    replay: F,
) -> Result<HistoryExport>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<HistoryExport>>,
{
    let original = match local {
        Some(Ok(export)) => return Ok(export),
        Some(Err(error)) => Some(error),
        None => None,
    };
    replay().await.map_err(|error| match original {
        // Keep both reasons in Display too: panel.error uses to_string().
        Some(files) => anyhow::anyhow!("{files:#}. ACP fallback failed: {error:#}"),
        None => error,
    })
}

async fn export_acp(
    exe: &Path,
    cwd: &Path,
    id: &str,
    version: String,
    source: Option<SourceSession>,
) -> Result<HistoryExport> {
    let mut reader = Reader::open(exe, cwd).await?;
    ensure!(
        reader.capabilities["loadSession"] == true,
        "Cursor does not advertise ACP history loading"
    );
    let source = match source {
        Some(source) => source,
        None => list(&mut reader, cwd)
            .await?
            .into_iter()
            .find(|s| s.id == id)
            .context("Cursor session not found for this exact working directory")?,
    };
    let (_, updates) = reader
        .request(
            "session/load",
            json!({"sessionId":id,"cwd":cwd,"mcpServers":[]}),
            Some(id),
        )
        .await?;
    let mut records = Vec::new();
    let mut skipped = 0;
    for (ordinal, update) in updates.iter().enumerate() {
        skipped += acp_update(update, ordinal, &mut records);
    }
    let last = records
        .last()
        .map(|r| r.item_id.clone())
        .unwrap_or_default();
    finish(source, version, last, records, vec![
        "Coverage: partial ACP history replay drained until the stream went quiet after session/load returned. Cursor can omit non-conversation turns or unreadable turns; replay completeness cannot be proven. Synthetic IDs are replay positions, not native turn IDs.".into(),
        "No session/prompt or authentication request is sent. File/terminal/tool requests are rejected and permission requests are cancelled. Loading can initialize Cursor session services; this is not a source activity lock.".into(),
        format!("{skipped} thought, non-text, metadata or opaque update events omitted."),
    ])
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod tests;
