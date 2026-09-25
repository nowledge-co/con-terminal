use super::*;
use serde_json::json;

#[test]
fn recent_meta_is_only_a_confirmable_suggestion() {
    let root = std::env::temp_dir().join(format!("cursor-binding-{}", uuid::Uuid::new_v4()));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    assert!(select_recent(discover_at(&root, &cwd).unwrap()).is_none());
    for (id, updated) in [("older-id", 1000), ("newest-id", 2000)] {
        let chat = root.join("chats/group").join(id);
        let transcript = root.join("projects/work/agent-transcripts").join(id);
        std::fs::create_dir_all(&chat).unwrap();
        std::fs::create_dir_all(&transcript).unwrap();
        std::fs::write(
            chat.join("meta.json"),
            json!({"schemaVersion":1,
            "hasConversation":true,"cwd":cwd,"updatedAtMs":updated})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            transcript.join(format!("{id}.jsonl")),
            format!("{}\n{}", user("goal"), turn_ended("success")),
        )
        .unwrap();
    }
    let sessions = discover_at(&root, &cwd).unwrap();
    let bound = select_recent(sessions.clone()).unwrap();
    assert_eq!(bound.id, "newest-id");
    assert!(bound.requires_confirmation);
    std::fs::write(
        root.join("projects/work/agent-transcripts/newest-id/newest-id.jsonl"),
        format!("{}\n{}", user("unfinished"), turn_ended("aborted")),
    )
    .unwrap();
    let listed = discover_at(&root, &cwd).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .find(|s| s.id == "newest-id")
            .unwrap()
            .export_warning
            .as_deref()
            .unwrap()
            .contains("Interrupted")
    );
    assert_eq!(select_recent(listed).unwrap().id, "older-id");
    let tied = sessions
        .into_iter()
        .map(|mut s| {
            s.updated_at = 2000;
            s
        })
        .collect();
    assert!(select_recent(tied).is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn discovers_cli_chat_omitted_by_acp_and_filters_private_blocks() {
    let root = std::env::temp_dir().join(format!("con-cursor-cli-{}", uuid::Uuid::new_v4()));
    let cwd = root.join("workspace");
    let chat = root.join("chats/hash/native-id");
    let transcript = root.join("projects/project/agent-transcripts/native-id/native-id.jsonl");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&chat).unwrap();
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(
        chat.join("meta.json"),
        serde_json::to_vec(&json!({"schemaVersion":1,"hasConversation":true,
            "cwd":cwd,"title":"Continue work","updatedAtMs":1790130454081_i64}))
        .unwrap(),
    )
    .unwrap();
    let lines = [
        json!({"role":"user","message":{"content":[{"type":"text","text":"Fix multiply"}]}}),
        json!({"role":"assistant","message":{"content":[
            {"type":"text","text":"I will fix it."},
            {"type":"thinking","thinking":"PRIVATE_REASONING"},
            {"type":"tool_use","input":{"secret":"PRIVATE_ARGUMENT"}}]}}),
        json!({"role":"assistant","message":{"content":[{"type":"text","text":"Done"}]}}),
        json!({"type":"turn_ended","status":"success"}),
    ];
    std::fs::write(
        &transcript,
        lines
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let found = find_at(&root, &cwd).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1.id, "native-id");
    let entries = files::jsonl(&files::read(&transcript, MAX_EXPORT_BYTES).unwrap()).unwrap();
    let export = normalize(found[0].1.clone(), "test-version".into(), &entries).unwrap();
    assert_eq!(export.records.len(), 3);
    let text = serde_json::to_string(&export.records).unwrap();
    assert!(!text.contains("PRIVATE_REASONING"));
    assert!(!text.contains("PRIVATE_ARGUMENT"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_unfinished_cli_transcript() {
    let source = SourceSession {
        export_warning: None,
        agent: AgentKind::Cursor,
        id: "native-id".into(),
        store_identity: "store".into(),
        title: "test".into(),
        cwd: PathBuf::from("/tmp"),
        updated_at: 0,
    };
    assert!(
        normalize(
            source,
            "test".into(),
            &[json!({"role":"user",
        "message":{"content":[{"type":"text","text":"unfinished"}]}})]
        )
        .is_err()
    );
}

fn user(text: &str) -> Value {
    json!({"role":"user","message":{"content":[{"type":"text","text":text}]}})
}

fn assistant(text: &str) -> Value {
    json!({"role":"assistant","message":{"content":[{"type":"text","text":text}]}})
}

fn turn_ended(status: &str) -> Value {
    json!({"type":"turn_ended","status":status})
}

fn source() -> SourceSession {
    SourceSession {
        export_warning: None,
        agent: AgentKind::Cursor,
        id: "native-id".into(),
        store_identity: "store".into(),
        title: "test".into(),
        cwd: PathBuf::from("/tmp"),
        updated_at: 0,
    }
}

#[test]
fn skips_failed_middle_turn_but_keeps_completed_turns() {
    let entries = [
        user("First question"),
        assistant("First answer"),
        turn_ended("success"),
        user("Failed question"),
        assistant("Failed answer"),
        turn_ended("cancelled"),
        user("Third question"),
        assistant("Third answer"),
        turn_ended("success"),
    ];
    let export = normalize(source(), "test".into(), &entries).unwrap();
    let roles: Vec<(&str, &str, &str)> = export
        .records
        .iter()
        .map(|r| (r.turn_id.as_str(), r.role.as_str(), r.text.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("turn:1", "user", "First question"),
            ("turn:1", "assistant", "First answer"),
            ("turn:2", "user", "Third question"),
            ("turn:2", "assistant", "Third answer"),
        ]
    );
    let text = serde_json::to_string(&export.records).unwrap();
    assert!(!text.contains("Failed question"));
    assert!(!text.contains("Failed answer"));
}

#[test]
fn rejects_trailing_turn_without_turn_ended_even_after_success() {
    let entries = [
        user("Done question"),
        assistant("Done answer"),
        turn_ended("success"),
        user("Dangling question"),
    ];
    assert!(normalize(source(), "test".into(), &entries).is_err());
}

#[test]
fn exports_completed_turns_when_final_turn_failed() {
    let entries = [
        user("Done question"),
        assistant("Done answer"),
        turn_ended("success"),
        user("Aborted question"),
        assistant("Aborted answer"),
        turn_ended("error"),
    ];
    let export = normalize(source(), "test".into(), &entries).unwrap();
    assert_eq!(export.records.len(), 2);
    let text = serde_json::to_string(&export.records).unwrap();
    assert!(!text.contains("Aborted"));
}

#[test]
fn interrupted_only_history_is_distinct_from_empty_history() {
    let aborted = normalize(
        source(),
        "test".into(),
        &[user("unfinished"), turn_ended("aborted")],
    )
    .unwrap_err()
    .to_string();
    assert_eq!(aborted, INTERRUPTED);
    let empty = normalize(source(), "test".into(), &[])
        .unwrap_err()
        .to_string();
    assert_eq!(empty, "Cursor CLI transcript contains no text");
    let active = normalize(source(), "test".into(), &[user("working")]).unwrap_err();
    assert!(active.is::<ActiveTurn>());
}

#[test]
fn preflight_rejects_empty_success_invalid_and_active_transcripts() {
    let root = std::env::temp_dir().join(format!("cursor-preflight-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("history.jsonl");
    for text in [
        String::new(),
        turn_ended("success").to_string(),
        "{invalid".into(),
        format!(
            "{}\n{}\n{}",
            user("done"),
            turn_ended("success"),
            user("active")
        ),
    ] {
        std::fs::write(&path, text).unwrap();
        assert!(preflight(&path).is_err());
    }
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n{}",
            user("done"),
            turn_ended("success"),
            user("aborted"),
            turn_ended("aborted")
        ),
    )
    .unwrap();
    assert!(preflight(&path).is_ok());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_export_allows_parse_and_interrupted_fallback_but_refuses_active_turn() {
    let root = std::env::temp_dir().join(format!("cursor-export-{}", uuid::Uuid::new_v4()));
    let chat = root.join("chats/group/fixture");
    let transcript = root.join("projects/work/agent-transcripts/fixture/fixture.jsonl");
    std::fs::create_dir_all(&chat).unwrap();
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(
        chat.join("meta.json"),
        json!({"schemaVersion":1,
        "hasConversation":true,"cwd":root,"updatedAtMs":1})
        .to_string(),
    )
    .unwrap();
    for text in [
        format!("{}\n{}", user("goal"), turn_ended("aborted")),
        "{invalid".into(),
    ] {
        std::fs::write(&transcript, text).unwrap();
        let fallback = export_at(&root, &root, "fixture", "test").unwrap().unwrap();
        assert_eq!(fallback.source.id, "fixture");
        assert!(fallback.history.is_err());
    }
    std::fs::write(&transcript, user("active").to_string()).unwrap();
    assert!(matches!(export_at(&root, &root, "fixture", "test"), Err(e) if e.is::<ActiveTurn>()));
    std::fs::remove_dir_all(root).unwrap();
}
