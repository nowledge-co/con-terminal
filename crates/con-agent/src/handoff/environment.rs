//! Explicit environments for handoff subprocesses; never forward arbitrary secrets.
use std::ffi::OsString;

use super::{AgentKind, protocol};

// PATH finds native CLIs/interpreters; HOME/USER locate native login/config;
// TMPDIR supports temporary files; LANG/LC_ALL preserve text encoding.
const BASE: &[&str] = &["HOME", "USER", "TMPDIR", "LANG", "LC_ALL"];

// Authenticated Cursor reads and interactive agents must honor proxy/TLS routing.
const NETWORK: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// Credential-free environment for version/help, SQLite, Git and helper probes.
pub fn probe_environment() -> Vec<(OsString, OsString)> {
    let mut env = selected(BASE);
    env.push(("PATH".into(), protocol::process_path()));
    env
}

fn selected(names: &[&str]) -> Vec<(OsString, OsString)> {
    names
        .iter()
        .filter_map(|name| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(|value| (OsString::from(name), value))
        })
        .collect()
}

/// Local history needs no provider credentials. Cursor's ACP/models/create-chat
/// require its native API login; CODEX_HOME selects the same local history store.
pub(super) fn reader_environment(agent: AgentKind) -> Vec<(OsString, OsString)> {
    let mut env = probe_environment();
    // Preserve native config/login locations instead of silently selecting a
    // different account/history directory when the user uses XDG overrides.
    env.extend(selected(&[
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ]));
    if agent == AgentKind::Cursor {
        env.extend(selected(NETWORK));
    }
    env.extend(selected(reader_names(agent)));
    env
}

fn reader_names(agent: AgentKind) -> &'static [&'static str] {
    match agent {
        AgentKind::Cursor => &["CURSOR_API_KEY"],
        AgentKind::Codex => &["CODEX_HOME"],
        AgentKind::Kimi | AgentKind::Unknown => &[],
    }
}

/// Interactive agents need terminal identity, native config, network routing/TLS,
/// and their provider login. Keep exact names, not arbitrary *_KEY/*_TOKEN or
/// inherited shell/loader hooks. HOME retains native on-disk OAuth credentials;
/// this does not bypass any agent login, workspace trust or permission checks.
pub fn launch_environment(agent: AgentKind) -> Vec<(OsString, OsString)> {
    let mut env = reader_environment(agent);
    env.extend(selected(NETWORK));
    env.extend(selected(&[
        "TERM",
        "COLORTERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "TERMINFO",
        "TERMINFO_DIRS",
        "SHELL",
        // Native coding tools may authenticate Git operations through an SSH agent.
        "SSH_AUTH_SOCK",
    ]));
    env.extend(selected(launch_names(agent)));
    env
}

fn launch_names(agent: AgentKind) -> &'static [&'static str] {
    match agent {
        // OpenAI API login and compatible endpoint; CODEX_HOME is above.
        AgentKind::Codex => &[
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_ORG_ID",
            "OPENAI_PROJECT_ID",
            // Defensive: some shells route assist traffic through this endpoint.
            // Its absence did not reproduce the reported bootstrap timeout locally.
            "CODE_ASSIST_ENDPOINT",
        ],
        AgentKind::Unknown => &[],
        AgentKind::Cursor => &[], // CURSOR_API_KEY is above; no other provider key.
        // Kimi's native API login and endpoint override.
        AgentKind::Kimi => &[
            "KIMI_API_KEY",
            "KIMI_BASE_URL",
            "KIMI_MODEL_NAME",
            "KIMI_SHARE_DIR",
        ],
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn subprocess_environments_preserve_only_allowed_values() {
        const MARKER: &str = "CON_HANDOFF_ENV_TEST";
        if std::env::var_os(MARKER).is_none() {
            // Isolate a poisoned parent environment without mutating process-wide
            // environment while the rest of the Rust tests run in parallel.
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "handoff::environment::tests::subprocess_environments_preserve_only_allowed_values"])
                .env(MARKER, "1")
                .env("HTTP_PROXY", "")
                .env("CODE_ASSIST_ENDPOINT", "assist-sentinel")
                .env("UNRELATED_API_KEY", "unrelated-sentinel")
                .env("CURSOR_API_KEY", "cursor-sentinel")
                .env("OPENAI_API_KEY", "openai-sentinel")
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let probe = runtime
            .block_on(protocol::output(std::path::Path::new("/usr/bin/env"), &[]))
            .unwrap();
        let capture = runtime
            .block_on(protocol::capture(
                std::path::Path::new("/usr/bin/env"),
                &[],
                None,
                128 * 1024,
                None,
            ))
            .unwrap();
        for text in [probe, String::from_utf8(capture).unwrap()] {
            assert!(!text.contains("sentinel"));
            assert!(!text.contains(MARKER));
            assert!(text.contains("PATH="));
        }
        for agent in AgentKind::ALL {
            for env in [reader_environment(agent), launch_environment(agent)] {
                assert!(!env.iter().any(|(name, _)| name == "UNRELATED_API_KEY"));
                assert!(!env.iter().any(|(name, _)| name == "HTTP_PROXY"));
                assert_eq!(env.iter().any(|(name, value)| name == "CURSOR_API_KEY" && value == "cursor-sentinel"), agent == AgentKind::Cursor);
            }
        }
        assert!(
            launch_environment(AgentKind::Codex)
                .iter()
                .any(|(name, value)| name == "OPENAI_API_KEY" && value == "openai-sentinel")
        );
    }

    #[test]
    fn credentials_are_scoped_to_the_operation_and_agent() {
        assert_eq!(reader_names(AgentKind::Cursor), &["CURSOR_API_KEY"]);
        assert_eq!(reader_names(AgentKind::Codex), &["CODEX_HOME"]);
        assert!(reader_names(AgentKind::Kimi).is_empty());
        assert!(launch_names(AgentKind::Codex).contains(&"OPENAI_API_KEY"));
        assert!(launch_names(AgentKind::Codex).contains(&"CODE_ASSIST_ENDPOINT"));
        assert!(launch_names(AgentKind::Kimi).contains(&"KIMI_API_KEY"));
        assert!(!launch_names(AgentKind::Kimi).contains(&"OPENAI_API_KEY"));
        assert!(launch_names(AgentKind::Cursor).is_empty());
    }
}
