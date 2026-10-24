//! Exercise the real Rig client and Con hook against an isolated gateway fixture.
use con_agent::{
    AgentConfig, AgentEvent, AgentProvider, Conversation, Message, ProviderConfig, ProviderKind,
    TerminalContext, ToolApprovalDecision,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant};

struct Request {
    headers: String,
    body: Value,
}

fn gateway(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<Request>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let thread = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "gateway request timed out");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("accept: {e}"),
                }
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let (headers, body_start, length) = loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert!(count > 0, "request ended before headers");
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (headers, end + 4, length);
                }
            };
            while bytes.len() < body_start + length {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert!(count > 0, "request ended before body");
                bytes.extend_from_slice(&buffer[..count]);
            }
            requests.push(Request {
                headers,
                body: serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap(),
            });
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        }
        requests
    });
    (base, thread)
}

fn stream(delta: Value, finish: &str) -> String {
    let chunk = |delta, finish| {
        json!({"id":"chatcmpl-fixture", "object":"chat.completion.chunk", "created":1, "model":"group/con-test", "choices":[{"index":0,"delta":delta,"finish_reason":finish}]}).to_string()
    };
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(delta, Value::Null),
        chunk(json!({}), json!(finish))
    )
}

fn tool_stream(id: &str, name: &str, args: Value) -> String {
    stream(
        json!({"role":"assistant","tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]}),
        "tool_calls",
    )
}

fn config(base: String) -> AgentConfig {
    let mut config = AgentConfig::default();
    config.select_provider(ProviderKind::Magpie);
    config.providers.magpie = Some(ProviderConfig {
        model: Some("group/con-test".into()),
        base_url: Some(base),
        api_key: Some("magpie-con".into()),
        ..ProviderConfig::default()
    });
    config
}

fn assert_requests(requests: &[Request]) {
    for request in requests {
        let headers = request.headers.to_ascii_lowercase();
        assert!(headers.starts_with("post /v1/chat/completions http/1.1\r\n"));
        assert!(headers.contains("authorization: bearer magpie-con\r\n"));
        assert!(headers.contains("user-agent: con/"));
        assert_eq!(request.body["model"], "group/con-test");
        assert_eq!(request.body["stream"], true);
    }
}

#[tokio::test]
async fn magpie_streams_lightweight_completion() {
    let (base, server) = gateway(vec![stream(
        json!({"role":"assistant","content":"gateway answer"}),
        "stop",
    )]);
    let result = AgentProvider::new(config(base))
        .complete_with_options("hello", "Be concise", 48)
        .await
        .unwrap();
    assert_eq!(result, "gateway answer");
    assert_requests(&server.join().unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn magpie_tools_keep_con_approval_and_return_tool_results() {
    let root = std::env::temp_dir().join(format!("con-magpie-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    std::fs::write(root.join("fixture.txt"), "isolated fixture").unwrap();
    let target = root.join("must-not-exist.txt");
    let (base, server) = gateway(vec![
        tool_stream(
            "read-1",
            "file_read",
            json!({"path":root.join("fixture.txt")}),
        ),
        tool_stream(
            "write-1",
            "file_write",
            json!({"path":target,"content":"denied"}),
        ),
        stream(
            json!({"role":"assistant","content":"Finished safely"}),
            "stop",
        ),
    ]);
    let mut conversation = Conversation::new();
    conversation.add_message(Message::user("Read the fixture, then ask to write a file."));
    let mut context = TerminalContext::empty();
    context.cwd = Some(root.to_string_lossy().into_owned());
    let (events_tx, events_rx) = crossbeam_channel::unbounded();
    let (approval_tx, approval_rx) = crossbeam_channel::unbounded();
    let (terminal_tx, _terminal_rx) = crossbeam_channel::unbounded();
    let (pane_tx, _pane_rx) = crossbeam_channel::unbounded();
    let observer = std::thread::spawn(move || {
        let mut completed_read = false;
        let mut denied_write = false;
        while let Ok(event) = events_rx.recv_timeout(Duration::from_secs(20)) {
            match event {
                AgentEvent::ToolCallStart {
                    call_id, tool_name, ..
                } if tool_name == "file_write" => {
                    approval_tx
                        .send(ToolApprovalDecision {
                            call_id,
                            allowed: false,
                            reason: Some("Denied by Magpie integration test".into()),
                        })
                        .unwrap();
                    denied_write = true;
                }
                AgentEvent::ToolCallComplete {
                    tool_name, result, ..
                } if tool_name == "file_read" => {
                    completed_read = result.contains("isolated fixture");
                }
                AgentEvent::Done(_) => break,
                _ => {}
            }
        }
        (completed_read, denied_write)
    });
    let result = AgentProvider::new(config(base))
        .send(
            &conversation,
            &context,
            events_tx,
            approval_rx,
            terminal_tx,
            pane_tx,
            Arc::new(AtomicBool::new(false)),
        )
        .await
        .unwrap();
    assert_eq!(result.content, "Finished safely");
    assert_eq!(observer.join().unwrap(), (true, true));
    assert!(!target.exists(), "denied tool must not write");
    let requests = server.join().unwrap();
    assert_requests(&requests);
    assert!(
        requests[1].body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool"
                && m["tool_call_id"] == "read-1"
                && m["content"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("isolated fixture"))
    );
    assert!(
        requests[2]
            .body
            .to_string()
            .contains("Denied by Magpie integration test")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn magpie_requires_a_catalog_model_instead_of_guessing() {
    let mut config = AgentConfig::default();
    config.select_provider(ProviderKind::Magpie);
    assert_eq!(
        config.effective_base_url(&ProviderKind::Magpie),
        Some(con_agent::magpie::BASE_URL)
    );
    let error = AgentProvider::new(config)
        .complete("hello")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Choose a Magpie model"));
}
