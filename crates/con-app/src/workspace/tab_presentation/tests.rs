use super::*;

#[cfg(test)]
mod tests_editor_tab_title {
    use super::*;

    #[test]
    fn test_basename_extraction() {
        let path = std::path::Path::new("/a/b/main.rs");
        assert_eq!(editor_tab_title(Some(path)), "main.rs");
    }

    #[test]
    fn test_file_without_extension() {
        let path = std::path::Path::new("/a/b/Makefile");
        assert_eq!(editor_tab_title(Some(path)), "Makefile");
    }

    #[test]
    fn test_none_returns_editor() {
        assert_eq!(editor_tab_title(None), "Editor");
    }

    #[test]
    fn test_path_ending_with_slash_returns_dir_name() {
        // Trailing slash means file_name() returns the directory name ("b"), not empty
        let path = std::path::Path::new("/a/b/");
        assert_eq!(editor_tab_title(Some(path)), "b");
    }

    #[test]
    fn test_root_path() {
        let path = std::path::Path::new("/");
        assert_eq!(editor_tab_title(Some(path)), "Editor");
    }

    #[test]
    fn test_hidden_file() {
        let path = std::path::Path::new("/a/b/.gitignore");
        assert_eq!(editor_tab_title(Some(path)), ".gitignore");
    }
}

#[cfg(test)]
mod tests_smart_tab_presentation_editor_only {
    use super::*;

    #[test]
    fn editor_only_tab_uses_file_code_icon_and_title() {
        let p =
            smart_tab_presentation(None, None, None, None, None, Some("main.rs"), None, 0, true);
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "main.rs");
        assert_eq!(p.subtitle, None);
        assert!(!p.is_ssh);
    }

    #[test]
    fn editor_only_tab_falls_back_to_editor_name() {
        let p = smart_tab_presentation(None, None, None, None, None, None, None, 2, true);
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "Editor");
    }

    #[test]
    fn editor_only_tab_respects_user_label() {
        let p = smart_tab_presentation(
            Some("My Notes"),
            None,
            None,
            None,
            None,
            Some("main.rs"),
            None,
            0,
            true,
        );
        assert_eq!(p.name, "My Notes");
        assert_eq!(p.icon, "phosphor/file-code.svg");
    }

    #[test]
    fn terminal_tab_keeps_terminal_icon_when_editor_only_flag_false() {
        let p = smart_tab_presentation(None, None, None, None, None, None, None, 0, false);
        assert_eq!(p.icon, "phosphor/terminal.svg");
    }
}

#[cfg(test)]
mod tests_agent_cli_icon {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn maps_known_agent_clis_and_falls_back() {
        assert_eq!(agent_cli_icon(Some("codex")), Some("agents/codex.svg"));
        assert_eq!(agent_cli_icon(Some("claude")), Some("agents/claude.svg"));
        assert_eq!(
            agent_cli_icon(Some("opencode")),
            Some("agents/opencode.svg")
        );
        assert_eq!(agent_cli_icon(Some("gemini")), Some("agents/gemini.svg"));
        assert_eq!(agent_cli_icon(Some("herdr")), Some("agents/herdr.svg"));
        assert_eq!(agent_cli_icon(Some("kimi")), Some("agents/kimi.svg"));
        assert_eq!(agent_cli_icon(Some("cursor")), Some("agents/cursor.svg"));
        assert_eq!(agent_cli_icon(Some("grok")), Some("agents/grok.svg"));
        assert_eq!(agent_cli_icon(Some("pi")), Some("agents/pi.svg"));
        assert_eq!(agent_cli_icon(Some("mimo")), Some("agents/mimo.svg"));
        assert_eq!(agent_cli_icon(Some("qoder")), Some("agents/qoder.svg"));
        assert_eq!(agent_cli_icon(Some("droid")), Some("agents/droid.svg"));
        assert_eq!(agent_cli_icon(Some("hermes")), Some("agents/hermes.svg"));
        assert_eq!(agent_cli_icon(Some("kiro")), Some("agents/kiro.svg"));
        assert_eq!(agent_cli_icon(Some("cline")), Some("agents/cline.svg"));
        assert_eq!(agent_cli_icon(Some("qwen")), Some("agents/qwen.svg"));
        assert_eq!(agent_cli_icon(Some("amp")), Some("agents/amp.svg"));
        assert_eq!(agent_cli_icon(Some("kilo")), Some("agents/kilo.svg"));
        assert_eq!(agent_cli_icon(Some("goose")), Some("agents/goose.svg"));
        assert_eq!(agent_cli_icon(Some("dim")), Some("agents/dim.svg"));
        // Crush has no usable monochrome mark; the icon falls back.
        assert_eq!(agent_cli_icon(Some("crush")), None);
        assert_eq!(agent_cli_icon(None), None);
        assert_eq!(agent_cli_icon(Some("unknown")), None);
        assert_eq!(agent_cli_icon(Some("")), None);
    }

    #[test]
    fn process_name_maps_only_known_tuis() {
        assert_eq!(agent_from_process_name("herdr"), Some("herdr"));
        assert_eq!(agent_from_process_name(" herdr "), Some("herdr"));
        assert_eq!(agent_from_process_name("kimi"), Some("kimi"));
        assert_eq!(agent_from_process_name("mimo"), Some("mimo"));
        assert_eq!(agent_from_process_name("droid"), Some("droid"));
        assert_eq!(agent_from_process_name("kiro-cli"), Some("kiro"));
        assert_eq!(agent_from_process_name("kiro"), Some("kiro"));
        // grok's binary carries its version suffix.
        assert_eq!(
            agent_from_process_name("grok-1.0.34-macos-aarch64"),
            Some("grok")
        );
        assert_eq!(agent_from_process_name("crush"), Some("crush"));
        assert_eq!(agent_from_process_name("goose"), Some("goose"));
        assert_eq!(agent_from_process_name("amp"), Some("amp"));
        // The npm amp build reports itself as `amp.exe` on macOS.
        assert_eq!(agent_from_process_name("amp.exe"), Some("amp"));
        assert_eq!(agent_from_process_name("dim"), Some("dim"));
        assert_eq!(agent_from_process_name("zsh"), None);
        assert_eq!(agent_from_process_name("node"), None);
        assert_eq!(agent_from_process_name("python3"), None);
        assert_eq!(agent_from_process_name("codex"), Some("codex"));
        assert_eq!(agent_from_process_name("Claude.EXE"), Some("claude"));
        assert_eq!(agent_from_process_name("opencode"), Some("opencode"));
        assert_eq!(agent_from_process_name("claude-helper"), None);
        assert_eq!(agent_from_process_name("Cursor-Agent.EXE"), Some("cursor"));
        assert_eq!(agent_from_process_name("kimi-code"), Some("kimi"));
        assert_eq!(agent_from_process_name("copilot"), Some("copilot"));
        assert_eq!(agent_from_process_name(""), None);
    }

    #[test]
    fn recognizes_shells_that_make_stale_screen_markers_non_authoritative() {
        for shell in [
            "zsh",
            "-zsh",
            "bash",
            "fish",
            "nu",
            "pwsh.exe",
            "PowerShell.EXE",
            "cmd.exe",
        ] {
            assert!(process_name_is_shell(shell), "missed shell {shell}");
        }
        assert!(!process_name_is_shell("node"));
        assert!(!process_name_is_shell("codex"));
    }

    #[test]
    fn osc_title_maps_known_titles() {
        assert_eq!(agent_from_osc_title(Some("grok")), Some("grok"));
        assert_eq!(agent_from_osc_title(Some("Grok")), Some("grok"));
        assert_eq!(
            agent_from_osc_title(Some("my-session - grok")),
            Some("grok")
        );
        assert_eq!(agent_from_osc_title(Some("π - tmp")), Some("pi"));
        assert_eq!(agent_from_osc_title(Some("Qwen - tmp")), Some("qwen"));
        assert_eq!(agent_from_osc_title(Some("crush /tmp")), Some("crush"));
        assert_eq!(agent_from_osc_title(Some("Kilo CLI")), Some("kilo"));
        assert_eq!(
            agent_from_osc_title(Some("my-repo - amp - main")),
            Some("amp")
        );
        assert_eq!(agent_from_osc_title(Some("dim")), Some("dim"));
        assert_eq!(agent_from_osc_title(Some("San3an.local: tmp")), None);
        // Partial words must not match.
        assert_eq!(agent_from_osc_title(Some("grokking")), None);
        assert_eq!(agent_from_osc_title(Some("qwen-project — zsh")), None);
        assert_eq!(agent_from_osc_title(Some("crushing-bugs — zsh")), None);
        assert_eq!(agent_from_osc_title(Some("kilobytes — zsh")), None);
        assert_eq!(agent_from_osc_title(None), None);
        assert_eq!(agent_from_osc_title(Some("   ")), None);
    }

    #[test]
    fn screen_text_maps_script_shipped_agents() {
        let lines = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            agent_from_screen_text(&lines(&["Kimi Code updated to v2.0.1"])),
            Some("kimi")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Cursor Agent", "v2026.09.10"])),
            Some("cursor")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Grok Build  1.0.34"])),
            Some("grok")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&[
                "escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more"
            ])),
            Some("pi")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Welcome to Qoder CLI"])),
            Some("qoder")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Welcome to Kiro CLI V3!"])),
            Some("kiro")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Hermes Agent", "Nous Research"])),
            Some("hermes")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Gemini CLI v0.60.0"])),
            Some("gemini")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&[
                "Droid - Factory's AI coding agent in your terminal"
            ])),
            Some("droid")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["Cline needs permission", "Approve tool call?"])),
            Some("cline")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["let cline use this tool"])),
            Some("cline")
        );
        assert_eq!(
            agent_from_screen_text(&lines(&["$ ls -la", "total 0"])),
            None
        );
        assert_eq!(agent_from_screen_text(&[]), None);
    }

    #[test]
    fn should_refresh_agent_cli_throttles_after_first_call() {
        let now = Instant::now();
        let interval = Duration::from_secs(1);
        assert!(should_refresh_agent_cli(None, now, interval));
        assert!(!should_refresh_agent_cli(
            Some(now),
            now + Duration::from_millis(500),
            interval
        ));
        assert!(should_refresh_agent_cli(
            Some(now),
            now + Duration::from_secs(1),
            interval
        ));
    }

    #[test]
    fn screen_detection_is_bounded_until_terminal_context_changes() {
        let observation = AgentCliObservation {
            terminal_id: 7,
            foreground_process_group_id: None,
            title_agent: None,
            input_generation: 2,
        };
        let mut state = AgentCliDetectionState::default();

        assert!(state.observe(observation));
        for _ in 0..6 {
            assert_eq!(state.scan_interval(), Duration::from_millis(300));
            assert!(state.take_screen_scan_attempt());
        }
        for seconds in [2, 4, 8, 16, 32, 64] {
            assert_eq!(state.scan_interval(), Duration::from_secs(seconds));
            assert!(state.take_screen_scan_attempt());
        }
        assert!(!state.take_screen_scan_attempt());
        assert!(!state.observe(observation));
        assert!(!state.take_screen_scan_attempt());

        let changed = AgentCliObservation {
            input_generation: 3,
            ..observation
        };
        assert!(state.observe(changed));
        assert_eq!(state.scan_interval(), Duration::from_millis(300));
        assert!(state.take_screen_scan_attempt());
        state.finish();
        assert!(state.is_exhausted());
        assert!(!state.take_screen_scan_attempt());
    }
}

#[cfg(test)]
mod tests_smart_tab_presentation_agent_cli {
    use super::*;

    #[test]
    fn detected_codex_uses_brand_logo_without_label() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("codex"),
            None,
            Some("zsh"),
            None,
            0,
            false,
        );
        assert_eq!(p.icon, "agents/codex.svg");
        assert_eq!(p.name, "zsh");
    }

    #[test]
    fn detected_codex_keeps_user_label_but_uses_logo() {
        let p = smart_tab_presentation(
            Some("My Session"),
            None,
            Some("phosphor/rocket.svg"),
            Some("codex"),
            None,
            Some("vim README.md"),
            None,
            0,
            false,
        );
        assert_eq!(p.name, "My Session");
        assert_eq!(p.icon, "agents/codex.svg");
    }

    #[test]
    fn agent_logo_wins_over_ssh_globe() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("codex"),
            Some("prod-1.example.com"),
            Some("ssh prod-1.example.com"),
            Some("/home/src"),
            0,
            false,
        );
        assert_eq!(p.name, "prod-1.example.com");
        assert_eq!(p.icon, "agents/codex.svg");
        assert!(p.is_ssh);
    }

    #[test]
    fn editor_tab_ignores_agent_logo() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("codex"),
            None,
            Some("main.rs"),
            None,
            0,
            true,
        );
        assert_eq!(p.icon, "phosphor/file-code.svg");
        assert_eq!(p.name, "main.rs");
    }

    #[test]
    fn no_agent_keeps_ssh_globe() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            None,
            Some("prod-1.example.com"),
            Some("ssh prod-1.example.com"),
            Some("/home/src"),
            0,
            false,
        );
        assert_eq!(p.icon, "phosphor/globe.svg");
        assert_eq!(p.name, "prod-1.example.com");
        assert!(p.is_ssh);
    }

    #[test]
    fn no_agent_keeps_heuristic_process_icon() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            None,
            None,
            Some("vim README.md"),
            None,
            0,
            false,
        );
        assert_eq!(p.icon, "phosphor/code.svg");
        assert_eq!(p.name, "vim README.md");
    }

    #[test]
    fn no_agent_keeps_ai_icon_under_user_label() {
        let p = smart_tab_presentation(
            Some("Deploy"),
            None,
            Some("phosphor/rocket.svg"),
            None,
            None,
            Some("zsh"),
            None,
            0,
            false,
        );
        assert_eq!(p.name, "Deploy");
        assert_eq!(p.icon, "phosphor/rocket.svg");
    }

    #[test]
    fn unknown_agent_falls_through_to_heuristic() {
        let p = smart_tab_presentation(
            None,
            None,
            None,
            Some("unknown"),
            None,
            None,
            None,
            0,
            false,
        );
        assert_eq!(p.icon, "phosphor/terminal.svg");
        assert_eq!(p.name, "Tab 1");
    }
}
