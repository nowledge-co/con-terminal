//! Safe launch diagnostics: no environment values or captured TUI output.
use con_agent::handoff::{AgentKind, TargetCapabilities};
use std::{collections::BTreeSet, ffi::OsString, process::ExitStatus};

pub(super) fn failure_context(target: &TargetCapabilities, env: &[(OsString, OsString)]) -> String {
    let keys: BTreeSet<_> = env.iter().map(|(key, _)| key.to_string_lossy()).collect();
    format!(
        "Handoff target={} version={:?} launch_env_keys={keys:?}",
        target.agent, target.version
    )
}

pub(super) fn check_exit(agent: AgentKind, status: ExitStatus) -> anyhow::Result<()> {
    anyhow::ensure!(
        status.success(),
        "{} exited unsuccessfully ({status}); handoff needs review. If startup reported account/read or TUI bootstrap failure, check the agent login, network/proxy and assist endpoint, then prepare again. See the target terminal for the original error; nothing was retried.",
        agent.label()
    );
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn diagnostics_include_identity_and_only_unique_environment_keys() {
        let target = TargetCapabilities {
            agent: AgentKind::Codex,
            executable: "/unused".into(),
            version: "codex-cli test".into(),
            automatic_delivery: true,
        };
        let message = failure_context(
            &target,
            &[
                ("HTTP_PROXY".into(), "SECRET_PROXY".into()),
                ("OPENAI_API_KEY".into(), "SECRET_KEY".into()),
                ("HTTP_PROXY".into(), "SECRET_PROXY".into()),
            ],
        );
        assert!(message.contains("codex"));
        assert!(message.contains("codex-cli test"));
        assert!(message.contains("OPENAI_API_KEY"));
        assert_eq!(message.matches("HTTP_PROXY").count(), 1);
        assert!(!message.contains("SECRET"));
    }

    #[test]
    fn failed_bootstrap_exit_is_not_a_successful_handoff() {
        let error = check_exit(AgentKind::Codex, ExitStatus::from_raw(1 << 8)).unwrap_err();
        assert!(error.to_string().contains("TUI bootstrap"));
        assert!(error.to_string().contains("network/proxy"));
        assert!(check_exit(AgentKind::Codex, ExitStatus::from_raw(0)).is_ok());
        assert!(check_exit(AgentKind::Codex, ExitStatus::from_raw(9)).is_err());
    }
}
