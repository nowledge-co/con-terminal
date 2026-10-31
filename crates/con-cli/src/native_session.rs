use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use con_core::{ControlCommand, native_session::NativeAgentSession};
use serde_json::{Value, json};
use std::{
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

mod install;

#[derive(Subcommand)]
pub enum NativeSessionCommand {
    /// Install only con's native-session hooks, preserving other agent settings.
    Install {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Remove only con's hooks. Native history and other hooks are retained.
    Uninstall {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    #[command(hide = true)]
    Hook {
        #[arg(long, value_parser = ["claude", "codex"])]
        agent: String,
    },
    /// Visible, terminal-bound native resume helper; never selects a recent session.
    #[command(hide = true)]
    Resume {
        #[arg(long)]
        origin: String,
        #[arg(long)]
        agent: String,
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        cwd: PathBuf,
        #[arg(long)]
        config_dir: PathBuf,
        #[arg(long)]
        transcript_path: Option<PathBuf>,
    },
}

pub(super) fn config_dir(agent: &str) -> Result<PathBuf> {
    let key = match agent {
        "codex" => "CODEX_HOME",
        "claude" => "CLAUDE_CONFIG_DIR",
        _ => anyhow::bail!("Unsupported native agent"),
    };
    let path = std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            dirs::home_dir().map(|home| {
                home.join(if agent == "codex" {
                    ".codex"
                } else {
                    ".claude"
                })
            })
        })
        .context("Cannot locate the native agent configuration directory")?;
    path.canonicalize()
        .context("The native agent configuration directory is unavailable")
}

fn sequence() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

fn report(
    socket: &Path,
    origin: &str,
    reference: Option<NativeAgentSession>,
    error: Option<String>,
    expected_session_id: Option<String>,
) -> Result<()> {
    uuid::Uuid::parse_str(origin).context("Invalid terminal context")?;
    super::send_command(
        socket,
        ControlCommand::NativeSessionReport {
            started: false,
            expected_session_id,
            origin: origin.into(),
            sequence: sequence(),
            reference,
            error,
        },
    )?;
    Ok(())
}

/// Hook input is data. Never relay prompt text, credentials, commands, or a
/// subagent's identity into the parent's persisted session.
fn hook_reference(
    agent: &str,
    input: &Value,
    directory: PathBuf,
) -> Result<Option<NativeAgentSession>> {
    if input.get("hook_event_name").and_then(Value::as_str) != Some("SessionStart")
        || input
            .get("agent_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
        || input.get("cursor_version").is_some()
    {
        return Ok(None);
    }
    let reference = NativeAgentSession {
        agent: agent.into(),
        session_id: input["session_id"]
            .as_str()
            .context("Missing native session ID")?
            .into(),
        cwd: PathBuf::from(
            input["cwd"]
                .as_str()
                .context("Missing native session directory")?,
        )
        .canonicalize()?,
        config_dir: directory,
        transcript_path: input
            .get("transcript_path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        title: None,
        viewport_pixels: None,
    };
    reference.validate().map_err(anyhow::Error::msg)?;
    Ok(Some(reference))
}

/// This rejects shared-daemon hook context; it does not infer session identity
/// or grant control from process observations. Identity still comes exclusively
/// from the native report and the terminal's inherited origin/socket pair.
#[cfg(target_os = "macos")]
fn has_terminal_local_codex_ancestor() -> bool {
    let mut pid = std::process::id();
    for _ in 0..16 {
        let Ok(output) = Command::new("/bin/ps")
            .args(["-p", &pid.to_string(), "-o", "ppid=", "-o", "comm="])
            .output()
        else {
            return false;
        };
        let line = String::from_utf8_lossy(&output.stdout);
        let Some((parent, executable)) = line.trim().split_once(char::is_whitespace) else {
            return false;
        };
        if Path::new(executable.trim())
            .file_name()
            .is_some_and(|name| name == "codex")
        {
            let Ok(args) = Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "args="])
                .output()
            else {
                return false;
            };
            if String::from_utf8_lossy(&args.stdout)
                .split_whitespace()
                .any(|arg| arg == "--no-daemon")
            {
                return true;
            }
        }
        let Ok(parent) = parent.parse::<u32>() else {
            return false;
        };
        if parent <= 1 || parent == pid {
            return false;
        }
        pid = parent;
    }
    false
}

pub fn run(command: NativeSessionCommand, socket: &Path, json_output: bool) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Native agent session integration currently supports macOS only"
    );
    match command {
        NativeSessionCommand::Install {
            agent,
            config_dir: directory,
        } => return install::update(&agent, directory, true, json_output),
        NativeSessionCommand::Uninstall {
            agent,
            config_dir: directory,
        } => return install::update(&agent, directory, false, json_output),
        NativeSessionCommand::Hook { agent } => {
            // Hooks themselves may detach from the TTY. Require a terminal-
            // local Codex ancestor, not /dev/tty on this hook subprocess.
            #[cfg(target_os = "macos")]
            if agent == "codex" && !has_terminal_local_codex_ancestor() {
                return Ok(());
            }
            let Some(origin) = std::env::var_os("CON_TERMINAL_CONTEXT") else {
                return Ok(());
            };
            // No endpoint fallback: an old terminal's hook must not bind to a
            // different running app just because it uses the default socket.
            let Some(endpoint) = std::env::var_os("CON_SOCKET_PATH") else {
                return Ok(());
            };
            let result = (|| -> Result<()> {
                let mut input = String::new();
                std::io::stdin().take(65537).read_to_string(&mut input)?;
                ensure!(
                    input.len() <= 65536,
                    "Native session hook payload is oversized"
                );
                let input: Value = serde_json::from_str(&input)?;
                let reported_cwd = input
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .context("Missing native session directory")?
                    .canonicalize()?;
                ensure!(
                    reported_cwd == std::env::current_dir()?.canonicalize()?,
                    "The native hook directory does not match its reported session"
                );
                if let Some(reference) = hook_reference(&agent, &input, config_dir(&agent)?)? {
                    report(
                        Path::new(&endpoint),
                        &origin.to_string_lossy(),
                        Some(reference),
                        None,
                        None,
                    )?;
                }
                Ok(())
            })();
            // A failed optional integration must not block the native agent.
            if result.is_err() {
                eprintln!(
                    "con: native session link was not recorded; inspect con's session state before relying on restore"
                );
            }
        }
        NativeSessionCommand::Resume {
            origin,
            agent,
            session_id,
            cwd,
            config_dir,
            transcript_path,
        } => {
            ensure!(
                std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
                "Native resume must run in a visible terminal"
            );
            ensure!(
                std::env::var("CON_TERMINAL_CONTEXT").as_deref() == Ok(origin.as_str()),
                "The visible terminal context does not match this resume request"
            );
            let reference = NativeAgentSession {
                agent,
                session_id,
                cwd,
                config_dir,
                transcript_path,
                title: None,
                viewport_pixels: None,
            };
            let result = resume(&reference, || {
                super::send_command(
                    socket,
                    ControlCommand::NativeSessionReport {
                        started: true,
                        origin: origin.clone(),
                        sequence: sequence(),
                        reference: None,
                        error: None,
                        expected_session_id: Some(reference.session_id.clone()),
                    },
                )?;
                Ok(())
            });
            match result {
                Ok(()) => {
                    let _ = report(
                        socket,
                        &origin,
                        None,
                        None,
                        Some(reference.session_id.clone()),
                    );
                }
                Err(error) => {
                    let message = format!("Could not resume {}. {error}", reference.label());
                    let _ = report(
                        socket,
                        &origin,
                        None,
                        Some(message.clone()),
                        Some(reference.session_id.clone()),
                    );
                    anyhow::bail!(message);
                }
            }
        }
    }
    let _ = json_output;
    Ok(())
}

fn resume(reference: &NativeAgentSession, started: impl FnOnce() -> Result<()>) -> Result<()> {
    reference.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        reference.cwd.is_dir(),
        "The saved working directory is unavailable"
    );
    ensure!(
        reference.config_dir.is_dir(),
        "The saved native configuration directory is unavailable"
    );
    let transcript = reference.transcript_path.as_ref().context(
        "No exact native history path was recorded; link the session again before restoring",
    )?;
    ensure!(
        transcript.is_file(),
        "The native history file is unavailable; no new session was started"
    );
    let transcript = transcript.canonicalize()?;
    let store = reference.config_dir.canonicalize()?;
    ensure!(
        transcript.starts_with(&store),
        "The native history is outside the recorded agent store"
    );
    let file = transcript
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or_default();
    ensure!(
        file.contains(&reference.session_id),
        "The native history path does not match the exact saved session ID"
    );
    let argv = reference.resume_argv().map_err(anyhow::Error::msg)?;
    eprintln!(
        "Resuming {} conversation in {}",
        reference.label(),
        reference.cwd.display()
    );
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&reference.cwd)
        .env(reference.config_env().map_err(anyhow::Error::msg)?, &store)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("The native agent could not start")?;
    // Starting a process is not native session confirmation. Codex may defer
    // SessionStart until the next turn; report these independently.
    if started().is_err() {
        eprintln!(
            "con: native startup acknowledgement was not recorded; inspect the terminal before retrying"
        );
    }
    let status = child.wait()?;
    ensure!(
        status.success(),
        "The native agent exited unsuccessfully; inspect its output above"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hooks_bind_exact_session_and_reject_subagents_or_missing_ids() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let payload = json!({"hook_event_name":"SessionStart", "session_id": id, "cwd": root});
        assert_eq!(
            hook_reference("codex", &payload, root.clone())
                .unwrap()
                .unwrap()
                .session_id,
            id
        );
        let mut child = payload.clone();
        child["agent_id"] = json!("child");
        assert!(
            hook_reference("claude", &child, root.clone())
                .unwrap()
                .is_none()
        );
        let mut invalid = payload;
        invalid["session_id"] = json!("--last");
        assert!(hook_reference("codex", &invalid, root).is_err());
    }
}
