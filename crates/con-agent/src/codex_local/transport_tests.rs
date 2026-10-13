use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

fn transport() -> (AppServer, tokio::io::DuplexStream) {
    let (client, peer) = tokio::io::duplex(64 * 1024);
    let (read, write) = tokio::io::split(client);
    (
        AppServer {
            child: None,
            stdin: Box::new(write),
            stdout: BufReader::new(Box::new(read)),
            next_id: 1,
            pending: VecDeque::new(),
            pending_bytes: 0,
        },
        peer,
    )
}

async fn frame(stream: &mut (impl AsyncWrite + Unpin), value: Value) {
    let bytes = format!("{value}\n");
    stream.write_all(bytes.as_bytes()).await.unwrap();
}

async fn request(stream: &mut (impl AsyncBufReadExt + Unpin)) -> Value {
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    serde_json::from_str(&line).unwrap()
}

async fn setup(
    peer: tokio::io::DuplexStream,
) -> (
    BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
    Value,
) {
    let (read, mut write) = tokio::io::split(peer);
    let mut read = BufReader::new(read);
    let init = request(&mut read).await;
    assert_eq!(init["method"], "initialize");
    assert_eq!(init["params"]["capabilities"]["experimentalApi"], true);
    frame(&mut write, json!({"id": init["id"], "result": {}})).await;
    assert_eq!(request(&mut read).await["method"], "initialized");
    let config = request(&mut read).await;
    assert_eq!(config["method"], "config/read");
    frame(
        &mut write,
        json!({"id": config["id"], "result": {"config": {}}}),
    )
    .await;
    let start = request(&mut read).await;
    assert_eq!(start["method"], "thread/start");
    assert_eq!(start["params"]["ephemeral"], true);
    assert_eq!(start["params"]["config"]["features.shell_tool"], false);
    frame(
        &mut write,
        json!({"id": start["id"], "result": {
            "thread": {"id": "t"}, "approvalPolicy": "never", "sandbox": {"type": "readOnly"}
        }}),
    )
    .await;
    let turn = request(&mut read).await;
    assert_eq!(turn["method"], "turn/start");
    (read, write, turn)
}

async fn run(server: AppServer, cancelled: Arc<AtomicBool>) -> Result<String> {
    run_with_config(server, &AgentConfig::default(), cancelled).await
}

async fn run_with_config(
    server: AppServer,
    config: &AgentConfig,
    cancelled: Arc<AtomicBool>,
) -> Result<String> {
    let (events, _rx) = crossbeam_channel::unbounded();
    let (_approval_tx, approval_rx) = crossbeam_channel::unbounded();
    let hook = ConHook::new(events.clone(), approval_rx, false, cancelled.clone());
    send_with_server(
        server,
        config,
        "system",
        vec![],
        "question",
        Path::new("/tmp"),
        rig::tool::server::ToolServer::new().run(),
        hook,
        &events,
        &cancelled,
    )
    .await
}

#[tokio::test]
async fn jsonl_turn_preserves_pre_response_text_and_declines_native_execution() {
    let (server, peer) = transport();
    let mock = tokio::spawn(async move {
        let (mut read, mut write, turn) = setup(peer).await;
        frame(&mut write, json!({"method": "item/agentMessage/delta", "params": {"threadId": "t", "itemId": "a", "delta": "early "}})).await;
        frame(
            &mut write,
            json!({"id": turn["id"], "result": {"turn": {"id": "turn-1"}}}),
        )
        .await;
        frame(&mut write, json!({"id": "native", "method": "item/commandExecution/requestApproval", "params": {"threadId": "t"}})).await;
        assert_eq!(
            request(&mut read).await,
            json!({"id": "native", "result": {"decision": "decline"}})
        );
        frame(&mut write, json!({"method": "item/agentMessage/delta", "params": {"threadId": "t", "itemId": "a", "delta": "answer"}})).await;
        frame(&mut write, json!({"method": "item/completed", "params": {"threadId": "t", "item": {"type": "agentMessage", "id": "a", "text": "early answer"}}})).await;
        frame(&mut write, json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {"status": "completed"}}})).await;
    });
    assert_eq!(
        run(server, Arc::new(AtomicBool::new(false))).await.unwrap(),
        "early answer"
    );
    mock.await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn dynamic_tool_request_returns_rig_output_on_the_wire() {
    let (server, peer) = transport();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tools = rig::tool::server::ToolServer::new()
        .tool(super::tests::CountingTool(calls.clone()))
        .run();
    let mock = tokio::spawn(async move {
        let (mut read, mut write, turn) = setup(peer).await;
        frame(&mut write, json!({"id": turn["id"], "result": {}})).await;
        frame(&mut write, json!({"id": 900, "method": "item/tool/call", "params": {"threadId": "t", "callId": "visible-1", "tool": "terminal_exec", "arguments": {}}})).await;
        let result = request(&mut read).await;
        assert_eq!(result["id"], 900);
        assert_eq!(result["result"]["success"], true);
        assert!(
            result["result"]["contentItems"][0]["text"]
                .as_str()
                .unwrap()
                .contains("visible result")
        );
        frame(&mut write, json!({"method": "item/completed", "params": {"threadId": "t", "item": {"id": "a", "type": "agentMessage", "text": "done"}}})).await;
        frame(&mut write, json!({"method": "turn/completed", "params": {"threadId": "t", "turn": {"status": "completed"}}})).await;
    });
    let (events, rx) = crossbeam_channel::unbounded();
    let (_approval_tx, approval_rx) = crossbeam_channel::unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let hook = ConHook::new(events.clone(), approval_rx, true, cancelled.clone());
    let result = send_with_server(
        server,
        &AgentConfig::default(),
        "system",
        vec![],
        "question",
        Path::new("/tmp"),
        tools,
        hook,
        &events,
        &cancelled,
    )
    .await
    .unwrap();
    assert_eq!(result, "done");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        rx.try_iter()
            .any(|e| matches!(e, AgentEvent::ToolCallComplete { .. }))
    );
    mock.await.unwrap();
}

#[tokio::test]
async fn cancellation_stops_an_idle_server_without_waiting_for_a_delta() {
    let (server, peer) = transport();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let mock = tokio::spawn(async move {
        let (mut read, mut write, turn) = setup(peer).await;
        frame(&mut write, json!({"id": turn["id"], "result": {}})).await;
        frame(&mut write, json!({"method": "item/agentMessage/delta", "params": {"threadId": "t", "itemId": "a", "delta": "partial"}})).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        flag.store(true, Ordering::Relaxed);
        let mut line = String::new();
        assert_eq!(read.read_line(&mut line).await.unwrap(), 0);
    });
    let result = tokio::time::timeout(Duration::from_secs(2), run(server, cancelled))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result, "partial");
    mock.await.unwrap();
}

#[tokio::test]
async fn transport_surfaces_eof_invalid_json_and_rpc_errors() {
    let (mut server, peer) = transport();
    drop(peer);
    assert!(
        server
            .read()
            .await
            .unwrap_err()
            .to_string()
            .contains("closed")
    );
    let (mut server, mut peer) = transport();
    peer.write_all(b"not-json\n").await.unwrap();
    assert!(
        server
            .read()
            .await
            .unwrap_err()
            .to_string()
            .contains("JSON")
    );
    let (mut server, peer) = transport();
    let mock = tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(peer);
        let mut read = BufReader::new(read);
        let req = request(&mut read).await;
        frame(
            &mut write,
            json!({"id": req["id"], "error": {"code": -32601, "message": "Update Codex"}}),
        )
        .await;
    });
    assert!(
        server
            .initialize()
            .await
            .unwrap_err()
            .to_string()
            .contains("Update Codex")
    );
    mock.await.unwrap();
}

#[tokio::test]
async fn zero_tool_budget_rejects_a_tool_before_execution_or_approval() {
    let (server, peer) = transport();
    let mock = tokio::spawn(async move {
        let (mut read, mut write, turn) = setup(peer).await;
        frame(&mut write, json!({"id": turn["id"], "result": {}})).await;
        frame(&mut write, json!({"id": 900, "method": "item/tool/call", "params": {"threadId": "t", "callId": "over-budget", "tool": "terminal_exec", "arguments": {}}})).await;
        let mut line = String::new();
        assert_eq!(read.read_line(&mut line).await.unwrap(), 0);
    });
    let mut config = AgentConfig::default();
    config.max_turns = 0;
    let error = run_with_config(server, &config, Arc::new(AtomicBool::new(false)))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("tool-call budget"));
    mock.await.unwrap();
}

#[tokio::test]
async fn setup_fails_closed_on_a_writable_sandbox() {
    let (server, peer) = transport();
    let mock = tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(peer);
        let mut read = BufReader::new(read);
        for _ in 0..4 {
            let req = request(&mut read).await;
            if req["method"] == "initialized" {
                continue;
            }
            let result = if req["method"] == "thread/start" {
                json!({"thread": {"id": "t"}, "approvalPolicy": "never", "sandbox": {"type": "workspaceWrite"}})
            } else {
                json!({"config": {}})
            };
            frame(&mut write, json!({"id": req["id"], "result": result})).await;
        }
    });
    assert!(
        run(server, Arc::new(AtomicBool::new(false)))
            .await
            .unwrap_err()
            .to_string()
            .contains("safety policy")
    );
    mock.await.unwrap();
}
