use std::path::Path;

use anyhow::{Result, ensure};

use super::{AgentKind, TargetCapabilities, protocol};

/// Probe the named product, never an ambiguous alias such as `agent`.
/// All help/version calls are read-only.
pub async fn probe_target(agent: AgentKind) -> Result<TargetCapabilities> {
    ensure!(agent != AgentKind::Unknown, "Unsupported target Agent");
    let executable = protocol::executable(agent.executable_name())?;
    probe_executable(agent, &executable).await
}

pub(super) async fn probe_executable(
    agent: AgentKind,
    executable: &Path,
) -> Result<TargetCapabilities> {
    ensure!(agent != AgentKind::Unknown, "Unsupported target Agent");
    let version = protocol::output(executable, &["--version"]).await?;
    let help = protocol::output(executable, &["--help"]).await?;
    verify_help(agent, &help)?;
    let version = version.lines().next().unwrap_or_default().trim();
    ensure!(
        !version.is_empty() && version.len() <= 256 && !version.chars().any(char::is_control),
        "{} returned an invalid version",
        agent.label()
    );
    let branded_version = match agent {
        AgentKind::Codex => version.starts_with("codex-cli "),
        _ => true,
    };
    ensure!(branded_version, "Executable is not {}", agent.label());
    Ok(TargetCapabilities {
        agent,
        executable: executable.to_owned(),
        version: version.to_owned(),
        automatic_delivery: automatic_delivery(agent, version),
    })
}

// This capability gates argv only. Kimi uses Con-side PTY delivery.
// Codex's verified help promises [PROMPT] and --cd. Cursor retains the
// native positional-prompt contract accepted on 2026.09.18; require that
// release or newer plus verify_help's workspace/resume capability checks.
fn automatic_delivery(agent: AgentKind, version: &str) -> bool {
    match agent {
        AgentKind::Codex => true,
        AgentKind::Cursor => version.split_once('-').is_some_and(|(date, _)| {
            chrono::NaiveDate::parse_from_str(date, "%Y.%m.%d")
                .is_ok_and(|date| date >= chrono::NaiveDate::from_ymd_opt(2026, 9, 18).unwrap())
        }),
        _ => false,
    }
}

fn verify_help(agent: AgentKind, help: &str) -> Result<()> {
    // Require a product marker and the exact options used by this adapter.
    // Matching only a filename or a semver could identify an unrelated program.
    // `--model` is required wherever the adapter may pass a model override;
    let markers: &[&str] = match agent {
        AgentKind::Unknown => anyhow::bail!("Unsupported target Agent"),
        AgentKind::Codex => &["Codex CLI", "app-server", "--cd", "--model", "[PROMPT]"],
        AgentKind::Cursor => &[
            "Cursor Agent",
            "create-chat",
            "--resume",
            "--workspace",
            "--model",
        ],
        AgentKind::Kimi => &["Usage: kimi", "kimi-code", "--session", "--model", "export"],
    };
    ensure!(
        markers.iter().all(|marker| help.contains(marker)),
        "{} does not expose the required native handoff capabilities",
        agent.label()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_prompt_capability_accepts_newer_cursor_and_codex() {
        assert!(automatic_delivery(AgentKind::Codex, "codex-cli 0.156.1"));
        for version in ["2026.09.18-9a7762b", "2026.09.23-86fc751"] {
            assert!(automatic_delivery(AgentKind::Cursor, version));
        }
        for version in ["2026.09.17-old", "unknown", "2026.99.99-invalid"] {
            assert!(!automatic_delivery(AgentKind::Cursor, version));
        }
        assert!(!automatic_delivery(AgentKind::Kimi, "2.1.1"));
    }

    #[test]
    fn ambiguous_agent_alias_is_not_cursor() {
        assert!(verify_help(AgentKind::Cursor, "Usage: kimi --session --model export").is_err());
        assert!(verify_help(AgentKind::Kimi, "Cursor Agent --resume --workspace").is_err());
    }
}
