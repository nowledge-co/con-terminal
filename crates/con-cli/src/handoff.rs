mod diagnostics;
mod manual_delivery;
mod notices;

use std::{
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Result, ensure};
use clap::{Subcommand, ValueEnum};
use con_agent::handoff::{AgentKind, create_target, probe_target, target_args_with_model};
use con_core::{
    ControlCommand, SurfaceTarget,
    handoff::{
        HandoffResponse, HandoffRpc, HandoffService, LAUNCH_HELPER_PROTOCOL, PrepareRequest,
        instruction, validate_target_model,
    },
};

/// Launch-helper protocol revision that first understood `target_model`.
const PROTOCOL_WITH_TARGET_MODEL: u32 = 2;

#[derive(Subcommand)]
pub enum HandoffCommand {
    /// Open Agent Handoff for the active Con terminal.
    Open,
    /// List installed agents and their source/target capabilities.
    Agents,
    /// Print the launch-helper protocol revision this build understands.
    Protocol,
    /// List exact sessions for the selected local agent.
    Sources {
        #[arg(long, default_value = "codex")]
        agent: AgentKind,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
    },
    /// Export and review a handoff; never starts target inference.
    Prepare {
        #[arg(long, default_value = "codex")]
        source_agent: AgentKind,
        #[arg(long, default_value = "cursor")]
        target_agent: AgentKind,
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
        #[arg(long)]
        source_session: String,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long, default_value = "")]
        goal: String,
        /// Request an exact model for the new target session's native launch.
        #[arg(long)]
        target_model: Option<String>,
    },
    List {
        #[arg(long, default_value = ".")]
        cwd: PathBuf,
    },
    Get {
        job_id: String,
    },
    /// Open the visible startup helper in a new Con surface.
    Start {
        job_id: String,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        tab: Option<usize>,
        #[arg(long)]
        pane_id: Option<usize>,
    },
    Cancel {
        job_id: String,
        #[arg(long)]
        revision: u64,
    },
    Respond {
        job_id: String,
        #[arg(long)]
        revision: u64,
        #[arg(value_enum)]
        response: Response,
    },
    /// Visible helper. A launch is claimed once and never automatically retried.
    #[command(hide = true)]
    Run {
        job_id: String,
        #[arg(long)]
        revision: u64,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Response {
    Received,
    Sent,
    Stopped,
    Abandon,
}

pub fn run(command: HandoffCommand, socket: &Path, json: bool) -> Result<()> {
    ensure!(
        cfg!(target_os = "macos"),
        "Agent Handoff currently supports macOS only"
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let rpc = match command {
        // Protocol reporting must work without a live Con window so the
        // dispatcher can gate a launch on it.
        HandoffCommand::Protocol => {
            let result = serde_json::json!({
                "launch_helper_protocol": LAUNCH_HELPER_PROTOCOL,
            });
            return print(result, json);
        }
        // Open and Start need a live Con window and stay on the socket.
        HandoffCommand::Open => {
            let result = super::send_command(socket, ControlCommand::Handoff(HandoffRpc::Open))?;
            return print(result, json);
        }
        HandoffCommand::Start {
            job_id,
            revision,
            tab,
            pane_id,
        } => {
            let result = super::send_command(
                socket,
                ControlCommand::Handoff(HandoffRpc::Start {
                    job_id,
                    expected_revision: revision,
                    tab_index: tab,
                    source: SurfaceTarget::new(None, pane_id, None),
                }),
            )?;
            return print(result, json);
        }
        // The visible helper keeps its own terminal-bound launch flow.
        HandoffCommand::Run { job_id, revision } => {
            ensure!(
                std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
                "The handoff helper must run in a visible terminal"
            );
            let service = HandoffService::new()?;
            launch(&runtime, &service, &job_id, revision)?;
            return Ok(());
        }
        HandoffCommand::Agents => HandoffRpc::Agents,
        HandoffCommand::Sources { agent, cwd } => HandoffRpc::Sources { cwd, agent },
        HandoffCommand::Prepare {
            source_agent,
            target_agent,
            cwd,
            source_session,
            request_id,
            goal,
            target_model,
        } => {
            if let Some(model) = target_model.as_deref() {
                validate_target_model(model)?;
            }
            HandoffRpc::Prepare {
                request: PrepareRequest {
                    source_agent,
                    target_agent,
                    request_id: request_id.unwrap_or_else(con_core::handoff::new_request_id),
                    cwd,
                    source_session_id: source_session,
                    goal,
                    target_model,
                },
            }
        }
        HandoffCommand::List { cwd } => HandoffRpc::List { cwd },
        HandoffCommand::Get { job_id } => HandoffRpc::Get { job_id },
        HandoffCommand::Cancel { job_id, revision } => HandoffRpc::Cancel {
            job_id,
            expected_revision: revision,
        },
        HandoffCommand::Respond {
            job_id,
            revision,
            response,
        } => HandoffRpc::Respond {
            job_id,
            expected_revision: revision,
            outcome: match response {
                Response::Received => HandoffResponse::ConfirmReceived,
                Response::Sent => HandoffResponse::ConfirmSent,
                Response::Stopped => HandoffResponse::ConfirmStopped,
                Response::Abandon => HandoffResponse::Abandon,
            },
        },
    };
    print(runtime.block_on(rpc.execute())?, json)
}

fn print(result: serde_json::Value, json: bool) -> Result<()> {
    let _ = json; // All handoff output is structured, including the preview.
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn launch(
    runtime: &tokio::runtime::Runtime,
    service: &HandoffService,
    id: &str,
    revision: u64,
) -> Result<()> {
    let _guard = service.launch_guard(id)?;
    let pending = service.get(id)?;
    // A stale launch must be a clean no-op: refuse BEFORE the closure whose
    // failures are recorded via `launch_error`, so re-running an old
    // `con-cli handoff run` can never rewrite a healthy job's state.
    ensure!(
        pending.revision == revision
            && pending.state == con_core::handoff::HandoffState::LaunchPending,
        "This launch is stale or has already been claimed"
    );
    let result = (|| {
        // The helper half of the contract's double check (Con gates before
        // dispatch; the helper re-checks after reading the job): refuse a job
        // whose capabilities this build cannot honor.
        ensure_helper_protocol(&pending.request, LAUNCH_HELPER_PROTOCOL)?;
        let capabilities = runtime.block_on(probe_target(pending.request.target_agent))?;
        runtime.block_on(service.validate_source(id))?;
        let job = service.begin_launch(id, revision, &capabilities)?;
        launch_claimed(runtime, service, &job)
    })();
    if let Err(error) = &result {
        let target = service
            .get(id)
            .map(|job| job.target)
            .unwrap_or(pending.target);
        let diagnostic = diagnostics::failure_context(
            &target,
            &con_agent::handoff::launch_environment(target.agent),
        );
        eprintln!("{diagnostic}");
        let _ = service.launch_error(id, &format!("{error}\n{diagnostic}"));
    }
    result
}

fn launch_claimed(
    runtime: &tokio::runtime::Runtime,
    service: &HandoffService,
    job: &con_core::handoff::HandoffJob,
) -> Result<()> {
    let id = job.id.as_str();
    let capabilities = &job.target;
    let label = capabilities.agent.label();
    notices::opening(&mut std::io::stdout(), label)?;
    if let Some(model) = job.request.target_model.as_deref() {
        eprintln!(
            "Requested model: {model} (the target TUI/service decides whether it takes effect)"
        );
    }
    let target = runtime.block_on(create_target(capabilities, &job.request.cwd))?;
    let job = service.record_launch(id, job.revision, target.as_deref())?;
    let prompt = instruction(id);
    if let Some(target) = &target {
        eprintln!("{label} session: {target}");
    } else {
        eprintln!("Native session ID is not exposed before startup. Handoff: {id}");
    }
    // Kimi clipboard backup and PTY delivery belong to Con after spawn.
    if !capabilities.automatic_delivery && capabilities.agent != AgentKind::Kimi {
        manual_delivery::copy_instruction(capabilities.agent, &prompt)?;
    }
    service.validate_workspace(id)?;
    let mut child = Command::new(&capabilities.executable)
        .env_clear()
        // Explicit terminal/network/native-auth categories are documented in launch_environment.
        .envs(con_agent::handoff::launch_environment(capabilities.agent))
        .args(target_args_with_model(
            capabilities,
            &job.request.cwd,
            target.as_deref(),
            capabilities.automatic_delivery.then_some(prompt.as_str()),
            // The value was validated at prepare time and re-validated by
            // ensure_helper_protocol on read; it rides as its own `--model`
            // argv pair, never a shell fragment. A target without a model
            // flag fails here (clear error → NeedsInteraction), never a
            // silent default-model launch.
            job.request.target_model.as_deref(),
        )?)
        .current_dir(&job.request.cwd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    let start = con_agent::handoff::process_start_secs(child.id() as i32);
    if let Err(error) = service.record_spawn(id, job.revision, child.id(), start) {
        eprintln!("Target agent is running; delivery state needs reconciliation: {error}");
    }
    let status = child.wait()?;
    diagnostics::check_exit(capabilities.agent, status)?;
    notices::exited(&mut std::io::stdout(), label, &status.to_string())?;
    Ok(())
}

/// Refuse a job that requires a newer launch-helper protocol instead of
/// silently dropping the capability (the handoff contract's model-override
/// rules): never launch with the target's default model when the job names
/// one. The model value is re-validated on read even though prepare checked
/// it before persisting.
fn ensure_helper_protocol(request: &PrepareRequest, protocol: u32) -> Result<()> {
    ensure!(
        protocol >= LAUNCH_HELPER_PROTOCOL,
        "Upgrade con-cli and rebuild Con from the same version; handoff requires protocol {LAUNCH_HELPER_PROTOCOL}, got {protocol}"
    );
    if let Some(model) = request.target_model.as_deref() {
        ensure!(
            protocol >= PROTOCOL_WITH_TARGET_MODEL,
            "This handoff requests model '{model}' (launch-helper protocol {PROTOCOL_WITH_TARGET_MODEL}), but this con-cli speaks protocol {protocol}. Upgrade con-cli — refusing to launch with the target's default model."
        );
        validate_target_model(model)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(target_model: Option<&str>) -> PrepareRequest {
        PrepareRequest {
            source_agent: AgentKind::Codex,
            target_agent: AgentKind::Cursor,
            request_id: "req".into(),
            cwd: PathBuf::from("/tmp"),
            source_session_id: "s".into(),
            goal: String::new(),
            target_model: target_model.map(str::to_owned),
        }
    }

    #[test]
    fn helper_refuses_a_model_job_it_cannot_understand() {
        let job = request(Some("gpt-5"));
        let error = ensure_helper_protocol(&job, PROTOCOL_WITH_TARGET_MODEL - 1).unwrap_err();
        assert!(error.to_string().contains("Upgrade con-cli"), "{error}");
        ensure_helper_protocol(&job, PROTOCOL_WITH_TARGET_MODEL).unwrap();
        // Prompt delivery requires protocol 2 even without a model.
        assert!(ensure_helper_protocol(&request(None), 1).is_err());
        ensure_helper_protocol(&request(None), LAUNCH_HELPER_PROTOCOL).unwrap();
    }

    #[test]
    fn helper_revalidates_the_model_value_on_read() {
        let error =
            ensure_helper_protocol(&request(Some("--model")), LAUNCH_HELPER_PROTOCOL).unwrap_err();
        assert!(error.to_string().contains("command-line option"), "{error}");
    }
}
