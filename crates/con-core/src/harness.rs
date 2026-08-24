use con_agent::{
    AgentEvent, AgentProvider, Conversation, Message, PaneRequest, SkillRegistry, TerminalContext,
    TerminalExecRequest, ToolApprovalDecision, is_dangerous,
};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::runtime::Runtime;

use crate::config::Config;

#[derive(Default, Debug, Clone)]
struct RequestTrace {
    steps: Vec<con_agent::conversation::AgentStep>,
    thinking: String,
}

/// Events from the harness to the UI.
///
/// These flow over a crossbeam channel polled by the GPUI workspace.
/// All variants are Clone so the UI can cheaply forward them.
#[derive(Debug, Clone)]
pub enum HarnessEvent {
    /// Agent is preparing to call the model
    Thinking,
    /// Incremental extended thinking/reasoning text from the model
    ThinkingDelta(String),
    /// Incremental text token from streaming
    Token(String),
    /// Reasoning step (e.g. "anthropic:claude-sonnet-4-0")
    Step(con_agent::conversation::AgentStep),
    /// A tool is about to execute (safe tools proceed immediately)
    ToolCallStart {
        call_id: String,
        tool_name: String,
        args: String,
    },
    /// A dangerous tool needs user approval before executing.
    /// The UI should present an approval dialog and call
    /// `respond_to_approval()` with the decision.
    ToolApprovalNeeded {
        call_id: String,
        tool_name: String,
        args: String,
        /// Sender to deliver the approval decision back to the hook.
        /// This is per-request: each agent invocation gets its own channel.
        approval_tx: Sender<ToolApprovalDecision>,
    },
    /// Tool finished executing
    ToolCallComplete {
        call_id: String,
        tool_name: String,
        result: String,
    },
    /// Agent produced a final response
    ResponseComplete(Message),
    /// An error occurred
    Error(String),
    /// Skill registry changed
    SkillsUpdated(Vec<String>),
}

/// Classifies user input
#[derive(Debug)]
pub enum InputKind {
    NaturalLanguage(String),
    ShellCommand(String),
    SkillInvoke(String, Option<String>),
}

// ---------------------------------------------------------------------------
// AgentSession — per-tab conversation state and channels
// ---------------------------------------------------------------------------

/// Per-tab agent session. Owns the conversation and channels for one tab's
/// agent interactions. Lightweight: no runtime, no config, no skills.
pub struct AgentSession {
    conversation: Arc<Mutex<Conversation>>,
    event_tx: Sender<HarnessEvent>,
    event_rx: Receiver<HarnessEvent>,
    terminal_exec_tx: Sender<TerminalExecRequest>,
    terminal_exec_rx: Receiver<TerminalExecRequest>,
    pane_tx: Sender<PaneRequest>,
    pane_rx: Receiver<PaneRequest>,
    cancel_flag: Arc<AtomicBool>,
}

impl AgentSession {
    /// Create a fresh session with a new conversation.
    pub fn new() -> Self {
        let (event_tx, event_rx) = crossbeam_channel::unbounded();
        let (terminal_exec_tx, terminal_exec_rx) = crossbeam_channel::unbounded();
        let (pane_tx, pane_rx) = crossbeam_channel::unbounded();
        Self {
            conversation: Arc::new(Mutex::new(Conversation::new())),
            event_tx,
            event_rx,
            terminal_exec_tx,
            terminal_exec_rx,
            pane_tx,
            pane_rx,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a session from a loaded conversation.
    pub fn with_conversation(conv: Conversation) -> Self {
        let mut s = Self::new();
        s.conversation = Arc::new(Mutex::new(conv));
        s
    }

    pub fn events(&self) -> &Receiver<HarnessEvent> {
        &self.event_rx
    }

    pub fn terminal_exec_requests(&self) -> &Receiver<TerminalExecRequest> {
        &self.terminal_exec_rx
    }

    pub fn pane_requests(&self) -> &Receiver<PaneRequest> {
        &self.pane_rx
    }

    pub fn conversation_id(&self) -> String {
        self.conversation.lock().id.clone()
    }

    pub fn cancel_current(&self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
    }

    pub fn conversation(&self) -> Arc<Mutex<Conversation>> {
        self.conversation.clone()
    }

    /// Start a new conversation, saving the current one first.
    pub fn new_conversation(&mut self) {
        let conv = self.conversation.lock();
        if let Err(e) = conv.save() {
            log::warn!("Failed to save conversation: {}", e);
        }
        drop(conv);
        self.conversation = Arc::new(Mutex::new(Conversation::new()));
    }

    /// Restore a conversation by ID, saving the current one first.
    pub fn load_conversation(&mut self, id: &str) -> bool {
        match Conversation::load(id) {
            Ok(conv) => {
                let current = self.conversation.lock();
                let _ = current.save();
                drop(current);
                self.conversation = Arc::new(Mutex::new(conv));
                true
            }
            Err(e) => {
                log::warn!("Failed to load conversation {}: {}", id, e);
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// AgentHarness — shared infrastructure (1 per window)
// ---------------------------------------------------------------------------

/// Shared agent infrastructure. Owns the tokio runtime, config, and skills.
/// Does NOT own conversation or channels — those are per-tab in AgentSession.
pub struct AgentHarness {
    config: con_agent::AgentConfig,
    skills_config: crate::config::SkillsConfig,
    skills: SkillRegistry,
    /// Last skill scan request successfully applied. Repeating the exact same
    /// cwd and resolved roots is skipped, but cwd/config changes rebuild the
    /// registry off the UI thread so skill edits are still picked up at the
    /// same points as the old synchronous scanner.
    last_skill_scan_request: Option<SkillScanRequest>,
    latest_requested_skill_scan: Option<SkillScanRequest>,
    in_flight_skill_scans: HashSet<SkillScanRequest>,
    pending_skill_rescans: HashSet<SkillScanRequest>,
    skill_scan_generation: u64,
    runtime: Arc<Runtime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SkillScanCandidateDirs {
    global: Vec<PathBuf>,
    project: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SkillScanRequest {
    generation: u64,
    cwd: String,
    candidates: SkillScanCandidateDirs,
}

#[must_use = "skill scan job results must be delivered to AgentHarness::complete_skill_scan, including failed_result, to clear in-flight state"]
pub struct SkillScanJob {
    request: SkillScanRequest,
}

pub struct SkillScanResult {
    request: SkillScanRequest,
    registry: Option<SkillRegistry>,
    count: usize,
}

pub struct SkillScanCompletion {
    applied: bool,
    follow_up: Option<SkillScanJob>,
}

impl SkillScanCompletion {
    pub fn applied(&self) -> bool {
        self.applied
    }

    pub fn follow_up_job(self) -> Option<SkillScanJob> {
        self.follow_up
    }
}

impl SkillScanJob {
    pub fn failed_result(&self) -> SkillScanResult {
        SkillScanResult {
            request: self.request.clone(),
            registry: None,
            count: 0,
        }
    }

    pub fn scan(self) -> SkillScanResult {
        let global_dirs = existing_skill_dirs(&self.request.candidates.global);
        let project_dirs = existing_skill_dirs(&self.request.candidates.project);

        let mut registry = SkillRegistry::new();
        let count = registry.scan(&global_dirs, &project_dirs);
        SkillScanResult {
            request: self.request,
            registry: Some(registry),
            count,
        }
    }
}

impl AgentHarness {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?,
        );

        Ok(Self {
            config: config.agent.clone(),
            skills_config: config.skills.clone(),
            skills: SkillRegistry::new(),
            last_skill_scan_request: None,
            latest_requested_skill_scan: None,
            in_flight_skill_scans: HashSet::new(),
            pending_skill_rescans: HashSet::new(),
            skill_scan_generation: 0,
            runtime,
        })
    }

    /// Create a SuggestionEngine that shares the harness tokio runtime.
    pub fn suggestion_engine(&self, debounce_ms: u64) -> crate::suggestions::SuggestionEngine {
        crate::suggestions::SuggestionEngine::new(
            self.config.suggestion_agent_config(),
            self.runtime.clone(),
            debounce_ms,
        )
    }

    /// Create a TabSummaryEngine that shares the harness tokio
    /// runtime. Uses the **same suggestion model** the inline shell
    /// completions use — gated by the same `agent.suggestion_model`
    /// toggle, so users who turned suggestions off don't get extra
    /// LLM traffic for tab labels.
    pub fn tab_summary_engine(&self) -> crate::tab_summary::TabSummaryEngine {
        crate::tab_summary::TabSummaryEngine::new(
            self.config.suggestion_agent_config(),
            self.runtime.clone(),
        )
    }

    pub fn prewarm_input_classification(&self) {
        self.runtime.spawn(async move {
            let _ = tokio::task::spawn_blocking(|| {
                let _ = path_executables();
                let _ = shell_aliases();
            })
            .await;
        });
    }

    pub fn spawn_detached<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.runtime.spawn(future);
    }

    pub fn runtime_handle(&self) -> Arc<Runtime> {
        self.runtime.clone()
    }

    /// Classify user input: NLP, command, or skill.
    ///
    /// `is_remote` should be true when the focused pane is an SSH session,
    /// enabling more permissive command detection for remote executables
    /// that aren't on the local $PATH.
    pub fn classify_input(&self, input: &str, is_remote: bool) -> InputKind {
        let trimmed = input.trim();

        if trimmed.starts_with('/') {
            let parts: Vec<&str> = trimmed[1..].splitn(2, ' ').collect();
            let skill_name = parts[0].to_string();
            let args = parts.get(1).map(|s| s.to_string());
            if self.skills.get(&skill_name).is_some() {
                return InputKind::SkillInvoke(skill_name, args);
            }
        }

        if looks_like_command(trimmed, is_remote) {
            return InputKind::ShellCommand(trimmed.to_string());
        }

        InputKind::NaturalLanguage(trimmed.to_string())
    }

    /// Build agent context from Ghostty terminal state.
    pub fn build_context_from_snapshot(
        &self,
        focused_pane_index: usize,
        focused_pane_id: usize,
        focused_observation: &con_agent::context::PaneObservationFrame,
        focused_runtime: &con_agent::context::PaneRuntimeState,
        other_panes: Vec<con_agent::context::PaneSummary>,
    ) -> TerminalContext {
        let cwd = focused_observation.cwd.clone();

        let ssh_host = focused_runtime.remote_host.clone();
        let focused_control = con_agent::control::PaneControlState::from_runtime(focused_runtime);
        let focused_remote_workspace =
            con_agent::context::remote_workspace_anchor(focused_runtime, focused_observation);

        TerminalContext {
            focused_pane_index,
            focused_pane_id,
            focused_hostname: focused_runtime.remote_host.clone(),
            focused_hostname_confidence: focused_runtime.remote_host_confidence,
            focused_hostname_source: focused_runtime.remote_host_source,
            focused_title: focused_observation.title.clone(),
            focused_front_state: focused_runtime.front_state,
            focused_pane_mode: focused_runtime.mode,
            focused_has_shell_integration: focused_observation.has_shell_integration,
            focused_shell_metadata_fresh: focused_runtime.shell_metadata_fresh,
            focused_observation_support: focused_observation.support.clone(),
            focused_runtime_stack: focused_runtime.scope_stack.clone(),
            focused_last_verified_runtime_stack: focused_runtime.last_verified_scope_stack.clone(),
            focused_runtime_warnings: focused_runtime.warnings.clone(),
            focused_control,
            focused_shell_context: focused_runtime.shell_context.clone(),
            focused_shell_context_fresh: focused_runtime.shell_context_fresh,
            focused_recent_actions: focused_runtime.recent_actions.clone(),
            focused_remote_workspace,
            cwd,
            recent_output: focused_observation.recent_output.clone(),
            focused_screen_hints: focused_observation.screen_hints.clone(),
            focused_tmux_snapshot: None,
            last_command: focused_observation.last_command.clone(),
            last_exit_code: focused_observation.last_exit_code,
            last_command_duration_secs: focused_observation.last_command_duration_secs,
            git_branch: None,
            ssh_host,
            tmux_session: focused_runtime.tmux_session.clone(),
            agents_md: None,
            skills: self.skills.summaries(),
            command_history: Vec::new(), // ghostty doesn't track command blocks
            other_panes,
            git_diff: None,
            project_structure: None,
        }
    }

    /// Send a natural language message to the agent using a specific tab's session.
    ///
    /// The session provides the conversation and channels;
    /// the harness provides runtime and config.
    pub fn send_message(
        &self,
        session: &AgentSession,
        agent_config: con_agent::AgentConfig,
        content: String,
        context: TerminalContext,
    ) {
        let user_msg = Message::user(&content);
        session.conversation.lock().add_message(user_msg);

        // Reset cancellation flag for new request
        session.cancel_flag.store(false, Ordering::Relaxed);

        let harness_tx = session.event_tx.clone();
        let conversation = session.conversation.clone();
        let terminal_exec_tx = session.terminal_exec_tx.clone();
        let pane_tx = session.pane_tx.clone();
        let cancelled = session.cancel_flag.clone();

        // Per-request approval channel: the sender goes to the UI via
        // ToolApprovalNeeded events, the receiver goes to the ConHook.
        // This ensures no cross-request interference.
        let (approval_tx, approval_rx) = crossbeam_channel::unbounded();

        self.runtime.spawn(async move {
            let conv_snapshot = conversation.lock().clone();
            let provider = AgentProvider::new(agent_config);
            let request_start = std::time::Instant::now();
            let prepared_context = gather_focused_read_only_facts(pane_tx.clone(), context).await;
            let enriched_context = enrich_context_with_workspace_snapshot(prepared_context).await;

            const MAX_RETRIES: u32 = 3;
            let mut attempt = 0u32;

            loop {
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }

                let (agent_tx, agent_rx) = crossbeam_channel::unbounded();
                let trace = Arc::new(Mutex::new(RequestTrace::default()));

                // Bridge AgentEvent → HarnessEvent on a blocking thread.
                // Recreated per attempt so each stream gets a fresh channel.
                let htx = harness_tx.clone();
                let per_request_approval_tx = approval_tx.clone();
                let trace_for_bridge = trace.clone();
                let bridge = tokio::task::spawn_blocking(move || {
                    let bridge_start = std::time::Instant::now();
                    while let Ok(event) = agent_rx.recv() {
                        let mapped = match event {
                            AgentEvent::Thinking => HarnessEvent::Thinking,
                            AgentEvent::ThinkingDelta(t) => {
                                trace_for_bridge.lock().thinking.push_str(&t);
                                HarnessEvent::ThinkingDelta(t)
                            }
                            AgentEvent::Token(t) => HarnessEvent::Token(t),
                            AgentEvent::Step(s) => {
                                trace_for_bridge.lock().steps.push(s.clone());
                                HarnessEvent::Step(s)
                            }
                            AgentEvent::ToolCallStart {
                                call_id,
                                tool_name,
                                args,
                            } => {
                                let input = serde_json::from_str(&args)
                                    .unwrap_or_else(|_| serde_json::Value::String(args.clone()));
                                trace_for_bridge.lock().steps.push(
                                    con_agent::conversation::AgentStep::ToolCall {
                                        call_id: Some(call_id.clone()),
                                        tool: tool_name.clone(),
                                        input,
                                    },
                                );
                                if is_dangerous(&tool_name) {
                                    let _ = htx.send(HarnessEvent::ToolApprovalNeeded {
                                        call_id: call_id.clone(),
                                        tool_name: tool_name.clone(),
                                        args: args.clone(),
                                        approval_tx: per_request_approval_tx.clone(),
                                    });
                                }
                                HarnessEvent::ToolCallStart {
                                    call_id,
                                    tool_name,
                                    args,
                                }
                            }
                            AgentEvent::ToolCallComplete {
                                call_id,
                                tool_name,
                                result,
                            } => {
                                trace_for_bridge.lock().steps.push(
                                    con_agent::conversation::AgentStep::ToolResult {
                                        call_id: Some(call_id.clone()),
                                        tool: tool_name.clone(),
                                        output: result.clone(),
                                        success: true,
                                    },
                                );
                                HarnessEvent::ToolCallComplete {
                                    call_id,
                                    tool_name,
                                    result,
                                }
                            }
                            AgentEvent::Done(mut m) => {
                                let duration_ms = bridge_start.elapsed().as_millis() as u64;
                                m.duration_ms = Some(duration_ms);
                                HarnessEvent::ResponseComplete(m)
                            }
                            AgentEvent::Error(e) => HarnessEvent::Error(e),
                        };
                        if htx.send(mapped).is_err() {
                            break;
                        }
                    }
                });

                log::info!("[harness] provider.send() attempt {}", attempt + 1);
                let result = provider
                    .send(
                        &conv_snapshot,
                        &enriched_context,
                        agent_tx,
                        approval_rx.clone(),
                        terminal_exec_tx.clone(),
                        pane_tx.clone(),
                        cancelled.clone(),
                    )
                    .await;

                // Wait for bridge to drain before inspecting the result
                let _ = bridge.await;

                match result {
                    Ok(mut assistant_msg) => {
                        let duration_ms = request_start.elapsed().as_millis() as u64;
                        assistant_msg.duration_ms = Some(duration_ms);
                        let trace = trace.lock().clone();
                        assistant_msg.steps = trace.steps;
                        if !trace.thinking.trim().is_empty() {
                            assistant_msg.thinking = Some(trace.thinking);
                        }
                        log::info!(
                            "[harness] provider.send() completed: {} chars in {}ms",
                            assistant_msg.content.len(),
                            duration_ms,
                        );
                        let mut conv = conversation.lock();
                        conv.add_message(assistant_msg);
                        if let Err(e) = conv.save() {
                            log::warn!("Failed to auto-save conversation: {}", e);
                        }
                        break;
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        if attempt < MAX_RETRIES && is_transient_error(&msg) {
                            let delay_ms = 1000 * 2u64.pow(attempt); // 1s, 2s, 4s
                            log::warn!(
                                "[harness] Transient error (attempt {}/{}), retrying in {}ms: {}",
                                attempt + 1,
                                MAX_RETRIES + 1,
                                delay_ms,
                                msg,
                            );
                            let _ = harness_tx.send(HarnessEvent::Error(format!(
                                "Retrying ({}/{})… {}",
                                attempt + 1,
                                MAX_RETRIES,
                                short_error(&msg),
                            )));
                            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                            attempt += 1;
                            continue;
                        }
                        log::error!("[harness] provider.send() failed: {}", msg);
                        let user_msg = friendly_error(&msg);
                        let _ = harness_tx.send(HarnessEvent::Error(user_msg));
                        break;
                    }
                }
            }

            log::info!("[harness] Request complete");
        });
    }

    /// Invoke a skill using the active tab's session.
    pub fn invoke_skill(
        &self,
        session: &AgentSession,
        agent_config: con_agent::AgentConfig,
        skill_name: &str,
        args: Option<&str>,
        context: TerminalContext,
    ) -> Option<String> {
        let skill = self.skills.get(skill_name)?.clone();
        let prompt = if let Some(args) = args {
            format!("{}\n\nAdditional context: {}", skill.prompt_template, args)
        } else {
            skill.prompt_template.clone()
        };
        self.send_message(session, agent_config, prompt, context);
        Some(skill.description.clone())
    }

    pub fn display_label_for_user_message(&self, content: &str) -> Option<String> {
        let mut skills = self.skills.list();
        skills.sort_by_key(|skill| std::cmp::Reverse(skill.prompt_template.len()));

        for skill in skills {
            if content == skill.prompt_template {
                return Some(format!("/{}", skill.name));
            }

            let prefix = format!("{}\n\nAdditional context: ", skill.prompt_template);
            if let Some(rest) = content.strip_prefix(&prefix) {
                let args = rest.trim();
                if args.is_empty() {
                    return Some(format!("/{}", skill.name));
                }
                return Some(format!("/{} {}", skill.name, args));
            }
        }

        None
    }

    pub fn request_skill_scan(&mut self, cwd: &str) -> Option<SkillScanJob> {
        let cwd_path = Path::new(cwd);
        let candidates = SkillScanCandidateDirs {
            global: self.skills_config.resolved_global_paths(),
            project: self.skills_config.resolved_project_paths(cwd_path),
        };

        let request = SkillScanRequest {
            generation: self.skill_scan_generation,
            cwd: cwd.to_string(),
            candidates,
        };

        if self.last_skill_scan_request.as_ref() == Some(&request) {
            return None;
        }

        if !self.in_flight_skill_scans.insert(request.clone()) {
            if self.latest_requested_skill_scan.as_ref() != Some(&request) {
                self.pending_skill_rescans.insert(request.clone());
                self.latest_requested_skill_scan = Some(request);
            }
            return None;
        }
        self.latest_requested_skill_scan = Some(request.clone());

        Some(SkillScanJob { request })
    }

    pub fn complete_skill_scan(&mut self, result: SkillScanResult) -> SkillScanCompletion {
        self.in_flight_skill_scans.remove(&result.request);
        let should_rescan = self.pending_skill_rescans.remove(&result.request);
        if self.latest_requested_skill_scan.as_ref() != Some(&result.request) {
            return SkillScanCompletion {
                applied: false,
                follow_up: None,
            };
        }

        if should_rescan {
            let request = result.request;
            self.in_flight_skill_scans.insert(request.clone());
            self.latest_requested_skill_scan = Some(request.clone());
            return SkillScanCompletion {
                applied: false,
                follow_up: Some(SkillScanJob { request }),
            };
        }

        self.latest_requested_skill_scan = None;

        if result.registry.is_none() {
            log::warn!(
                "Skill scan for {} failed before completion",
                result.request.cwd
            );
            return SkillScanCompletion {
                applied: false,
                follow_up: None,
            };
        }

        SkillScanCompletion {
            applied: self.apply_skill_scan_result(result),
            follow_up: None,
        }
    }

    fn apply_skill_scan_result(&mut self, result: SkillScanResult) -> bool {
        self.last_skill_scan_request = Some(result.request.clone());

        let Some(registry) = result.registry else {
            return false;
        };

        self.skills = registry;
        log::info!(
            "Skill scan for {}: {} skill(s) found",
            result.request.cwd,
            result.count
        );
        true
    }

    /// Update skills config (e.g. after settings change).
    pub fn update_skills_config(&mut self, config: crate::config::SkillsConfig) {
        self.skills_config = config;
        // Force rescan on next cwd check
        self.last_skill_scan_request = None;
        self.latest_requested_skill_scan = None;
        self.in_flight_skill_scans.clear();
        self.pending_skill_rescans.clear();
        self.skill_scan_generation = self.skill_scan_generation.wrapping_add(1);
    }

    pub fn config(&self) -> &con_agent::AgentConfig {
        &self.config
    }

    pub fn update_config(&mut self, config: con_agent::AgentConfig) {
        self.config = config;
    }

    pub fn set_auto_approve(&mut self, enabled: bool) {
        self.config.auto_approve_tools = enabled;
    }

    /// Display name for the active model (e.g. "claude-sonnet-4-6").
    pub fn active_model_name(&self) -> String {
        Self::active_model_name_for(&self.config)
    }

    pub fn active_model_name_for(config: &con_agent::AgentConfig) -> String {
        config.effective_model(&config.provider).to_string()
    }

    pub fn skill_names(&self) -> Vec<String> {
        self.skills.names()
    }

    pub fn skill_summaries(&self) -> Vec<(String, String)> {
        self.skills.summaries()
    }
}

fn existing_skill_dirs(dirs: &[PathBuf]) -> Vec<PathBuf> {
    dirs.iter().filter(|dir| dir.is_dir()).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, SkillsConfig};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let id = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("con-harness-{name}-{}-{id}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_skill(root: &Path, name: &str) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Test skill\n---\n\nUse {name}."),
        )
        .unwrap();
    }

    fn harness_with_skill_paths(global: &Path) -> AgentHarness {
        let mut config = Config::default();
        config.skills = SkillsConfig {
            project_paths: vec![".con/skills".into()],
            global_paths: vec![global.display().to_string()],
        };
        AgentHarness::new(&config).unwrap()
    }

    fn harness_with_global_skill_paths_only(global: &Path) -> AgentHarness {
        let mut config = Config::default();
        config.skills = SkillsConfig {
            project_paths: Vec::new(),
            global_paths: vec![global.display().to_string()],
        };
        AgentHarness::new(&config).unwrap()
    }

    #[test]
    fn async_skill_scan_reloads_when_candidates_change_even_if_existing_roots_do_not() {
        let root = TempDir::new("skill-scan-candidate-change");
        let global = root.path().join("global");
        let cwd_a = root.path().join("a");
        let cwd_b = root.path().join("b");
        fs::create_dir_all(&cwd_a).unwrap();
        fs::create_dir_all(&cwd_b).unwrap();
        write_skill(&global, "global-skill");

        let mut harness = harness_with_skill_paths(&global);

        let first = harness.request_skill_scan(cwd_a.to_str().unwrap()).unwrap();
        let first_result = first.scan();
        assert!(first_result.registry.is_some());
        assert!(harness.complete_skill_scan(first_result).applied());
        assert_eq!(harness.skill_summaries().len(), 1);

        write_skill(&global, "new-global-skill");

        let second = harness.request_skill_scan(cwd_b.to_str().unwrap()).unwrap();
        let second_result = second.scan();
        assert!(second_result.registry.is_some());
        assert!(harness.complete_skill_scan(second_result).applied());

        let names = harness.skill_names();
        assert!(names.contains(&"global-skill".to_string()));
        assert!(names.contains(&"new-global-skill".to_string()));
    }

    #[test]
    fn async_skill_scan_reloads_when_cwd_changes_even_if_candidate_roots_do_not() {
        let root = TempDir::new("skill-scan-cwd-change");
        let global = root.path().join("global");
        let cwd_a = root.path().join("a");
        let cwd_b = root.path().join("b");
        fs::create_dir_all(&cwd_a).unwrap();
        fs::create_dir_all(&cwd_b).unwrap();
        write_skill(&global, "global-skill");

        let mut harness = harness_with_global_skill_paths_only(&global);
        let first = harness.request_skill_scan(cwd_a.to_str().unwrap()).unwrap();
        assert!(harness.complete_skill_scan(first.scan()).applied());

        write_skill(&global, "new-global-skill");

        let second = harness.request_skill_scan(cwd_b.to_str().unwrap()).unwrap();
        assert!(harness.complete_skill_scan(second.scan()).applied());

        let names = harness.skill_names();
        assert!(names.contains(&"global-skill".to_string()));
        assert!(names.contains(&"new-global-skill".to_string()));
    }

    #[test]
    fn async_skill_scan_reloads_when_project_skill_root_appears() {
        let root = TempDir::new("skill-scan-project-root");
        let global = root.path().join("global");
        let cwd_a = root.path().join("a");
        let cwd_b = root.path().join("b");
        fs::create_dir_all(&cwd_a).unwrap();
        fs::create_dir_all(&cwd_b).unwrap();
        write_skill(&global, "global-skill");

        let mut harness = harness_with_skill_paths(&global);
        let first = harness.request_skill_scan(cwd_a.to_str().unwrap()).unwrap();
        assert!(harness.complete_skill_scan(first.scan()).applied());

        let project_skills = cwd_b.join(".con/skills");
        write_skill(&project_skills, "project-skill");

        let second = harness.request_skill_scan(cwd_b.to_str().unwrap()).unwrap();
        let second_result = second.scan();
        assert!(second_result.registry.is_some());
        assert!(harness.complete_skill_scan(second_result).applied());

        let names = harness.skill_names();
        assert!(names.contains(&"global-skill".to_string()));
        assert!(names.contains(&"project-skill".to_string()));
    }

    #[test]
    fn async_skill_scan_ignores_stale_results() {
        let root = TempDir::new("skill-scan-stale-result");
        let global = root.path().join("global");
        let cwd_a = root.path().join("a");
        let cwd_b = root.path().join("b");
        fs::create_dir_all(&cwd_a).unwrap();
        fs::create_dir_all(&cwd_b).unwrap();
        write_skill(&global, "global-skill");
        write_skill(&cwd_a.join(".con/skills"), "a-skill");
        write_skill(&cwd_b.join(".con/skills"), "b-skill");

        let mut harness = harness_with_skill_paths(&global);

        let scan_a = harness.request_skill_scan(cwd_a.to_str().unwrap()).unwrap();
        let scan_b = harness.request_skill_scan(cwd_b.to_str().unwrap()).unwrap();

        assert!(!harness.complete_skill_scan(scan_a.scan()).applied());
        assert!(harness.skill_names().is_empty());

        assert!(harness.complete_skill_scan(scan_b.scan()).applied());
        let names = harness.skill_names();
        assert!(names.contains(&"global-skill".to_string()));
        assert!(names.contains(&"b-skill".to_string()));
        assert!(!names.contains(&"a-skill".to_string()));
    }

    #[test]
    fn duplicate_in_flight_scan_can_become_latest_request_again() {
        let root = TempDir::new("skill-scan-duplicate-in-flight");
        let global = root.path().join("global");
        let cwd_a = root.path().join("a");
        let cwd_b = root.path().join("b");
        fs::create_dir_all(&cwd_a).unwrap();
        fs::create_dir_all(&cwd_b).unwrap();
        write_skill(&global, "global-skill");
        write_skill(&cwd_a.join(".con/skills"), "a-skill");
        write_skill(&cwd_b.join(".con/skills"), "b-skill");

        let mut harness = harness_with_skill_paths(&global);

        let scan_a = harness.request_skill_scan(cwd_a.to_str().unwrap()).unwrap();
        let scan_b = harness.request_skill_scan(cwd_b.to_str().unwrap()).unwrap();
        let stale_a_result = scan_a.scan();
        assert!(
            harness
                .request_skill_scan(cwd_a.to_str().unwrap())
                .is_none()
        );
        write_skill(&cwd_a.join(".con/skills"), "new-a-skill");

        assert!(!harness.complete_skill_scan(scan_b.scan()).applied());
        let completion = harness.complete_skill_scan(stale_a_result);
        assert!(!completion.applied());
        let follow_up = completion.follow_up_job().unwrap();
        assert!(harness.complete_skill_scan(follow_up.scan()).applied());

        let names = harness.skill_names();
        assert!(names.contains(&"global-skill".to_string()));
        assert!(names.contains(&"a-skill".to_string()));
        assert!(names.contains(&"new-a-skill".to_string()));
        assert!(!names.contains(&"b-skill".to_string()));
    }

    #[test]
    fn exact_duplicate_in_flight_scan_stays_noop() {
        let root = TempDir::new("skill-scan-exact-duplicate-in-flight");
        let global = root.path().join("global");
        let cwd = root.path().join("cwd");
        fs::create_dir_all(&cwd).unwrap();
        write_skill(&global, "global-skill");

        let mut harness = harness_with_skill_paths(&global);

        let scan = harness.request_skill_scan(cwd.to_str().unwrap()).unwrap();
        assert!(harness.request_skill_scan(cwd.to_str().unwrap()).is_none());

        let completion = harness.complete_skill_scan(scan.scan());
        assert!(completion.applied());
        assert!(completion.follow_up_job().is_none());

        assert_eq!(harness.skill_names(), vec!["global-skill".to_string()]);
    }

    #[test]
    fn config_update_makes_old_skill_scan_result_stale_without_disturbing_new_scan() {
        let root = TempDir::new("skill-scan-config-generation");
        let global = root.path().join("global");
        let cwd = root.path().join("cwd");
        fs::create_dir_all(&cwd).unwrap();
        write_skill(&global, "global-skill");

        let mut harness = harness_with_skill_paths(&global);

        let old_scan = harness.request_skill_scan(cwd.to_str().unwrap()).unwrap();
        harness.update_skills_config(harness.skills_config.clone());
        write_skill(&global, "new-global-skill");
        let new_scan = harness.request_skill_scan(cwd.to_str().unwrap()).unwrap();

        assert!(!harness.complete_skill_scan(old_scan.scan()).applied());
        assert!(harness.complete_skill_scan(new_scan.scan()).applied());

        let names = harness.skill_names();
        assert!(names.contains(&"global-skill".to_string()));
        assert!(names.contains(&"new-global-skill".to_string()));
    }

    #[test]
    fn failed_async_skill_scan_clears_in_flight_candidate() {
        let root = TempDir::new("skill-scan-failed-result");
        let global = root.path().join("global");
        let cwd = root.path().join("cwd");
        fs::create_dir_all(&cwd).unwrap();
        write_skill(&global, "global-skill");

        let mut harness = harness_with_skill_paths(&global);

        let scan = harness.request_skill_scan(cwd.to_str().unwrap()).unwrap();
        assert!(!harness.complete_skill_scan(scan.failed_result()).applied());

        let retry = harness.request_skill_scan(cwd.to_str().unwrap()).unwrap();
        assert!(harness.complete_skill_scan(retry.scan()).applied());
        assert_eq!(harness.skill_names(), vec!["global-skill".to_string()]);
    }
}

/// Shell builtins that won't appear as executables on $PATH.
/// POSIX + bash/zsh builtins that users commonly type as standalone commands.
const SHELL_BUILTINS: &[&str] = &[
    // POSIX required builtins
    "cd", "echo", "eval", "exec", "exit", "export", "readonly", "return", "set", "shift", "source",
    "test", "times", "trap", "type", "ulimit", "umask", "unset", "wait",
    // bash/zsh builtins commonly typed as commands
    "alias", "bg", "bind", "builtin", "caller", "command", "compgen", "complete", "declare", "dirs",
    "disown", "enable", "fc", "fg", "hash", "help", "history", "jobs", "let", "local", "logout",
    "popd", "pushd", "read", "shopt", "suspend", "typeset", "unalias",
];

/// Scan $PATH once and cache the set of executable names.
fn path_executables() -> &'static HashSet<String> {
    static CACHE: OnceLock<HashSet<String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let path_var = std::env::var("PATH").unwrap_or_default();
        let mut exes = HashSet::with_capacity(2048);
        for dir in std::env::split_paths(&path_var) {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    if let Some(name) = entry.file_name().to_str() {
                        exes.insert(name.to_string());
                    }
                }
            }
        }
        exes
    })
}

/// Discover shell aliases by running `alias` in the user's login shell.
/// Cached on first call. Returns empty set on failure (no blocking, no panic).
fn shell_aliases() -> &'static HashSet<String> {
    static CACHE: OnceLock<HashSet<String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into());
        let output = con_paths::host_command(&shell)
            .args(["-ic", "alias"])
            .stderr(std::process::Stdio::null())
            .output()
            .ok();
        let mut aliases = HashSet::new();
        if let Some(out) = output {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                // zsh: "name=value" or "name='value'"
                // bash: "alias name='value'"
                let line = line.trim();
                let name_part = if let Some(rest) = line.strip_prefix("alias ") {
                    rest
                } else {
                    line
                };
                if let Some((name, _)) = name_part.split_once('=') {
                    let name = name.trim();
                    if !name.is_empty() && is_command_shaped(name) {
                        aliases.insert(name.to_string());
                    }
                }
            }
        }
        log::info!("[classify] discovered {} shell aliases", aliases.len());
        aliases
    })
}

/// Returns true if a token looks like a valid command name:
/// lowercase alphanumeric, hyphens, underscores, dots — no spaces.
fn is_command_shaped(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= 40
        && word.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_' || b == b'.'
        })
}

/// Detect whether input is a shell command or natural language.
///
/// Uses structural signals — no static word list:
/// 1. First token is an executable on $PATH or a shell builtin
/// 2. First token is a path (`./`, `/`, `~/`)
/// 3. Input contains shell operators (`|`, `>`, `>>`, `&&`, `;`)
/// 4. Input contains flag-like arguments (`-x`, `--foo`) with a command-shaped first word
/// 5. First token is an env var assignment (`VAR=value`)
/// 6. Input contains subshell/expansion syntax (`$(...)`, backticks)
///
/// When `is_remote` is true (focused pane is an SSH session), the classification
/// is more permissive: a command-shaped first word alone is enough, since remote
/// executables aren't on the local $PATH. Natural-language signals (question words,
/// articles, pronouns) override this to prevent false positives.
fn looks_like_command(input: &str, is_remote: bool) -> bool {
    let mut words = input.split_whitespace();
    let first_word = match words.next() {
        Some(w) => w,
        None => return false,
    };

    // --- Definitive structural signals (always apply) ---

    // Shell builtins (these are never NL — "cd", "export", etc.)
    if SHELL_BUILTINS.contains(&first_word) {
        return true;
    }

    // Shell aliases (ll, la, gs, gst, etc.) — discovered from user's shell config
    if shell_aliases().contains(first_word) {
        return true;
    }

    // Executable on local $PATH — but only if the surrounding text
    // doesn't read like natural language. Many short English words
    // (`what`, `test`, `time`, `sort`, `make`) are also executables.
    if path_executables().contains(first_word) && !has_natural_language_signals(input) {
        return true;
    }

    // Explicit path invocation
    if first_word.starts_with("./") || first_word.starts_with('/') || first_word.starts_with("~/") {
        return true;
    }

    // Env var assignment: VAR=value or VAR=value command
    if first_word.contains('=') {
        let (name, _) = first_word.split_once('=').unwrap();
        if !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        {
            return true;
        }
    }

    // Shell operators indicate a pipeline/compound command
    if input.contains(" | ")
        || input.contains(" > ")
        || input.contains(" >> ")
        || input.contains(" && ")
        || input.contains(" ; ")
    {
        return true;
    }

    // Subshell / expansion syntax
    if input.contains("$(") || input.contains('`') {
        return true;
    }

    // Flag arguments with a command-shaped first word:
    // "free -g", "docker --version" — NL never uses -flags
    if is_command_shaped(first_word) {
        let has_flags = words.clone().any(|w| w.starts_with('-'));
        if has_flags {
            return true;
        }
    }

    // --- Remote-aware classification ---
    // On SSH sessions, remote executables aren't on local $PATH.
    // A command-shaped first word is likely a remote command — unless
    // the input reads like natural language.
    if is_remote && is_command_shaped(first_word) {
        if !has_natural_language_signals(input) {
            return true;
        }
    }

    false
}

/// Detects natural-language syntax using structural signals only.
///
/// Uses three purely syntactic checks — no word lists:
/// 1. Sentence-ending punctuation: `?` or `.` at the end (shell commands never end this way)
/// 2. First word starts with uppercase (shell executables are lowercase)
/// 3. Contains clause-separating commas (`, ` — distinct from shell comma usage)
/// 4. Multi-word input with no shell argument structure (no flags, paths, or operators)
fn has_natural_language_signals(input: &str) -> bool {
    let trimmed = input.trim();

    // 1. Ends with sentence punctuation — commands never end with ? or .
    //    (note: shell `!` is history expansion, so we skip it)
    if trimmed.ends_with('?') || trimmed.ends_with('.') {
        return true;
    }

    // 2. First character is uppercase — shell commands are lowercase.
    //    Env var assignments (VAR=val) are caught earlier in the pipeline.
    if trimmed.starts_with(|c: char| c.is_ascii_uppercase()) {
        return true;
    }

    // 3. Contains clause-separating comma+space pattern.
    //    Shell uses commas in brace expansion {a,b} but not ", " between words.
    if trimmed.contains(", ") {
        return true;
    }

    // 4. Multi-word input where no token looks like a shell argument.
    //    Shell arguments are: flags (-x, --foo), paths (/foo, ./bar),
    //    env vars ($FOO), globs (*.txt), or quoted strings.
    //    If none of the tokens after the first have these patterns, it's likely prose.
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() >= 3 {
        let has_shell_args = words[1..].iter().any(|w| {
            w.starts_with('-')       // flag
                || w.starts_with('/')    // absolute path
                || w.starts_with("./")   // relative path
                || w.starts_with("~/")   // home path
                || w.starts_with('$')    // variable
                || w.contains('*')       // glob
                || w.contains('=')       // assignment
                || w.starts_with('"')    // quoted string
                || w.starts_with('\'') // quoted string
        });
        if !has_shell_args {
            return true;
        }
    }

    false
}

async fn enrich_context_with_workspace_snapshot(mut context: TerminalContext) -> TerminalContext {
    let Some(cwd) = context.cwd.clone() else {
        return context;
    };

    let Ok((agents_md, git_branch, git_diff, project_structure)) =
        tokio::task::spawn_blocking(move || {
            let cwd_path = Path::new(&cwd);

            let agents_md = std::fs::read_to_string(cwd_path.join("AGENTS.md")).ok();

            let git_branch = con_paths::host_command("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(cwd_path)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

            let git_diff = {
                let stat = con_paths::host_command("git")
                    .args(["diff", "--stat"])
                    .current_dir(cwd_path)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .unwrap_or_default();

                if stat.trim().is_empty() {
                    None
                } else {
                    let diff = con_paths::host_command("git")
                        .args(["diff"])
                        .current_dir(cwd_path)
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                        .unwrap_or_default();

                    const MAX_DIFF_CHARS: usize = 8_000;
                    let mut result = stat;
                    result.push('\n');
                    if diff.len() <= MAX_DIFF_CHARS {
                        result.push_str(&diff);
                    } else {
                        let truncation_point =
                            diff[..MAX_DIFF_CHARS].rfind('\n').unwrap_or(MAX_DIFF_CHARS);
                        result.push_str(&diff[..truncation_point]);
                        result.push_str(&format!(
                            "\n... (diff truncated — {:.0}K of {:.0}K chars shown, see `git diff` for full)",
                            truncation_point as f64 / 1000.0,
                            diff.len() as f64 / 1000.0,
                        ));
                    }
                    Some(result)
                }
            };

            let project_structure = {
                let output = con_paths::host_command("git")
                    .args(["ls-files", "--cached", "--others", "--exclude-standard"])
                    .current_dir(cwd_path)
                    .output()
                    .ok()
                    .filter(|o| o.status.success())
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string());

                let listing = output.or_else(|| {
                    con_paths::host_command("find")
                        .args([
                            ".",
                            "-maxdepth",
                            "3",
                            "-type",
                            "f",
                            "-not",
                            "-path",
                            "./.git/*",
                        ])
                        .current_dir(cwd_path)
                        .output()
                        .ok()
                        .filter(|o| o.status.success())
                        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                });

                listing.and_then(|listing| {
                    if listing.trim().is_empty() {
                        return None;
                    }

                    let file_lines: Vec<&str> = listing.lines().collect();
                    let total = file_lines.len();
                    if total > 200 {
                        Some(format!(
                            "{}\n... ({} more files)",
                            file_lines[..200].join("\n"),
                            total - 200
                        ))
                    } else {
                        Some(file_lines.join("\n"))
                    }
                })
            };

            (agents_md, git_branch, git_diff, project_structure)
        })
        .await
    else {
        return context;
    };

    context.agents_md = agents_md;
    context.git_branch = git_branch;
    context.git_diff = git_diff;
    context.project_structure = project_structure;
    context
}

async fn gather_focused_read_only_facts(
    pane_tx: Sender<PaneRequest>,
    context: TerminalContext,
) -> TerminalContext {
    auto_query_focused_tmux_snapshot(pane_tx, context).await
}

async fn auto_query_focused_tmux_snapshot(
    pane_tx: Sender<PaneRequest>,
    mut context: TerminalContext,
) -> TerminalContext {
    if context.focused_tmux_snapshot.is_some()
        || !context
            .focused_control
            .capabilities
            .contains(&con_agent::PaneControlCapability::QueryTmux)
    {
        return context;
    }

    let pane_index = context.focused_pane_index;
    let pane_id = context.focused_pane_id;
    let (response_tx, response_rx) = crossbeam_channel::bounded(1);
    if pane_tx
        .send(PaneRequest {
            query: con_agent::PaneQuery::TmuxList {
                pane: con_agent::tools::PaneSelector::new(Some(pane_index), Some(pane_id)),
            },
            response_tx,
        })
        .is_err()
    {
        return context;
    }

    let response = match tokio::task::spawn_blocking(move || {
        response_rx.recv_timeout(std::time::Duration::from_secs(12))
    })
    .await
    {
        Ok(Ok(response)) => response,
        Ok(Err(_)) | Err(_) => return context,
    };

    match response {
        con_agent::PaneResponse::TmuxList(snapshot) => {
            if context.tmux_session.is_none() {
                context.tmux_session = snapshot
                    .panes
                    .iter()
                    .find(|pane| pane.pane_active)
                    .or_else(|| snapshot.panes.first())
                    .map(|pane| pane.session_name.clone());
            }
            context.focused_tmux_snapshot = Some(snapshot);
            context
        }
        con_agent::PaneResponse::Error(err) => {
            log::debug!(
                "[harness] focused tmux auto-query unavailable for pane {}: {}",
                pane_index,
                err
            );
            context
        }
        other => {
            log::debug!(
                "[harness] focused tmux auto-query returned unexpected response for pane {}: {:?}",
                pane_index,
                other
            );
            context
        }
    }
}

/// Classify errors as transient (worth retrying) vs permanent.
/// Produce a user-friendly error message from a verbose provider error.
/// Detects known failure patterns and translates them into actionable guidance.
fn friendly_error(msg: &str) -> String {
    // Empty tool_call_id: the streaming parser failed to parse the model's
    // tool call (missing name/id). This is a provider compatibility issue.
    if msg.contains("tool_call_id") && msg.contains("invalid") {
        return format!(
            "The model returned a malformed tool call (empty name or call_id). \
             This is a known compatibility issue with some providers' streaming format. \
             Try a different model, or retry the request.\n\nRaw error: {}",
            short_error_detail(msg),
        );
    }
    // ToolNotFoundError with empty name — same root cause
    if msg.contains("ToolNotFoundError") && msg.contains("\"\"") {
        return "The model returned a tool call with an empty name. \
                This usually means the provider's streaming format is incompatible. \
                Try a different model."
            .to_string();
    }
    msg.to_string()
}

/// Extract the JSON error body from a verbose provider error, if present.
fn short_error_detail(msg: &str) -> &str {
    // Look for the JSON portion: {"type":"error",...}
    if let Some(start) = msg.find("{\"type\":") {
        &msg[start..]
    } else {
        msg
    }
}

/// Rig surfaces HTTP errors as stringified messages, so we match on substrings.
fn is_transient_error(msg: &str) -> bool {
    // HTTP 429 rate limit
    msg.contains("429") || msg.contains("rate_limit") || msg.contains("Rate limit")
    // Server-side transient
    || msg.contains("500") || msg.contains("502") || msg.contains("503")
    || msg.contains("Internal Server Error") || msg.contains("Bad Gateway")
    || msg.contains("Service Unavailable") || msg.contains("overloaded")
    // Network transient
    || msg.contains("connection reset") || msg.contains("timed out")
    || msg.contains("Connection reset")
}

/// Extract a short, user-facing error summary from a verbose provider error.
fn short_error(msg: &str) -> &str {
    if msg.contains("429") || msg.contains("rate_limit") {
        "rate limited"
    } else if msg.contains("500") || msg.contains("Internal Server Error") {
        "server error (500)"
    } else if msg.contains("502") || msg.contains("Bad Gateway") {
        "bad gateway (502)"
    } else if msg.contains("503") || msg.contains("Service Unavailable") {
        "service unavailable (503)"
    } else if msg.contains("overloaded") {
        "provider overloaded"
    } else if msg.contains("timed out") {
        "request timed out"
    } else if msg.contains("connection reset") || msg.contains("Connection reset") {
        "connection reset"
    } else {
        "transient error"
    }
}
