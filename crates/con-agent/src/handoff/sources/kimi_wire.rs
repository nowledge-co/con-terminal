//! Kimi Code 2.0.1 wire tree / persisted text projection (protocol 1.0–1.5).
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use serde_json::Value;

use super::files;

pub(super) type Row = (usize, Value);

pub(super) fn parse(bytes: &[u8]) -> Result<Vec<Row>> {
    let text = std::str::from_utf8(bytes).context("Kimi wire is not UTF-8")?;
    let mut rows = Vec::<Row>::new();
    for (index, line) in text.lines().enumerate() {
        ensure!(index < 100_000, "Kimi wire contains too many lines");
        if !line.trim().is_empty() {
            rows.push((
                index + 1,
                serde_json::from_str(line)
                    .context("Kimi wire contains an incomplete or malformed record")?,
            ));
        }
    }
    let metadata = &rows.first().context("Kimi wire is empty")?.1;
    ensure!(
        metadata["type"] == "metadata"
            && matches!(
                metadata["protocol_version"].as_str(),
                Some("1.0" | "1.1" | "1.2" | "1.3" | "1.4" | "1.5")
            ),
        "Unsupported Kimi wire protocol; expected 1.0–1.5"
    );
    Ok(rows)
}

struct Segment {
    floor: usize,
    end: usize,
    base: Option<(String, usize)>,
}

/// Follow the persisted active branch; abandoned branches and paired legacy undo events are excluded.
pub(super) fn active(rows: &[Row]) -> Result<Vec<&Row>> {
    let mut segments = HashMap::new();
    let mut branch = "main".to_owned();
    let mut floor = 1;
    let mut base = None;
    let mut paired_undo = HashSet::new();
    for (line, value) in rows {
        if value["type"] != "agent.switched" {
            continue;
        }
        let next = files::string(value, "branch")?.to_owned();
        let parent = files::string(&value["base"], "branch")?.to_owned();
        let upto = value["base"]["line"]
            .as_u64()
            .context("Invalid Kimi branch base line")? as usize;
        ensure!(
            upto < *line && next != branch && !segments.contains_key(&next),
            "Kimi wire has an invalid or duplicate branch"
        );
        if let Some(undo) = value["legacyUndoLine"].as_u64() {
            let undo = undo as usize;
            ensure!(
                undo > *line
                    && rows
                        .iter()
                        .any(|(n, v)| *n == undo && v["type"] == "context.undo"),
                "Kimi branch has a missing paired undo"
            );
            paired_undo.insert(undo);
        }
        segments.insert(
            branch,
            Segment {
                floor,
                end: *line,
                base,
            },
        );
        branch = next;
        floor = line + 1;
        base = Some((parent, upto));
    }
    segments.insert(
        branch.clone(),
        Segment {
            floor,
            end: rows.last().map(|r| r.0).unwrap_or(0),
            base,
        },
    );
    let mut visited = HashSet::new();
    let mut ranges = Vec::new();
    let mut upto = None;
    loop {
        ensure!(
            visited.insert(branch.clone()),
            "Kimi wire branch ancestry is cyclic"
        );
        let segment = segments
            .get(&branch)
            .context("Kimi branch references missing ancestry")?;
        let end = upto.unwrap_or(segment.end);
        ensure!(
            end <= segment.end,
            "Kimi branch base escapes its parent segment"
        );
        ranges.push((segment.floor, end));
        match &segment.base {
            Some((parent, line)) => {
                branch = parent.clone();
                upto = Some(*line);
            }
            None => break,
        }
    }
    ranges.reverse();
    let mut result = Vec::new();
    for (start, end) in ranges {
        for row in rows
            .iter()
            .filter(|(line, _)| *line >= start && *line <= end)
        {
            if row.1["type"] == "agent.switched"
                || row.1["type"] == "context.undone"
                || paired_undo.contains(&row.0)
            {
                continue;
            }
            result.push(row);
        }
    }
    Ok(result)
}

/// Attributed message with an explicit undo anchor; source summary is not a fresh user request.
pub(super) struct Message {
    pub line: usize,
    pub id: String,
    pub role: &'static str,
    pub text: String,
    pub anchor: bool,
    pub summary: bool,
}

pub(super) fn message(line: usize, value: &Value) -> Option<Message> {
    let role = match value["role"].as_str()? {
        "user" => "user",
        "assistant" => "assistant",
        "tool" => "tool",
        _ => return None,
    };
    let origin = &value["origin"];
    let kind = origin["kind"].as_str();
    if role == "user"
        && !matches!(
            kind,
            None | Some("user" | "skill_activation" | "plugin_command" | "compaction_summary")
        )
    {
        return None;
    }
    let summary = kind == Some("compaction_summary");
    let text = files::text(&value["content"]);
    let text = if summary {
        format!("Historical Kimi compaction summary:\n{text}")
    } else {
        text
    };
    Some(Message {
        line,
        id: value["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("line:{line}")),
        role: if summary { "assistant" } else { role },
        text,
        summary,
        anchor: role == "user"
            && !summary
            && origin["inTurn"] != true
            && (matches!(kind, None | Some("user")) || origin["trigger"] == "user-slash"),
    })
}

pub(super) fn undo(messages: &mut Vec<Message>, count: usize) -> Result<()> {
    ensure!(count > 0, "Invalid Kimi undo count");
    let mut remaining = count;
    for (index, message) in messages.iter().enumerate().rev() {
        ensure!(
            !message.summary,
            "Kimi legacy undo crosses a compaction boundary; exact retained history is unavailable"
        );
        if message.anchor {
            remaining -= 1;
            if remaining == 0 {
                messages.truncate(index);
                return Ok(());
            }
        }
    }
    anyhow::bail!("Kimi legacy undo references missing user turns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn branch_selection_excludes_abandoned_context_and_paired_undo() {
        let rows = vec![
            (1, json!({"type":"metadata"})),
            (
                2,
                json!({"type":"context.append_message","message":{"role":"user","content":"goal"}}),
            ),
            (
                3,
                json!({"type":"context.append_message","message":{"role":"assistant","content":"abandoned"}}),
            ),
            (
                4,
                json!({"type":"agent.switched","branch":"b","base":{"branch":"main","line":2},"legacyUndoLine":5}),
            ),
            (5, json!({"type":"context.undo","count":1})),
            (
                6,
                json!({"type":"context.append_message","message":{"role":"assistant","content":"kept"}}),
            ),
        ];
        assert_eq!(
            active(&rows)
                .unwrap()
                .iter()
                .map(|r| r.0)
                .collect::<Vec<_>>(),
            [1, 2, 6]
        );
    }
}
