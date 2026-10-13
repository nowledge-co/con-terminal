//! Opt-in integration against the real CLI and a loopback-only Responses fixture.
//! No user's CODEX_HOME, credentials, terminal sessions, or paid model are used.
use super::*;
use crate::tools::TerminalExecResponse;
use tokio::net::{TcpListener, TcpStream};

async fn http_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0, "model connection ended before the request");
        bytes.extend_from_slice(&chunk[..n]);
        assert!(bytes.len() < MAX_FRAME_BYTES);
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .expect("fixture expects a bounded request body");
    assert!(length < MAX_FRAME_BYTES);
    while bytes.len() < header_end + length {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&chunk[..n]);
    }
    serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
}

async fn response(stream: &mut TcpStream, tool_call: bool) {
    let item = if tool_call {
        json!({"type": "function_call", "id": "fc_1", "call_id": "visible-call", "name": "terminal_exec", "arguments": "{\"command\":\"echo con-fixture\"}", "status": "completed"})
    } else {
        json!({"type": "message", "id": "msg_1", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "Con Codex integration works", "annotations": []}]})
    };
    let completed = json!({"id": "fixture-response", "object": "response", "status": "completed", "model": "test-model", "output": [item], "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}});
    let mut created = completed.clone();
    created["status"] = json!("in_progress");
    created["output"] = json!([]);
    let mut added = item.clone();
    added["status"] = json!("in_progress");
    let mut events = vec![
        json!({"type": "response.created", "response": created}),
        json!({"type": "response.output_item.added", "output_index": 0, "item": added}),
    ];
    if !tool_call {
        events.push(json!({"type": "response.output_text.delta", "item_id": "msg_1", "output_index": 0, "content_index": 0, "delta": "Con Codex integration works"}));
    }
    events.extend([
        json!({"type": "response.output_item.done", "output_index": 0, "item": item}),
        json!({"type": "response.completed", "response": completed}),
    ]);
    let body = events
        .into_iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect::<String>();
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.write_all(body.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires CON_CODEX_TEST_BIN pointing to a recent installed Codex CLI"]
async fn real_app_server_routes_tools_through_con_approval_and_visible_execution() {
    let binary = std::env::var_os("CON_CODEX_TEST_BIN")
        .expect("set CON_CODEX_TEST_BIN to the installed Codex CLI's absolute path");
    let fixture = super::lifecycle_tests::FixtureDir::new();
    let home = fixture.0.join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = format!(
        "model = \"test-model\"\nmodel_provider = \"local-fixture\"\n[model_providers.local-fixture]\nname = \"Con fixture\"\nbase_url = \"http://127.0.0.1:{}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\n",
        listener.local_addr().unwrap().port()
    );
    std::fs::write(home.join("config.toml"), config).unwrap();
    let model = tokio::spawn(async move {
        for index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = http_request(&mut stream).await;
            if index == 0 {
                let names = request["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|tool| tool["name"].as_str())
                    .collect::<Vec<_>>();
                assert!(names.contains(&"terminal_exec"));
                for hidden in [
                    "shell",
                    "shell_command",
                    "exec_command",
                    "apply_patch",
                    "spawn_agent",
                    "create_goal",
                ] {
                    assert!(!names.contains(&hidden), "hidden tool leaked: {hidden}");
                }
                assert!(
                    request["input"].to_string().contains("previous question"),
                    "history injection was lost"
                );
            } else {
                assert!(
                    request["input"]
                        .to_string()
                        .contains("visible fixture result"),
                    "Con tool output was not sent back to the model"
                );
            }
            response(&mut stream, index == 0).await;
        }
    });
    let mut command = Command::new(binary);
    command
        .args([
            "app-server",
            "--listen",
            "stdio://",
            "-c",
            "features.hooks=false",
            "-c",
            "features.shell_snapshot=false",
            "-c",
            "notify=[]",
        ])
        .env("CODEX_HOME", &home)
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY");
    let server = AppServer::spawn_command(command).unwrap();
    let (exec_tx, exec_rx) = crossbeam_channel::unbounded();
    let (pane_tx, _pane_rx) = crossbeam_channel::unbounded();
    let cancelled = Arc::new(AtomicBool::new(false));
    let tools =
        crate::provider::build_tool_server(fixture.0.clone(), exec_tx, pane_tx, cancelled.clone());
    let (event_tx, events) = crossbeam_channel::unbounded();
    let (approval_tx, approval_rx) = crossbeam_channel::unbounded();
    let approval = tokio::task::spawn_blocking(move || {
        loop {
            if let AgentEvent::ToolApprovalNeeded {
                call_id, tool_name, ..
            } = events.recv_timeout(Duration::from_secs(30)).unwrap()
            {
                assert_eq!(tool_name, "terminal_exec");
                assert!(
                    exec_rx.try_recv().is_err(),
                    "execution happened before approval"
                );
                approval_tx
                    .send(crate::ToolApprovalDecision {
                        call_id,
                        allowed: true,
                        reason: None,
                    })
                    .unwrap();
                break;
            }
        }
        let request = exec_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(request.command, "echo con-fixture");
        request
            .response_tx
            .send(TerminalExecResponse::completed(
                "visible fixture result".into(),
                Some(0),
            ))
            .unwrap();
        // Only the fixture responds to the visible-execution channel. No shell
        // command is run by this test or by the local model transport.
    });
    let hook = ConHook::new(event_tx.clone(), approval_rx, false, cancelled.clone());
    let result = tokio::time::timeout(
        Duration::from_secs(60),
        send_with_server(
            server,
            &AgentConfig::default(),
            "Use Con tools only.",
            vec![
                Message::user("previous question"),
                Message::assistant("previous answer"),
            ],
            "Exercise visible execution",
            &fixture.0,
            tools,
            hook,
            &event_tx,
            &cancelled,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result, "Con Codex integration works");
    model.await.unwrap();
    approval.await.unwrap();
    assert!(
        !home.join("auth.json").exists(),
        "fixture unexpectedly needed authentication"
    );
}
