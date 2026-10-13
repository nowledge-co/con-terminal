use super::*;
use crate::ProviderConfig;

#[test]
fn thread_keeps_codex_model_default_and_disables_hidden_execution() {
    let params = thread_params(
        &AgentConfig::default(),
        "system",
        Path::new("/tmp"),
        json!([]),
        &json!({
            "config": {"mcp_servers": {"dangerous": {"command": "do-not-run"}, "with.dot": {"command": "do-not-run"}}}
        }),
    );
    assert!(params["model"].is_null());
    assert_eq!(params["ephemeral"], true);
    assert_eq!(params["sandbox"], "read-only");
    assert_eq!(params["approvalPolicy"], "never");
    assert_eq!(params["config"]["features.shell_tool"], false);
    assert_eq!(
        params["config"]["mcp_servers"]["dangerous"]["enabled"],
        false
    );
    assert_eq!(
        params["config"]["mcp_servers"]["with.dot"]["enabled"],
        false
    );
    assert_eq!(params["config"]["features.hooks"], false);
    assert!(params.get("apiKey").is_none());
    let mut config = AgentConfig::default();
    config.providers.set(
        &ProviderKind::CodexLocal,
        ProviderConfig {
            model: Some("configured-codex-model".into()),
            api_key: Some("must-not-forward".into()),
            base_url: Some("https://must-not-forward.example".into()),
            ..Default::default()
        },
    );
    let params = thread_params(
        &config,
        "system",
        Path::new("/tmp"),
        json!([]),
        &Value::Null,
    );
    assert_eq!(params["model"], "configured-codex-model");
    assert!(!params.to_string().contains("must-not-forward"));
}

#[test]
fn streaming_and_completed_messages_are_not_duplicated() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut out = TurnOutput::default();
    for message in [
        json!({"method": "item/agentMessage/delta", "params": {"threadId": "t", "itemId": "a", "delta": "你好"}}),
        json!({"method": "item/completed", "params": {"threadId": "t", "item": {"type": "agentMessage", "id": "a", "text": "你好"}}}),
        json!({"method": "item/completed", "params": {"threadId": "t", "item": {"type": "agentMessage", "id": "b", "text": "世界"}}}),
    ] {
        out.notification(&message, &tx, "t").unwrap();
    }
    assert_eq!(out.text, "你好世界");
    assert_eq!(
        rx.try_iter()
            .filter(|e| matches!(e, AgentEvent::Token(_)))
            .count(),
        2
    );
    assert!(!out.notification(&json!({"method": "turn/completed", "params": {"threadId": "other", "turn": {"status": "completed"}}}), &tx, "t").unwrap());
    assert!(out.notification(&json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {"status": "failed", "error": {"message": "login required"}}}}), &tx, "t").unwrap_err().to_string().contains("login required"));
}

#[test]
fn completed_messages_backfill_a_missing_unicode_suffix_once() {
    let (tx, rx) = crossbeam_channel::unbounded();
    let mut output = TurnOutput::default();
    let delta = json!({"method": "item/agentMessage/delta", "params": {"threadId": "t", "itemId": "a", "delta": "你好"}});
    let complete = json!({"method": "item/completed", "params": {"threadId": "t", "item": {"id": "a", "type": "agentMessage", "text": "你好世界"}}});
    output.notification(&delta, &tx, "t").unwrap();
    output.notification(&complete, &tx, "t").unwrap();
    output.notification(&complete, &tx, "t").unwrap();
    assert_eq!(output.text, "你好世界");
    let emitted = rx
        .try_iter()
        .filter_map(|event| match event {
            AgentEvent::Token(text) => Some(text),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(emitted, "你好世界");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_a_pending_approval_does_not_execute_the_tool() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tools = rig::tool::server::ToolServer::new()
        .tool(CountingTool(calls.clone()))
        .run();
    let (event_tx, events) = crossbeam_channel::unbounded();
    let (_approval_tx, approval_rx) = crossbeam_channel::unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let hook = ConHook::new(event_tx.clone(), approval_rx, false, cancelled.clone());
    let wait_for_approval = tokio::task::spawn_blocking(move || {
        while let Ok(event) = events.recv_timeout(Duration::from_secs(2)) {
            if matches!(event, AgentEvent::ToolApprovalNeeded { .. }) {
                cancelled.store(true, Ordering::Relaxed);
                return;
            }
        }
        panic!("Con never requested approval");
    });
    let request = json!({"id": "cancelled", "params": {"callId": "call", "tool": "terminal_exec", "arguments": {}}});
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        run_tool(&request, &tools, &hook, &event_tx),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response["result"]["success"], false);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    wait_for_approval.await.unwrap();
}

#[test]
fn history_keeps_roles_without_replaying_actions() {
    let history = history_items(vec![
        Message::user("previous question"),
        Message::assistant("previous answer"),
        Message::system("excluded"),
    ]);
    assert_eq!(history.len(), 2);
    assert_eq!(history[0]["role"], "user");
    assert_eq!(history[1]["role"], "assistant");
    assert_eq!(history[1]["content"][0]["text"], "previous answer");
}

#[derive(Clone)]
pub(super) struct CountingTool(pub(super) Arc<std::sync::atomic::AtomicUsize>);
#[derive(serde::Deserialize)]
pub(super) struct EmptyArgs {}
impl rig::tool::Tool for CountingTool {
    const NAME: &'static str = "terminal_exec";
    type Args = EmptyArgs;
    type Output = String;
    type Error = std::io::Error;
    fn description(&self) -> String {
        "Test visible execution".into()
    }
    fn parameters(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _: EmptyArgs) -> std::result::Result<String, std::io::Error> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("visible result".into())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dynamic_tools_share_con_approval_and_rig_execution() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tools = rig::tool::server::ToolServer::new()
        .tool(CountingTool(calls.clone()))
        .run();
    let (event_tx, events) = crossbeam_channel::unbounded();
    let (approval_tx, approval_rx) = crossbeam_channel::unbounded();
    let hook = ConHook::new(
        event_tx.clone(),
        approval_rx,
        false,
        Arc::new(AtomicBool::new(false)),
    );
    let request = json!({"id": "server-1", "params": {"callId": "call-1", "tool": "terminal_exec", "arguments": {}}});
    approval_tx
        .send(crate::ToolApprovalDecision {
            call_id: "call-1".into(),
            allowed: false,
            reason: Some("Denied by user".into()),
        })
        .unwrap();
    let denied = run_tool(&request, &tools, &hook, &event_tx).await.unwrap();
    assert_eq!(denied["result"]["success"], false);
    assert_eq!(
        denied["result"]["contentItems"][0]["text"],
        "Denied by user"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        events
            .try_iter()
            .any(|e| matches!(e, AgentEvent::ToolApprovalNeeded { .. }))
    );
    approval_tx
        .send(crate::ToolApprovalDecision {
            call_id: "call-1".into(),
            allowed: true,
            reason: None,
        })
        .unwrap();
    let allowed = run_tool(&request, &tools, &hook, &event_tx).await.unwrap();
    assert_eq!(allowed["id"], "server-1");
    assert_eq!(allowed["result"]["success"], true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
