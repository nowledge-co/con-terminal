use super::*;
use crate::handoff::HistoryRecord;

fn fixture() -> HistoryExport {
    HistoryExport {
        source: SourceSession {
            export_warning: None,
            agent: AgentKind::Cursor,
            id: "fixture".into(),
            store_identity: "store".into(),
            title: "Fixture".into(),
            cwd: "/tmp".into(),
            updated_at: 1,
        },
        agent_version: "test".into(),
        last_turn_id: "turn:1".into(),
        records: vec![HistoryRecord {
            role: "user".into(),
            text: "Recovered goal".into(),
            turn_id: "turn:1".into(),
            item_id: "item:1".into(),
        }],
        omissions: vec![],
        digest: "fixture".into(),
    }
}

#[tokio::test]
async fn normalization_failure_calls_acp_and_recovers_history() {
    let called = std::cell::Cell::new(false);
    let recovered = with_acp_fallback(
        Some(Err(anyhow::anyhow!(
            "Session was interrupted before completing a turn — no exportable history"
        ))),
        || async {
            called.set(true);
            Ok(fixture())
        },
    )
    .await
    .unwrap();
    assert!(called.get());
    assert_eq!(recovered.records[0].text, "Recovered goal");
}

#[tokio::test]
async fn successful_files_do_not_call_acp() {
    let result = with_acp_fallback(Some(Ok(fixture())), || async {
        panic!("ACP must not run for successful file export");
    })
    .await
    .unwrap();
    assert_eq!(result.records.len(), 1);
}

#[tokio::test]
async fn missing_files_still_call_acp() {
    let result = with_acp_fallback(None, || async { Ok(fixture()) })
        .await
        .unwrap();
    assert_eq!(result.records.len(), 1);
}

#[tokio::test]
async fn acp_empty_error_and_timeout_preserve_original_reason_in_display() {
    for reason in [
        "History has no exportable user goal",
        "ACP history request timed out",
        "ACP session/load failed (code -32602)",
    ] {
        let error = with_acp_fallback(
            Some(Err(anyhow::anyhow!("Interrupted — no completed turns"))),
            || async { Err(anyhow::anyhow!(reason)) },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.starts_with("Interrupted — no completed turns"));
        assert!(error.contains(reason));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn acp_load_uses_verified_local_identity_without_requiring_list_membership() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("cursor-acp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let exe = root.join("cursor-agent");
    for replay in [true, false] {
        let update = if replay {
            "echo '{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{\"sessionId\":\"fixture\",\"update\":{\"sessionUpdate\":\"user_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"Recovered goal\"}}}}'"
        } else {
            ""
        };
        std::fs::write(&exe, format!(r##"#!/bin/sh
read -r init
echo '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":1,"agentCapabilities":{{"loadSession":true}}}}}}'
read -r load
case "$load" in
  *session/load*)
    {update}
    echo '{{"jsonrpc":"2.0","id":2,"result":{{}}}}'
    ;;
  *) exit 1 ;;
esac
"##)).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = with_acp_fallback(
            Some(Err(anyhow::anyhow!("Interrupted — no completed turns"))),
            || {
                export_acp(
                    &exe,
                    &root,
                    "fixture",
                    "test".into(),
                    Some(fixture().source),
                )
            },
        )
        .await;
        if replay {
            assert_eq!(result.unwrap().records[0].text, "Recovered goal");
        } else {
            let error = result.unwrap_err().to_string();
            assert!(error.contains("Interrupted"));
            assert!(error.contains("no exportable user goal"));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn acp_replay_updates_arriving_after_the_result_are_drained() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("cursor-acp-drain-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let exe = root.join("cursor-agent");
    // The server answers session/load first, then keeps streaming the replay:
    // the export must collect the late update instead of truncating at the
    // result.
    std::fs::write(&exe, r##"#!/bin/sh
read -r init
echo '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true}}}'
read -r load
echo '{"jsonrpc":"2.0","id":2,"result":{}}'
sleep 0.1
echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"fixture","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"Late turn"}}}}'
"##).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let export = export_acp(
        &exe,
        &root,
        "fixture",
        "test".into(),
        Some(fixture().source),
    )
    .await
    .unwrap();
    assert!(
        export
            .records
            .iter()
            .any(|record| record.text == "Late turn"),
        "late replay update must be exported: {:?}",
        export.records
    );
    std::fs::remove_dir_all(root).unwrap();
}
