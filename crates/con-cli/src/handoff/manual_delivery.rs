//! Keep manual delivery reachable after the native TUI takes over the PTY.
use anyhow::{Context, Result, ensure};
use con_agent::handoff::AgentKind;
use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

pub(super) fn copy_instruction(agent: AgentKind, instruction: &str) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Manual handoff requires macOS clipboard support"
    );
    copy_with(Path::new("/usr/bin/pbcopy"), agent, instruction)
}

fn copy_with(executable: &Path, agent: AgentKind, instruction: &str) -> Result<()> {
    let mut child = Command::new(executable)
        .env_clear()
        .envs(con_agent::handoff::launch_environment(agent))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Cannot copy handoff instruction; target was not started")?;
    let written = child
        .stdin
        .take()
        .context("Clipboard stdin unavailable")?
        .write_all(instruction.as_bytes());
    let status = child.wait()?;
    written.context("Cannot write handoff instruction to clipboard")?;
    ensure!(
        status.success(),
        "Clipboard copy failed; target was not started"
    );
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn clipboard_receives_literal_stdin_and_failure_blocks_launch() {
        let root = std::env::temp_dir().join(format!("con-clipboard-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("copy");
        let output = root.join("received");
        std::fs::write(
            &script,
            format!("#!/bin/sh\n/bin/cat > '{}'\n", output.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let prompt = "Read context; $(touch should-not-exist)\n中文";
        copy_with(&script, AgentKind::Kimi, prompt).unwrap();
        assert_eq!(std::fs::read_to_string(&output).unwrap(), prompt);
        assert!(copy_with(Path::new("/usr/bin/false"), AgentKind::Kimi, prompt).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
