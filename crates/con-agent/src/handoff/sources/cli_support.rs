//! Conservative helpers shared by the CLI-backed history readers.
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::file_support as files;
use crate::handoff::{AgentKind, HistoryExport, HistoryRecord, SourceSession, sanitize};

pub(super) fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value[name]
        .as_str()
        .with_context(|| format!("Unsupported history schema: missing {name}"))
}

pub(super) fn native_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= 256
            && !id.starts_with('-')
            && !id.chars().any(|c| c.is_control()),
        "Select an exact native session ID"
    );
    Ok(())
}

pub(super) fn identity(agent: AgentKind, path: &Path) -> Result<String> {
    let root = path
        .canonicalize()
        .context("Agent history storage is unavailable")?;
    Ok(files::store_identity(&format!("{agent:?}:"), &root))
}

pub(super) fn timestamp(value: &Value) -> i64 {
    value
        .as_i64()
        .unwrap_or_else(|| files::rfc3339_seconds(value).unwrap_or_default())
}

pub(super) fn title(value: Option<&str>, fallback: &str) -> String {
    files::title(value.filter(|s| !s.trim().is_empty()).unwrap_or(fallback))
}

pub(super) fn push(
    records: &mut Vec<HistoryRecord>,
    turn: &str,
    item: &str,
    role: &str,
    text: &str,
) {
    let text = sanitize(text);
    if !text.trim().is_empty() {
        records.push(HistoryRecord {
            turn_id: turn.into(),
            item_id: item.into(),
            role: role.into(),
            text,
        });
    }
}

pub(super) fn finish(
    source: SourceSession,
    version: String,
    last_turn_id: String,
    records: Vec<HistoryRecord>,
    omissions: Vec<String>,
) -> Result<HistoryExport> {
    files::finish_records(
        source,
        version,
        last_turn_id,
        records,
        omissions,
        "Normalized history exceeds the export limit",
        [
            "Persisted history only; source activity is unknown. The user must stop the source and its background commands before export.",
            "Hidden reasoning, credentials, non-text inputs, raw tool arguments, permission decisions and configuration are omitted. Potential credential lines are redacted.",
        ],
    )
}

/// ACP text/content only. Never fall back to rawInput, rawOutput or thought chunks.
pub(super) fn acp_update(
    update: &Value,
    _ordinal: usize,
    records: &mut Vec<HistoryRecord>,
) -> usize {
    // Ignore varying metadata notification counts when assigning replay IDs.
    let ordinal = records.len();
    let item_id = format!("replay:{ordinal}");
    let turn_id = format!("replay-event:{ordinal}");
    let kind = update["sessionUpdate"].as_str().unwrap_or("");
    match kind {
        "user_message_chunk" | "agent_message_chunk" if update["content"]["type"] == "text" => {
            if let Some(text) = update["content"]["text"].as_str() {
                push(
                    records,
                    &turn_id,
                    &item_id,
                    if kind == "user_message_chunk" {
                        "user"
                    } else {
                        "assistant"
                    },
                    text,
                );
                return 0;
            }
        }
        "tool_call" | "tool_call_update" => {
            let mut texts = Vec::new();
            if let Some(title) = update["title"].as_str() {
                texts.push(title);
            }
            if let Some(status) = update["status"].as_str() {
                texts.push(status);
            }
            for content in update["content"].as_array().into_iter().flatten() {
                if content["type"] == "content"
                    && content["content"]["type"] == "text"
                    && let Some(text) = content["content"]["text"].as_str()
                {
                    texts.push(text);
                }
            }
            if !texts.is_empty() {
                let native = update["toolCallId"].as_str().unwrap_or(&item_id);
                push(
                    records,
                    &turn_id,
                    &format!("{native}:{ordinal}"),
                    "tool",
                    &texts.join("\n"),
                );
                return 0;
            }
        }
        _ => {}
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replay_omits_private_payloads_and_stabilizes_ids_across_metadata() {
        let user = json!({"sessionUpdate":"user_message_chunk", "content":{"type":"text","text":"Fix parser"}});
        let thought = json!({"sessionUpdate":"agent_thought_chunk", "content":{"type":"text","text":"PRIVATE_REASONING"}});
        let tool = json!({"sessionUpdate":"tool_call_update", "toolCallId":"native-tool-1", "rawInput":{"key":"PRIVATE_ARGUMENT"}, "rawOutput":"PRIVATE_OUTPUT",
            "content":[{"type":"content","content":{"type":"text","text":"Test failed: expected 3"}}]});
        let mut a = Vec::new();
        acp_update(&user, 0, &mut a);
        acp_update(&thought, 1, &mut a);
        acp_update(&tool, 2, &mut a);
        let mut b = Vec::new();
        acp_update(&user, 5, &mut b);
        acp_update(&tool, 99, &mut b);
        assert_eq!(a, b);
        assert_eq!(a[1].role, "tool");
        assert!(a[1].item_id.starts_with("native-tool-1:"));
        assert!(!serde_json::to_string(&a).unwrap().contains("PRIVATE"));
    }
}
