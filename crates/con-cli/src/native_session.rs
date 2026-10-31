use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use con_core::{
    ControlCommand,
    native_session::{NativeAgentSession, native_agent_adapter},
};
use serde_json::{Value, json};
use std::{
    io::{IsTerminal, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

mod install;
mod launch;

#[derive(Subcommand)]
pub enum NativeSessionCommand {
    /// Install native hooks and, for Codex, an opt-in con-only shell launcher.
    Install {
        #[arg(long, value_parser = ["claude", "codex", "cursor", "qwen", "copilot"])]
        agent: String,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// Shell startup file for the opt-in Codex launcher (default: current shell).
        #[arg(long)]
        shell_rc: Option<PathBuf>,
    },
    /// Remove only con's hooks. Native history and other hooks are retained.
    Uninstall {
        #[arg(long, value_parser = ["claude", "codex", "cursor", "qwen", "copilot"])]
        agent: String,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        /// Shell startup file for the opt-in Codex launcher (default: current shell).
        #[arg(long)]
        shell_rc: Option<PathBuf>,
    },
    /// Launch Codex with automatic terminal-local session integration when installed.
    Launch {
        #[arg(long, value_parser = ["codex"])]
        agent: String,
        #[arg(long, hide = true)]
        launcher_dir: Option<PathBuf>,
        #[arg(last = true, allow_hyphen_values = true)]
        args: Vec<std::ffi::OsString>,
    },
    #[command(hide = true)]
    Hook {
        #[arg(long, value_parser = ["claude", "codex", "cursor", "qwen", "copilot"])]
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
    let adapter = native_agent_adapter(agent).context("Unsupported native agent")?;
    let path = std::env::var_os(adapter.config_env)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            (agent == "cursor")
                .then(|| std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()))
                .flatten()
                .map(|root| PathBuf::from(root).join("cursor"))
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(adapter.default_directory)))
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

fn hook_cwd(agent: &str, input: &Value) -> Result<PathBuf> {
    let path = if agent == "cursor" {
        let roots = input
            .get("workspace_roots")
            .and_then(Value::as_array)
            .context("Missing native workspace roots")?;
        ensure!(
            roots.len() == 1,
            "Only single-root local Cursor sessions can be linked"
        );
        roots[0].as_str()
    } else {
        input.get("cwd").and_then(Value::as_str)
    }
    .context("Missing native session directory")?;
    Path::new(path)
        .canonicalize()
        .context("Native session directory is unavailable")
}

/// Hook input is data. Never relay prompt text, credentials, commands, or a
/// subagent's identity into the parent's persisted session.
fn hook_reference(
    agent: &str,
    input: &Value,
    directory: PathBuf,
) -> Result<Option<NativeAgentSession>> {
    let event = input
        .get("hook_event_name")
        .or_else(|| input.get("hookEventName"))
        .and_then(Value::as_str);
    let expected_event = if agent == "cursor" {
        "sessionStart"
    } else {
        "SessionStart"
    };
    if event != Some(expected_event)
        || input
            .get("agent_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
        || (agent != "cursor" && input.get("cursor_version").is_some())
        || (agent == "cursor" && std::env::var("CURSOR_CODE_REMOTE").as_deref() == Ok("true"))
    {
        return Ok(None);
    }
    let mut reference = NativeAgentSession {
        agent: agent.into(),
        session_id: input
            .get(if agent == "cursor" {
                "conversation_id"
            } else {
                "session_id"
            })
            .and_then(Value::as_str)
            .context("Missing native session ID")?
            .into(),
        cwd: hook_cwd(agent, input)?,
        config_dir: directory,
        transcript_path: input
            .get("transcript_path")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        title: None,
        viewport_pixels: None,
    };
    if agent == "copilot" && reference.transcript_path.is_none() {
        // Validate the ID before constructing a path from it.
        reference.validate().map_err(anyhow::Error::msg)?;
        reference.transcript_path = Some(
            reference
                .config_dir
                .join("session-state")
                .join(&reference.session_id)
                .join("events.jsonl"),
        );
    }
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
            shell_rc,
        } => return install::update(&agent, directory, shell_rc, true, json_output),
        NativeSessionCommand::Uninstall {
            agent,
            config_dir: directory,
            shell_rc,
        } => return install::update(&agent, directory, shell_rc, false, json_output),
        NativeSessionCommand::Launch {
            agent: _,
            launcher_dir,
            args,
        } => return launch::run(args, launcher_dir),
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
                let reported_cwd = hook_cwd(&agent, &input)?;
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
    ensure!(
        transcript
            .components()
            .any(|part| part.as_os_str().to_str().is_some_and(|name| {
                name == reference.session_id
                    || name.strip_suffix(".jsonl") == Some(reference.session_id.as_str())
                    || name.strip_suffix(".txt") == Some(reference.session_id.as_str())
                    || name
                        .strip_suffix(".json")
                        .is_some_and(|stem| stem.ends_with(&reference.session_id))
                    || name
                        .strip_suffix(".jsonl")
                        .is_some_and(|stem| stem.ends_with(&reference.session_id))
            })),
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
    fn cursor_copilot_and_qwen_use_native_payloads_without_cross_agent_aliases() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let cursor = json!({"hook_event_name":"sessionStart", "conversation_id":id,
            "cursor_version":"fixture", "workspace_roots":[root], "transcript_path":root.join(format!("{id}.txt"))});
        let reference = hook_reference("cursor", &cursor, root.clone())
            .unwrap()
            .unwrap();
        assert_eq!(reference.session_id, id);
        assert!(
            hook_reference("claude", &cursor, root.clone())
                .unwrap()
                .is_none()
        );
        let mut multi = cursor.clone();
        multi["workspace_roots"] = json!([root, root]);
        assert!(hook_reference("cursor", &multi, root.clone()).is_err());
        let payload = json!({"hook_event_name":"SessionStart", "session_id":id, "cwd":root});
        let copilot = hook_reference("copilot", &payload, root.clone())
            .unwrap()
            .unwrap();
        assert_eq!(
            copilot.transcript_path,
            Some(root.join("session-state").join(&id).join("events.jsonl"))
        );
        assert_eq!(
            hook_reference("qwen", &payload, root)
                .unwrap()
                .unwrap()
                .session_id,
            id
        );
    }
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
