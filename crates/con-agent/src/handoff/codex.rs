use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{HistoryExport, HistoryRecord, SourceSession, digest, protocol::CodexReader, sanitize};

fn codex_home() -> Result<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
        .context("Codex storage is unavailable")?
        .canonicalize()
        .context("Codex storage cannot be resolved")
}

mod evidence;
#[cfg(target_os = "macos")]
mod lsof;
#[cfg(target_os = "macos")]
mod peers;
#[cfg(target_os = "macos")]
mod process;
pub(super) mod recent;

pub(super) struct ThreadProbe {
    pub id: Option<String>,
    pub peer_connected: bool,
}

/// Resolve process-owned evidence, including the TUI's managed app-server peer.
pub async fn active_codex_thread_id(pid: u64) -> Result<Option<String>> {
    Ok(probe_thread(pid).await?.id)
}

#[cfg(target_os = "macos")]
use process::probe_thread;

#[cfg(not(target_os = "macos"))]
pub(super) async fn probe_thread(_pid: u64) -> Result<ThreadProbe> {
    Ok(ThreadProbe {
        id: None,
        peer_connected: false,
    })
}

pub async fn discover_codex(cwd: &Path) -> Result<Vec<SourceSession>> {
    let cwd = cwd.canonicalize()?;
    let mut reader = CodexReader::open(&cwd).await?;
    let mut sessions = Vec::new();
    let mut cursor = Value::Null;
    for _ in 0..20 {
        let result = reader
            .request(
                "thread/list",
                json!({
                    "cwd":cwd,"cursor":cursor,"limit":100,"sortKey":"updated_at",
                    "sourceKinds":["cli","vscode","exec","appServer"],"modelProviders":[]
                }),
            )
            .await?;
        for thread in result["data"]
            .as_array()
            .context("Unsupported Codex session list")?
        {
            let session = parse_session(thread, &store_identity()?)?;
            if session.cwd.canonicalize().ok().as_ref() == Some(&cwd) {
                sessions.push(session);
            }
        }
        cursor = result["nextCursor"].clone();
        if cursor.is_null() {
            return Ok(sessions);
        }
    }
    anyhow::bail!("More than 2,000 matching sessions; use an explicit Codex session ID")
}

pub async fn export_codex(cwd: &Path, id: &str) -> Result<HistoryExport> {
    ensure!(!id.trim().is_empty(), "Select an exact Codex session ID");
    let cwd = cwd.canonicalize()?;
    let mut reader = CodexReader::open(&cwd).await?;
    let result = reader
        .request("thread/read", json!({"threadId":id,"includeTurns":true}))
        .await?;
    let export = normalize(&result["thread"], &store_identity()?)?;
    ensure!(export.source.id == id, "Codex returned a different session");
    ensure!(
        export.source.cwd.canonicalize()? == cwd,
        "Source session belongs to a different working directory"
    );
    Ok(export)
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .with_context(|| format!("Unsupported Codex history: missing {key}"))
}

fn store_identity() -> Result<String> {
    let home = codex_home()?;
    Ok(digest(
        format!(
            "{}:{}",
            gethostname::gethostname().to_string_lossy(),
            home.display()
        )
        .as_bytes(),
    ))
}

fn parse_session(thread: &Value, store_identity: &str) -> Result<SourceSession> {
    Ok(SourceSession {
        export_warning: None,
        agent: super::AgentKind::Codex,
        id: string(thread, "id")?.to_owned(),
        store_identity: store_identity.to_owned(),
        title: sanitize(
            thread["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| thread["preview"].as_str())
                .unwrap_or("Untitled Codex session"),
        )
        .chars()
        .take(160)
        .collect(),
        cwd: string(thread, "cwd")?.into(),
        updated_at: thread["updatedAt"].as_i64().unwrap_or_default(),
    })
}

fn normalize(thread: &Value, store_identity: &str) -> Result<HistoryExport> {
    let source = parse_session(thread, store_identity)?;
    let turns = thread["turns"]
        .as_array()
        .context("Codex did not return history")?;
    ensure!(!turns.is_empty(), "Source history is empty");
    let mut records = Vec::new();
    let mut omissions = vec![
        "Only persisted, exportable history is included; live activity is unknown.".into(),
        "Hidden reasoning, credentials, binary inputs and opaque tool arguments are excluded. Potential credential lines are redacted.".into(),
    ];
    let mut skipped = 0;
    for turn in turns {
        let turn_id = string(turn, "id")?;
        ensure!(
            turn["status"] != "inProgress",
            "Source turn is still running; stop it before preparing a handoff"
        );
        if let Some(view) = turn.get("itemsView") {
            let full = view == "full" || view["type"] == "full";
            ensure!(
                full,
                "Paginated or partial Codex history is not supported by this reader"
            );
        }
        for item in turn["items"]
            .as_array()
            .context("Missing Codex turn items")?
        {
            let kind = string(item, "type")?;
            let (role, text) = match kind {
                "userMessage" => {
                    let content = item["content"].as_array().context("Invalid user message")?;
                    let texts: Vec<_> = content
                        .iter()
                        .filter_map(|c| {
                            if c["type"] == "text" {
                                c["text"].as_str()
                            } else {
                                skipped += 1;
                                None
                            }
                        })
                        .collect();
                    ("user", texts.join("\n"))
                }
                "agentMessage" | "plan" => ("assistant", string(item, "text")?.to_owned()),
                "commandExecution" => (
                    "tool",
                    format!(
                        "Command: {}\nStatus: {}\nExit code: {}\nHistorical output:\n{}",
                        string(item, "command")?,
                        item["status"],
                        item["exitCode"],
                        item["aggregatedOutput"]
                            .as_str()
                            .unwrap_or("[output unavailable]")
                    ),
                ),
                "fileChange" => {
                    let paths: Vec<_> = item["changes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["path"].as_str())
                        .collect();
                    (
                        "tool",
                        format!(
                            "Historical file changes ({}) : {}",
                            item["status"],
                            paths.join(", ")
                        ),
                    )
                }
                "contextCompaction" => {
                    omissions.push("Source history contains compaction; pre-compaction context may be unavailable.".into());
                    continue;
                }
                _ => {
                    skipped += 1;
                    continue;
                }
            };
            if !text.is_empty() {
                records.push(HistoryRecord {
                    turn_id: turn_id.into(),
                    item_id: string(item, "id")?.into(),
                    role: role.into(),
                    text: sanitize(&text),
                });
            }
        }
    }
    ensure!(
        records.iter().any(|r| r.role == "user"),
        "History has no exportable user goal"
    );
    if skipped > 0 {
        omissions.push(format!(
            "{skipped} non-text, private or unsupported history items omitted."
        ));
    }
    let history_digest = digest(&serde_json::to_vec(&records)?);
    Ok(HistoryExport {
        source,
        agent_version: thread["cliVersion"].as_str().unwrap_or("unknown").into(),
        last_turn_id: string(turns.last().unwrap(), "id")?.into(),
        records,
        omissions,
        digest: history_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread() -> Value {
        json!({"id":"chosen","cwd":"/tmp","cliVersion":"0.155.1","turns":[
            {"id":"t1","status":"completed","items":[
                {"type":"userMessage","id":"u1","content":[{"type":"text","text":"Keep changes"}]},
                {"type":"reasoning","id":"r1","content":["private"]},
                {"type":"commandExecution","id":"c1","command":"cargo test","status":"completed","exitCode":1,"aggregatedOutput":"FAILED"},
                {"type":"contextCompaction","id":"x"}
            ]},
            {"id":"t2","status":"completed","items":[
                {"type":"userMessage","id":"u2","content":[{"type":"text","text":"Correction: only change parser"}]}
            ]}
        ]})
    }

    #[test]
    fn export_preserves_corrections_failed_tests_and_provenance() {
        let export = normalize(&thread(), "fixture").unwrap();
        assert_eq!(export.records.len(), 3);
        assert_eq!(export.records[1].role, "tool");
        assert!(export.records[1].text.contains("FAILED"));
        assert_eq!(export.records[2].item_id, "u2");
        assert!(export.records[2].text.starts_with("Correction:"));
        assert!(export.omissions.iter().any(|s| s.contains("compaction")));
        assert!(
            !serde_json::to_string(&export.records)
                .unwrap()
                .contains("private")
        );
    }

    #[test]
    fn running_and_paginated_history_fail_closed() {
        let mut data = thread();
        data["turns"][1]["status"] = json!("inProgress");
        assert!(normalize(&data, "fixture").is_err());
        data["turns"][1]["status"] = json!("completed");
        data["turns"][0]["itemsView"] = json!("partial");
        assert!(normalize(&data, "fixture").is_err());
    }
}
