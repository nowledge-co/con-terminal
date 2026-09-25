//! Cursor CLI chat metadata and completed text transcript fallback.
//! ACP session/list currently omits ordinary CLI chats, including chats created by Con.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::{cursor, file_support as files};
use crate::handoff::{AgentKind, HistoryExport, HistoryRecord, MAX_EXPORT_BYTES, SourceSession};

pub(super) fn discover(cwd: &Path) -> Result<Vec<SourceSession>> {
    discover_at(&cursor::config_root()?, cwd)
}

fn discover_at(root: &Path, cwd: &Path) -> Result<Vec<SourceSession>> {
    Ok(find_at(root, cwd)?
        .into_iter()
        .map(|(path, mut source)| {
            source.export_warning = preflight(&path).err().map(|e| e.to_string());
            source
        })
        .collect())
}

/// This is a recent-history suggestion, never proof of process ownership.
pub(crate) fn recent_binding(cwd: &Path) -> Result<Option<crate::handoff::SessionBinding>> {
    Ok(select_recent(discover(cwd)?))
}

fn select_recent(mut sessions: Vec<SourceSession>) -> Option<crate::handoff::SessionBinding> {
    sessions.retain(|s| s.export_warning.is_none());
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
    let recent = sessions.first()?;
    if recent.updated_at <= 0
        || sessions
            .get(1)
            .is_some_and(|s| s.updated_at == recent.updated_at)
    {
        return None;
    }
    Some(crate::handoff::SessionBinding {
        id: recent.id.clone(),
        evidence: "most recently updated Cursor session in this directory; confirm it is current",
        requires_confirmation: true,
    })
}

pub(super) struct FileExport {
    pub source: SourceSession,
    pub history: Result<HistoryExport>,
}

pub(super) fn export(cwd: &Path, id: &str, version: &str) -> Result<Option<FileExport>> {
    export_at(&cursor::config_root()?, cwd, id, version)
}

fn export_at(root: &Path, cwd: &Path, id: &str, version: &str) -> Result<Option<FileExport>> {
    let Some((path, source)) = files::select_exact(
        find_at(root, cwd)?,
        id,
        |entry| &entry.1.id,
        "Cursor chat ID is ambiguous",
    )?
    else {
        return Ok(None);
    };
    // Storage/identity failures stay fatal. Parse/normalization failures can
    // use ACP, carrying the exact local identity even when ACP list omits it.
    let bytes = files::read(&path, MAX_EXPORT_BYTES)?;
    let history = files::jsonl(&bytes)
        .and_then(|entries| normalize(source.clone(), version.to_owned(), &entries));
    if history.as_ref().is_err_and(|e| e.is::<ActiveTurn>()) {
        return Err(history.unwrap_err());
    }
    Ok(Some(FileExport { source, history }))
}

fn preflight(path: &Path) -> Result<()> {
    // One bounded read and a linear scan, without constructing an export,
    // serializing records or hashing their contents.
    let entries = files::jsonl(&files::read(path, MAX_EXPORT_BYTES)?)?;
    let (mut text, mut user, mut completed, mut interrupted) = (false, false, false, false);
    let mut exportable_user = false;
    for entry in &entries {
        if entry["type"] == "turn_ended" {
            if entry["status"] == "success" {
                completed = true;
                exportable_user |= user;
            } else {
                interrupted = true;
            }
            text = false;
            user = false;
        } else if matches!(entry["role"].as_str(), Some("user" | "assistant")) {
            let content = entry["message"]["content"]
                .as_array()
                .context("Unsupported Cursor CLI transcript message")?;
            let has_text = content.iter().any(|p| {
                p["type"] == "text"
                    && p["text"]
                        .as_str()
                        .is_some_and(|s| !crate::handoff::sanitize(s).trim().is_empty())
            });
            text |= content.iter().any(|p| {
                p["type"] == "text" && p["text"].as_str().is_some_and(|s| !s.trim().is_empty())
            });
            user |= has_text && entry["role"] == "user";
        }
    }
    if text {
        return Err(ActiveTurn.into());
    }
    ensure!(
        completed || !interrupted,
        "Interrupted — no completed turns"
    );
    ensure!(
        exportable_user,
        "Cursor CLI transcript contains no exportable user text"
    );
    Ok(())
}

const INTERRUPTED: &str =
    "Session was interrupted before completing a turn — no exportable history";

#[derive(Debug)]
struct ActiveTurn;
impl std::fmt::Display for ActiveTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Session has an unfinished turn — wait for completion before exporting")
    }
}
impl std::error::Error for ActiveTurn {}

fn find_at(root: &Path, cwd: &Path) -> Result<Vec<(PathBuf, SourceSession)>> {
    let cwd = cwd.canonicalize()?;
    let chats = root.join("chats");
    let projects = root.join("projects");
    let chat_groups = files::children(&chats, true)?;
    if chat_groups.is_empty() {
        return Ok(Vec::new());
    }
    let store_identity = files::identity(&chats)?;
    let mut found = Vec::new();
    for group in chat_groups {
        for session_dir in files::children(&group, true)? {
            let Some(id) = session_dir.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            files::valid_id(id)?;
            let meta = session_dir.join("meta.json");
            if !meta.exists() {
                continue;
            }
            let meta = files::json(&meta, files::METADATA_BYTES)?;
            if meta["schemaVersion"] != 1 || meta["hasConversation"] != true {
                continue;
            }
            let Some(directory) = meta["cwd"]
                .as_str()
                .and_then(|p| Path::new(p).canonicalize().ok())
            else {
                continue;
            };
            if directory != cwd {
                continue;
            }
            let Some(path) = transcript(&projects, id)? else {
                continue;
            };
            found.push((
                path,
                SourceSession {
                    export_warning: None,
                    agent: AgentKind::Cursor,
                    id: id.to_owned(),
                    store_identity: store_identity.clone(),
                    title: files::title(
                        meta["title"]
                            .as_str()
                            .filter(|title| !title.trim().is_empty())
                            .unwrap_or("Cursor CLI session"),
                    ),
                    cwd: directory,
                    updated_at: files::timestamp(&meta["updatedAtMs"]),
                },
            ));
            ensure!(
                found.len() <= files::MAX_SESSIONS,
                "Cursor storage contains too many sessions"
            );
        }
    }
    Ok(found)
}

fn transcript(projects: &Path, id: &str) -> Result<Option<PathBuf>> {
    let mut found = None;
    for project in files::children(projects, true)? {
        let transcript_dir = project.join("agent-transcripts");
        if !transcript_dir.exists() {
            continue;
        }
        files::check_dir(&transcript_dir)?;
        let session_dir = transcript_dir.join(id);
        if !session_dir.exists() {
            continue;
        }
        files::check_dir(&session_dir)?;
        let path = session_dir.join(format!("{id}.jsonl"));
        if !path.exists() {
            continue;
        }
        ensure!(found.is_none(), "Cursor transcript ID is ambiguous");
        found = Some(path);
    }
    Ok(found)
}

fn normalize(source: SourceSession, version: String, entries: &[Value]) -> Result<HistoryExport> {
    let mut records = Vec::new();
    let mut completed = false;
    let mut interrupted = false;
    let mut turn = 0usize;
    let mut omitted = 0usize;
    // Text from the turn currently being read; flushed only when the turn
    // closes with `turn_ended` status "success", discarded otherwise.
    let mut segment: Vec<(usize, &str, String)> = Vec::new();
    for (ordinal, entry) in entries.iter().enumerate() {
        if entry["type"] == "turn_ended" {
            if entry["status"] == "success" {
                completed = true;
                for (ordinal, role, text) in segment.drain(..) {
                    if role == "user" {
                        turn += 1;
                    }
                    records.push(HistoryRecord {
                        turn_id: format!("turn:{turn}"),
                        item_id: format!("transcript:{ordinal}"),
                        role: role.into(),
                        text,
                    });
                }
            } else {
                interrupted = true;
                omitted += segment.len();
                segment.clear();
            }
            continue;
        }
        let role = match entry["role"].as_str() {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => {
                omitted += 1;
                continue;
            }
        };
        let content = entry["message"]["content"]
            .as_array()
            .context("Unsupported Cursor CLI transcript message")?;
        let text = content
            .iter()
            .filter_map(|part| {
                if part["type"] == "text" {
                    part["text"].as_str()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        omitted += content.iter().filter(|part| part["type"] != "text").count();
        if !text.trim().is_empty() {
            segment.push((ordinal, role, text));
        }
    }
    if !segment.is_empty() {
        return Err(ActiveTurn.into());
    }
    ensure!(completed || !interrupted, "{INTERRUPTED}");
    let last = records
        .last()
        .map(|r| r.turn_id.clone())
        .context("Cursor CLI transcript contains no text")?;
    files::finish(
        source,
        version,
        last,
        records,
        vec![format!(
            "Cursor CLI completed transcript text only; {omitted} tool, thought, non-text or unsupported blocks omitted. Native tool results are not reconstructed."
        )],
    )
}

#[cfg(test)]
#[path = "cursor_files_tests.rs"]
mod tests;
