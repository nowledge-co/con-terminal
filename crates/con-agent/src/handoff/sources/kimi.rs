//! Kimi Code 2.0.1 local sessions: state.json and agents/main/wire.jsonl only.
//! No CLI load/export, ZIP extraction, global diagnostic log or credential access.
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::super::{
    AgentKind, HistoryExport, HistoryRecord, MAX_EXPORT_BYTES, SourceSession, protocol,
};
use super::file_support as files;

#[path = "kimi_wire.rs"]
mod wire;

pub async fn discover(cwd: &Path) -> Result<Vec<SourceSession>> {
    let cwd = cwd.canonicalize()?;
    tokio::task::spawn_blocking(move || Ok(find(&cwd)?.into_iter().map(|(_, s)| s).collect()))
        .await?
}

pub async fn export(cwd: &Path, id: &str) -> Result<HistoryExport> {
    files::valid_id(id)?;
    let version = protocol::output(&protocol::executable("kimi")?, &["--version"]).await?;
    let cwd = cwd.canonicalize()?;
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || {
        let (dir, source) = files::select_exact(
            find(&cwd)?,
            &id,
            |entry| &entry.1.id,
            "Kimi session ID is ambiguous in this storage",
        )?
        .context("Exact Kimi Code session was not found for this directory")?;
        files::check_dir(&dir.join("agents"))?;
        files::check_dir(&dir.join("agents/main"))?;
        let path = dir.join("agents/main/wire.jsonl");
        let rows = wire::parse(&files::read(&path, MAX_EXPORT_BYTES)?)?;
        let result = normalize(source, version, &rows)?;
        let latest = state(&dir)?;
        ensure!(
            session_cwd(&latest).as_ref() == Some(&cwd),
            "Kimi session directory changed during export"
        );
        Ok(result)
    })
    .await?
}

fn state(dir: &Path) -> Result<Value> {
    let primary = dir.join("state.json");
    let path = if primary.exists() {
        primary
    } else {
        dir.join("session-meta/state.json")
    };
    files::json(&path, files::METADATA_BYTES)
}

fn session_cwd(state: &Value) -> Option<PathBuf> {
    state["cwd"]
        .as_str()
        .or_else(|| state["workDir"].as_str())
        .or_else(|| state["custom"]["cwd"].as_str())
        .filter(|p| Path::new(p).is_absolute())
        .and_then(|p| Path::new(p).canonicalize().ok())
}

fn find(cwd: &Path) -> Result<Vec<(PathBuf, SourceSession)>> {
    let root = files::home("KIMI_CODE_HOME", ".kimi-code")?.join("sessions");
    let dirs = files::children(&root, true)?;
    if dirs.is_empty() {
        return Ok(Vec::new());
    }
    let identity = files::identity(&root)?;
    let mut out = Vec::new();
    let mut count = 0;
    for workspace in dirs {
        if workspace
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        for dir in files::children(&workspace, true)? {
            count += 1;
            ensure!(
                count <= files::MAX_SESSIONS,
                "Kimi storage contains too many sessions"
            );
            if !dir.join("state.json").exists() && !dir.join("session-meta/state.json").exists() {
                continue;
            }
            let data = state(&dir)?;
            if session_cwd(&data).as_deref() != Some(cwd) || data["archived"] == true {
                continue;
            }
            let id = dir
                .file_name()
                .and_then(|s| s.to_str())
                .context("Kimi session ID is not UTF-8")?
                .to_owned();
            files::valid_id(&id)?;
            if let Some(stored_id) = data["sessionId"].as_str() {
                ensure!(
                    stored_id == id,
                    "Kimi session directory does not match its ID"
                );
            }
            let title = files::title(
                data["title"]
                    .as_str()
                    .or_else(|| data["lastPrompt"].as_str())
                    .unwrap_or("Kimi session"),
            );
            out.push((
                dir,
                SourceSession {
                    export_warning: None,
                    agent: AgentKind::Kimi,
                    id,
                    store_identity: identity.clone(),
                    title,
                    cwd: cwd.into(),
                    updated_at: files::timestamp(&data["updatedAt"]),
                },
            ));
        }
    }
    Ok(out)
}

fn normalize(source: SourceSession, version: String, rows: &[wire::Row]) -> Result<HistoryExport> {
    let chain = wire::active(rows)?;
    let mut omissions = vec![format!("Kimi main-agent persisted active branch only; {} abandoned/control records and all subagent histories are excluded.", rows.len() - chain.len()),
        "Kimi blob-backed content, injected system messages and opaque tool arguments are not read. Global logs and credentials are never accessed.".into()];
    let mut messages = Vec::<wire::Message>::new();
    let mut open_step: Option<(String, usize)> = None;
    let mut pending_tools = HashSet::new();
    let mut active_turn = false;
    let mut last = String::new();
    for (line, row) in chain {
        last = format!("line:{line}");
        if row["agentId"].as_str().is_some_and(|id| id != "main") {
            continue;
        }
        match files::string(row, "type")? {
            "turn.prompt" => active_turn = true,
            "turn.ended" => {
                active_turn = false;
                open_step = None;
            }
            "context.append_message" => {
                if let Some(message) = wire::message(*line, &row["message"]) {
                    messages.push(message);
                }
            }
            "context.append_loop_event" => {
                let event = &row["event"];
                match files::string(event, "type")? {
                    "step.begin" => {
                        let id = files::string(event, "uuid")?.to_owned();
                        messages.push(wire::Message {
                            line: *line,
                            id: id.clone(),
                            role: "assistant",
                            text: String::new(),
                            anchor: false,
                            summary: false,
                        });
                        open_step = Some((id, messages.len() - 1));
                    }
                    "content.part" => {
                        if let Some((id, position)) = &open_step {
                            ensure!(
                                event["stepUuid"] == *id,
                                "Kimi content refers to an unexpected step"
                            );
                            let part = &event["part"];
                            if part["type"] == "text"
                                && let Some(text) = part["text"].as_str()
                            {
                                messages[*position].text.push_str(text);
                            }
                        }
                    }
                    "tool.call" => {
                        pending_tools.insert(files::string(event, "toolCallId")?.to_owned());
                    }
                    "tool.result" => {
                        let id = files::string(event, "toolCallId")?.to_owned();
                        if pending_tools.remove(&id) {
                            messages.push(wire::Message {
                                line: *line,
                                id,
                                role: "tool",
                                text: files::text(&event["result"]["output"]),
                                anchor: false,
                                summary: false,
                            });
                        }
                    }
                    "step.end" => open_step = None,
                    _ => {}
                }
            }
            "context.clear" => {
                messages.clear();
                open_step = None;
                pending_tools.clear();
                omissions
                    .push("Kimi cleared its context; earlier conversation is excluded.".into());
            }
            "context.undo" => {
                wire::undo(
                    &mut messages,
                    row["count"].as_u64().context("Invalid Kimi undo count")? as usize,
                )?;
                open_step = None;
                pending_tools.clear();
                active_turn = false;
                omissions.push("Kimi legacy undo removed abandoned user turns.".into());
            }
            "context.apply_compaction" => {
                let summary = row["summary"]
                    .as_str()
                    .or_else(|| row["contextSummary"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| files::text(&row["summary"]["content"]));
                messages.push(wire::Message {
                    line: *line,
                    id: format!("line:{line}"),
                    role: "assistant",
                    text: format!("Historical Kimi compaction summary:\n{summary}"),
                    anchor: false,
                    summary: true,
                });
                open_step = None;
                omissions.push("Kimi compacted its model context. Earlier attributed messages are historical evidence, not a claim that the source model still sees them.".into());
            }
            "config.update" | "profile.bind" => {
                if let Some(cwd) = row["environmentDisclosure"]["cwd"].as_str() {
                    ensure!(
                        Path::new(cwd).canonicalize().ok().as_ref() == Some(&source.cwd),
                        "Kimi history contains another working directory; export is refused"
                    );
                }
            }
            _ => {}
        }
    }
    ensure!(
        !active_turn,
        "Kimi history has an unfinished turn; stop the source before handoff"
    );
    if !pending_tools.is_empty() {
        omissions
            .push("Some Kimi tools have no persisted result; do not assume they completed.".into());
    }
    let mut turn = String::new();
    let records = messages
        .into_iter()
        .map(|message| {
            if message.anchor {
                turn = message.id.clone();
            }
            HistoryRecord {
                turn_id: if turn.is_empty() {
                    format!("line:{}", message.line)
                } else {
                    turn.clone()
                },
                item_id: message.id,
                role: message.role.into(),
                text: message.text,
            }
        })
        .collect();
    files::finish(source, version, last, records, omissions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exports_loop_text_without_thinking_or_injections() {
        let cwd = std::env::temp_dir().canonicalize().unwrap();
        let source = SourceSession {
            export_warning: None,
            agent: AgentKind::Kimi,
            id: "chosen".into(),
            store_identity: "fixture".into(),
            title: "fixture".into(),
            cwd,
            updated_at: 0,
        };
        let entries = [
            json!({"type":"metadata","protocol_version":"1.5"}),
            json!({"type":"turn.prompt","input":"goal"}),
            json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"goal"}]}}),
            json!({"type":"context.append_loop_event","event":{"type":"step.begin","uuid":"step"}}),
            json!({"type":"context.append_loop_event","event":{"type":"content.part","stepUuid":"step","part":{"type":"thinking","text":"hidden"}}}),
            json!({"type":"context.append_loop_event","event":{"type":"content.part","stepUuid":"step","part":{"type":"text","text":"visible"}}}),
            json!({"type":"turn.ended","reason":"completed"}),
        ];
        let rows = entries
            .into_iter()
            .enumerate()
            .map(|(i, v)| (i + 1, v))
            .collect::<Vec<_>>();
        let exported = normalize(source, "2.0.1".into(), &rows).unwrap();
        assert_eq!(exported.records.len(), 2);
        assert_eq!(exported.records[1].text, "visible");
    }
}
