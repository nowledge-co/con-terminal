//! Run the real request path with a private HOME, trap executables, and loopback
//! HTTP only. No model credentials or real user workspace are used.
#![cfg(unix)]

use con_agent::{
    AgentConfig, PaneControlCapability, ProviderConfig, ProviderKind, TerminalContext,
};
use con_core::{AgentHarness, AgentSession, Config, harness::HarnessEvent};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn isolated_workspace_privacy_cases() {
    for case in [
        "missing-key",
        "blank-key",
        "home",
        "project",
        "disabled",
        "remote",
    ] {
        let root = std::env::temp_dir().join(format!("con-privacy-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let bin = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        for command in ["git", "find"] {
            let trap = bin.join(command);
            fs::write(
                &trap,
                "#!/bin/sh\nprintf touched >> \"$CON_PRIVACY_TRAP\"\nexit 1\n",
            )
            .unwrap();
            fs::set_permissions(trap, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for directory in ["Desktop", "Documents", "project"] {
            fs::create_dir(home.join(directory)).unwrap();
        }
        fs::write(home.join("AGENTS.md"), "PRIVATE_AGENTS_SENTINEL").unwrap();
        fs::write(home.join("project/AGENTS.md"), "PRIVATE_AGENTS_SENTINEL").unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "privacy_child", "--nocapture"])
            .env("CON_PRIVACY_CASE", case)
            .env("CON_PRIVACY_ROOT", &root)
            .env("CON_PRIVACY_TRAP", root.join("unexpected-traversal"))
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("PATH", &bin)
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("OPENAI_API_KEY")
            .env_remove("FLATPAK_ID")
            .env_remove("container");
        if case == "blank-key" {
            command.env("ANTHROPIC_API_KEY", "  ");
        }
        let output = command.output().unwrap();
        let success = output.status.success();
        let trapped = root.join("unexpected-traversal").exists();
        fs::remove_dir_all(&root).unwrap();
        assert!(
            success,
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !trapped,
            "{case}: automatic context ran a traversal command"
        );
    }
}

#[test]
fn privacy_child() {
    let Ok(case) = std::env::var("CON_PRIVACY_CASE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("CON_PRIVACY_ROOT").unwrap());
    let mut config = AgentConfig::default();
    let mut context = TerminalContext::empty();
    context.cwd = Some(root.join("home").display().to_string());
    if case == "project" {
        context.cwd = Some(root.join("home/project").display().to_string());
    }
    if case == "remote" {
        context.cwd = Some("/remote/private/project".into());
        context.ssh_host = Some("remote.invalid".into());
    }
    context.recent_output = vec!["TERMINAL_VISIBLE_SENTINEL".into()];
    if case == "disabled" || matches!(case.as_str(), "missing-key" | "blank-key") {
        context
            .focused_control
            .capabilities
            .push(PaneControlCapability::QueryTmux);
    }
    config.auto_context = case != "disabled";
    let listener = if matches!(case.as_str(), "missing-key" | "blank-key") {
        None
    } else {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        config.provider = ProviderKind::OpenAICompatible;
        config.providers.set(
            &ProviderKind::OpenAICompatible,
            ProviderConfig {
                model: Some("test-only-model".into()),
                base_url: Some(format!("http://{address}/v1")),
                ..Default::default()
            },
        );
        Some(listener)
    };
    let receiver = listener.map(|listener| std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stream = loop {
            if let Ok((stream, _)) = listener.accept() { break stream; }
            assert!(Instant::now() < deadline, "no loopback model request");
            std::thread::sleep(Duration::from_millis(10));
        };
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        let (offset, length) = loop {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            assert!(bytes.len() < 1_000_000);
            if let Some(index) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..index]).to_lowercase();
                let length = headers.lines().find_map(|line| line.strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())).unwrap();
                break (index + 4, length);
            }
        };
        while bytes.len() < offset + length {
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
        }
        let payload = String::from_utf8(bytes[offset..offset + length].to_vec()).unwrap();
        let response = r#"{"error":{"message":"Fixture request captured","type":"invalid_request_error","code":"invalid_request"}}"#;
        write!(stream, "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        payload
    }));
    let harness = AgentHarness::new(&Config::default()).unwrap();
    let session = AgentSession::new();
    harness.send_message(&session, config, "A general question".into(), context);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut errors = Vec::new();
    loop {
        assert!(Instant::now() < deadline, "request did not finish");
        match session.events().recv_timeout(Duration::from_millis(100)) {
            Ok(HarnessEvent::Error(error)) => errors.push(error),
            Ok(HarnessEvent::RequestFinished { .. }) => break,
            _ => {}
        }
    }
    assert!(
        session.pane_requests().is_empty(),
        "unexpected automatic pane/tmux query"
    );
    assert!(!errors.is_empty());
    if matches!(case.as_str(), "missing-key" | "blank-key") {
        assert!(
            errors.iter().any(|error| error.contains("No API key")),
            "{errors:?}"
        );
    }
    if let Some(receiver) = receiver {
        let payload = receiver.join().unwrap();
        assert!(!payload.contains("PRIVATE_AGENTS_SENTINEL"));
        assert!(!payload.contains("Desktop"));
        assert!(!payload.contains("Documents"));
        if case == "disabled" {
            assert!(!payload.contains("TERMINAL_VISIBLE_SENTINEL"));
        } else {
            assert!(payload.contains("TERMINAL_VISIBLE_SENTINEL"));
        }
    }
}
