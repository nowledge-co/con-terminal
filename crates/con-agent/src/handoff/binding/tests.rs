use super::*;

#[test]
fn kimi_continue_never_consumes_a_session_id() {
    for flag in ["-c", "--continue"] {
        let args = ["kimi", flag, "session_not_an_id_argument"].map(String::from);
        assert_eq!(explicit_session_arg(AgentKind::Kimi, &args).unwrap(), None);
    }
}

#[test]
fn deep_node_launcher_stops_at_the_script() {
    let args = ["node", "--a", "--b", "--c", "--d", "/bin/kimi", "-c"].map(String::from);
    assert_eq!(agent_from_argv(&args), Some(AgentKind::Kimi));
    let args = ["node", "/tmp/app.js", "/bin/kimi"].map(String::from);
    assert_eq!(agent_from_argv(&args), None);
}

#[test]
fn cursor_worker_has_no_explicit_session() {
    let args = ["cursor-agent", "worker", "start", "--worker-dir", "/tmp"].map(String::from);
    assert_eq!(
        explicit_session_arg(AgentKind::Cursor, &args).unwrap(),
        None
    );
}

#[test]
fn reads_only_argv_not_environment() {
    let mut bytes = 2_i32.to_ne_bytes().to_vec();
    bytes.extend_from_slice(b"/bin/kimi\0\0kimi\0--session=chosen-id\0SECRET=hidden\0");
    let args = parse_macos_procargs(&bytes).unwrap();
    assert_eq!(args, ["kimi", "--session=chosen-id"]);
}

#[cfg(target_os = "macos")]
#[test]
fn reads_real_process_argv() {
    let args = macos_process_args(std::process::id() as i32).unwrap();
    assert!(!args.is_empty());
    assert!(args[0].contains("con_agent"));
}

#[test]
fn handles_native_resume_flags_without_guessing_continue() {
    let id = "01a0cd0f-5fcb-7200-88c2-b09fc036c5a3";
    for (agent, args) in [
        (AgentKind::Cursor, vec!["cursor-agent", "--resume", id]),
        (AgentKind::Kimi, vec!["kimi", "-S", id]),
    ] {
        let args = args.into_iter().map(String::from).collect::<Vec<_>>();
        assert_eq!(
            explicit_session_arg(agent, &args).unwrap().as_deref(),
            Some(id)
        );
        assert_eq!(
            explicit_session_arg(agent, &[args[0].clone(), "--continue".into()]).unwrap(),
            None
        );
    }
}

#[test]
fn recognizes_script_launchers_without_using_prompt_arguments() {
    let args = ["node", "/Users/me/.kimi-code/bin/kimi"].map(String::from);
    assert_eq!(agent_from_argv(&args), Some(AgentKind::Kimi));
    let args = [
        "node",
        "--max-old-space-size=8192",
        "/Users/me/.local/bin/cursor-agent",
    ]
    .map(String::from);
    assert_eq!(agent_from_argv(&args), Some(AgentKind::Cursor));
    let args = ["node", "/tmp/app.js", "kimi"].map(String::from);
    assert_eq!(agent_from_argv(&args), None);
}

#[test]
fn rejects_conflicting_ids_and_prompt_text() {
    let args = [
        "kimi",
        "--session",
        "11111111-1111-1111-1111-111111111111",
        "-S",
        "22222222-2222-2222-2222-222222222222",
    ]
    .map(String::from);
    assert!(explicit_session_arg(AgentKind::Kimi, &args).is_err());
    let args = [
        "kimi",
        "--",
        "--session",
        "11111111-1111-1111-1111-111111111111",
    ]
    .map(String::from);
    assert_eq!(explicit_session_arg(AgentKind::Kimi, &args).unwrap(), None);
}

#[test]
fn kimi_banner_must_have_one_visible_id() {
    let lines = vec![
        "│ Welcome to Kimi Code! │".into(),
        "│  Session:   session_ba238da7-e8f8-4f93-a405-c671af46906a │".into(),
    ];
    assert_eq!(
        kimi_screen_session(&lines).unwrap(),
        KimiScreenSession::Id("session_ba238da7-e8f8-4f93-a405-c671af46906a".into())
    );
    assert_eq!(
        kimi_screen_session(&["No session yet".into()]).unwrap(),
        KimiScreenSession::Unknown
    );
}

#[test]
fn kimi_banner_with_empty_session_line_is_not_started() {
    let lines = vec![
        "│  ▐█▛█▛█▌  Welcome to Kimi Code! │".into(),
        "│  Directory: /tmp/project │".into(),
        "│  Session: │".into(),
        "  No session yet — one will be created on your first message.".into(),
    ];
    assert_eq!(
        kimi_screen_session(&lines).unwrap(),
        KimiScreenSession::NotStarted
    );
    assert!(kimi_session_not_started(&lines));
    // A banner that already shows an ID is bound, never "not started".
    let bound = vec![
        "│  ▐█▛█▛█▌  Welcome to Kimi Code! │".into(),
        "│  Session:   session_ba238da7-e8f8-4f93-a405-c671af46906a │".into(),
    ];
    assert!(!kimi_session_not_started(&bound));
    // Without the banner nothing is proven.
    assert!(!kimi_session_not_started(&["No session yet".into()]));
}

// KERN_PROCARGS2 layout: argc, executable path, padding, argv, then environment.
fn argv_bytes(argv: &[&str]) -> Vec<u8> {
    let mut bytes = (argv.len() as i32).to_ne_bytes().to_vec();
    bytes.extend_from_slice(b"/fixture/executable\0\0");
    for arg in argv {
        bytes.extend_from_slice(arg.as_bytes());
        bytes.push(0);
    }
    bytes.extend_from_slice(b"IGNORED=/bin/kimi\0");
    bytes
}

#[test]
fn foreground_group_identifies_agent_behind_handoff_launcher() {
    let launcher = ["con-cli", "handoff", "run", "job-1234", "--revision", "2"];
    type GroupCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], Option<AgentKind>);
    let cases: &[GroupCase<'_>] = &[
        (
            "handoff kimi",
            &launcher,
            &["kimi", "--session", "session_1234"],
            Some(AgentKind::Kimi),
        ),
        (
            "handoff kimi-code",
            &launcher,
            &["kimi-code"],
            Some(AgentKind::Kimi),
        ),
        (
            "handoff interpreter",
            &launcher,
            &["python3", "/bin/kimi"],
            Some(AgentKind::Kimi),
        ),
        (
            "manual kimi",
            &["kimi-code"],
            &["sleep", "10"],
            Some(AgentKind::Kimi),
        ),
        ("shell alone", &["zsh"], &[], None),
        ("vim with helper", &["vim"], &["sleep", "10"], None),
        ("ordinary process", &["cat"], &[], None),
        (
            "node unrelated script",
            &["node", "/tmp/app.js", "/bin/kimi"],
            &[],
            None,
        ),
        ("launcher without agent", &launcher, &["sleep", "10"], None),
    ];
    // The old leader-only path must miss the handoff launcher.
    assert_eq!(
        agent_from_argv(&parse_macos_procargs(&argv_bytes(&launcher)).unwrap()),
        None
    );
    for (label, leader, child, expected) in cases {
        let members = [*leader, *child]
            .into_iter()
            .filter(|argv| !argv.is_empty())
            .map(|argv| (Some(parse_macos_procargs(&argv_bytes(argv)).unwrap()), None));
        assert_eq!(agent_from_group_evidence(members), *expected, "{label}");
    }
}

#[test]
fn group_process_names_work_when_argv_is_unavailable() {
    for (name, expected) in [
        ("kimi", Some(AgentKind::Kimi)),
        ("kimi-code", Some(AgentKind::Kimi)),
        ("codex", Some(AgentKind::Codex)),
        ("cursor-agent", Some(AgentKind::Cursor)),
        ("con-cli", None),
        ("node", None),
        ("vim", None),
    ] {
        assert_eq!(
            agent_from_group_evidence([(None, None), (None, Some(name.into()))]),
            expected
        );
    }
    assert_eq!(agent_from_group_evidence([]), None);
}

#[test]
fn invalid_process_group_leaders_cannot_identify_an_agent() {
    for pid in [0, 999_999_999, u64::MAX] {
        assert_eq!(agent_from_process_group(pid), None);
    }
}
