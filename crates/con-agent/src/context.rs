use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::control::{
    PaneAddressSpace, PaneControlCapability, PaneControlChannel, PaneControlState,
    PaneVisibleTarget, PaneVisibleTargetKind, TmuxControlMode, format_control_attachments,
    format_target_stack,
};
use crate::shell_probe::{ShellProbeResult, ShellProbeTmuxContext};
use crate::tmux::TmuxSnapshot;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PaneMode {
    Shell,
    Multiplexer,
    Tui,
    #[default]
    Unknown,
}

impl PaneMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::Multiplexer => "multiplexer",
            Self::Tui => "tui",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PaneFrontState {
    ShellPrompt,
    InteractiveSurface,
    #[default]
    Unknown,
}

impl PaneFrontState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ShellPrompt => "shell_prompt",
            Self::InteractiveSurface => "interactive_surface",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneScopeKind {
    Shell,
    RemoteShell,
    Multiplexer,
    InteractiveApp,
    AgentCli,
}

impl PaneScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::RemoteShell => "remote_shell",
            Self::Multiplexer => "multiplexer",
            Self::InteractiveApp => "interactive_app",
            Self::AgentCli => "agent_cli",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneEvidenceSource {
    ShellIntegration,
    SurfaceState,
    Osc7,
    CommandLine,
    ShellProbe,
    ActionHistory,
    Title,
    ScreenStructure,
}

impl PaneEvidenceSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ShellIntegration => "shell_integration",
            Self::SurfaceState => "surface_state",
            Self::Osc7 => "osc7",
            Self::CommandLine => "command_line",
            Self::ShellProbe => "shell_probe",
            Self::ActionHistory => "action_history",
            Self::Title => "title",
            Self::ScreenStructure => "screen_structure",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneConfidence {
    Strong,
    Advisory,
}

impl PaneConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Advisory => "advisory",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneRuntimeScope {
    pub kind: PaneScopeKind,
    pub label: Option<String>,
    pub host: Option<String>,
    pub confidence: PaneConfidence,
    pub evidence_source: PaneEvidenceSource,
}

impl PaneRuntimeScope {
    pub fn summary(&self) -> String {
        match (self.kind, self.label.as_deref(), self.host.as_deref()) {
            (PaneScopeKind::RemoteShell, _, Some(host)) => format!("remote_shell({host})"),
            (_, Some(label), _) => format!("{}({label})", self.kind.as_str()),
            _ => self.kind.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneActionKind {
    PaneCreated,
    VisibleShellExec,
    RawInput,
    ShellProbe,
    ProcessExited,
}

impl PaneActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PaneCreated => "pane_created",
            Self::VisibleShellExec => "visible_shell_exec",
            Self::RawInput => "raw_input",
            Self::ShellProbe => "shell_probe",
            Self::ProcessExited => "process_exited",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneActionRecord {
    pub sequence: u64,
    pub kind: PaneActionKind,
    pub summary: String,
    pub command: Option<String>,
    pub source: PaneEvidenceSource,
    pub confidence: PaneConfidence,
    pub input_generation: Option<u64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneShellContext {
    pub captured_input_generation: u64,
    pub host: Option<String>,
    pub pwd: Option<String>,
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub ssh_connection: Option<String>,
    pub ssh_tty: Option<String>,
    pub tmux_env: Option<String>,
    pub nvim_listen_address: Option<String>,
    pub tmux: Option<ShellProbeTmuxContext>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteWorkspaceAnchor {
    pub host: String,
    pub source: PaneEvidenceSource,
    pub confidence: PaneConfidence,
    pub note: String,
}

impl PaneShellContext {
    fn from_probe(result: &ShellProbeResult, captured_input_generation: u64) -> Self {
        Self {
            captured_input_generation,
            host: result.host.clone(),
            pwd: result.pwd.clone(),
            term: result.term.clone(),
            term_program: result.term_program.clone(),
            ssh_connection: result.ssh_connection.clone(),
            ssh_tty: result.ssh_tty.clone(),
            tmux_env: result.tmux_env.clone(),
            nvim_listen_address: result.nvim_listen_address.clone(),
            tmux: result.tmux.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneObservationSupport {
    /// Whether the backend can provide authoritative foreground command text.
    pub foreground_command: bool,
    /// Whether the backend can provide authoritative alternate-screen state.
    pub alternate_screen: bool,
    /// Whether the backend can provide authoritative remote-host identity.
    pub remote_host_identity: bool,
}

impl PaneObservationSupport {
    pub fn backend_limit_note(&self) -> Option<String> {
        let mut missing = Vec::new();
        if !self.foreground_command {
            missing.push("foreground command text");
        }
        if !self.alternate_screen {
            missing.push("alternate-screen state");
        }
        if !self.remote_host_identity {
            missing.push("remote-host identity");
        }

        if missing.is_empty() {
            return None;
        }

        Some(format!(
            "Embedded Ghostty does not currently export authoritative {} for this pane. Unproven foreground runtimes must stay unknown.",
            missing.join(", ")
        ))
    }
}

impl Default for PaneObservationSupport {
    fn default() -> Self {
        Self {
            foreground_command: false,
            alternate_screen: false,
            remote_host_identity: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaneObservationFrame {
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub recent_output: Vec<String>,
    pub screen_hints: Vec<PaneObservationHint>,
    pub last_command: Option<String>,
    pub last_exit_code: Option<i32>,
    pub last_command_duration_secs: Option<f64>,
    pub support: PaneObservationSupport,
    pub has_shell_integration: bool,
    pub is_alt_screen: bool,
    pub is_busy: bool,
    pub input_generation: u64,
    pub last_command_finished_input_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaneRuntimeState {
    pub front_state: PaneFrontState,
    pub mode: PaneMode,
    pub shell_metadata_fresh: bool,
    pub screen_prompt_like: bool,
    pub screen_tmux_like: bool,
    pub screen_ssh_disconnected: bool,
    pub remote_host: Option<String>,
    pub remote_host_confidence: Option<PaneConfidence>,
    pub remote_host_source: Option<PaneEvidenceSource>,
    pub agent_cli: Option<String>,
    pub tmux_session: Option<String>,
    pub last_verified_scope_stack: Vec<PaneRuntimeScope>,
    pub last_verified_tmux_session: Option<String>,
    pub shell_context: Option<PaneShellContext>,
    pub shell_context_fresh: bool,
    pub active_scope: Option<PaneRuntimeScope>,
    pub evidence: Vec<PaneEvidence>,
    pub scope_stack: Vec<PaneRuntimeScope>,
    pub recent_actions: Vec<PaneActionRecord>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneObservationHintKind {
    PromptLikeInput,
    HtopLikeScreen,
    LoginBannerVisible,
    SshConnectionClosed,
    TmuxLikeScreen,
}

impl PaneObservationHintKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PromptLikeInput => "prompt_like_input",
            Self::HtopLikeScreen => "htop_like_screen",
            Self::LoginBannerVisible => "login_banner_visible",
            Self::SshConnectionClosed => "ssh_connection_closed",
            Self::TmuxLikeScreen => "tmux_like_screen",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneObservationHint {
    pub kind: PaneObservationHintKind,
    pub confidence: PaneConfidence,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabWorkspaceKind {
    LocalShell,
    RemoteShell,
    TmuxWorkspace,
    Unknown,
}

impl TabWorkspaceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalShell => "local_shell",
            Self::RemoteShell => "remote_shell",
            Self::TmuxWorkspace => "tmux_workspace",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TabWorkspaceState {
    Ready,
    NeedsInspection,
    Disconnected,
    Interactive,
    Unknown,
}

impl TabWorkspaceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsInspection => "needs_inspection",
            Self::Disconnected => "disconnected",
            Self::Interactive => "interactive",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TabWorkspaceSummary {
    pub pane_index: usize,
    pub pane_id: usize,
    pub host: Option<String>,
    pub tmux_session: Option<String>,
    pub cwd: Option<String>,
    pub agent_cli: Option<String>,
    pub kind: TabWorkspaceKind,
    pub state: TabWorkspaceState,
    pub note: String,
}

impl PaneRuntimeState {
    pub fn from_observation(observation: &PaneObservationFrame) -> Self {
        PaneRuntimeTracker::default().observe(observation.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneEvidence {
    pub subject: String,
    pub value: Option<String>,
    pub source: PaneEvidenceSource,
    pub confidence: PaneConfidence,
    pub generation: u64,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
struct TrackedShellContext {
    result: ShellProbeResult,
    captured_input_generation: u64,
}

#[derive(Debug, Clone)]
pub enum PaneRuntimeEvent {
    PaneCreated {
        startup_command: Option<String>,
    },
    VisibleShellExec {
        command: String,
        input_generation: u64,
    },
    RawInput {
        keys: String,
        input_generation: u64,
    },
    ShellProbe {
        result: ShellProbeResult,
        captured_input_generation: u64,
    },
    ProcessExited,
}

#[derive(Debug, Clone, Default)]
pub struct PaneRuntimeTracker {
    generation: u64,
    event_sequence: u64,
    shell_context: Option<TrackedShellContext>,
    recent_actions: VecDeque<PaneActionRecord>,
}

impl PaneRuntimeTracker {
    pub fn record_action(&mut self, event: PaneRuntimeEvent) {
        self.event_sequence += 1;
        let sequence = self.event_sequence;

        match event {
            PaneRuntimeEvent::PaneCreated { startup_command } => {
                let summary = startup_command
                    .as_ref()
                    .map(|command| {
                        format!("con created this pane with startup command `{command}`")
                    })
                    .unwrap_or_else(|| "con created this pane".to_string());
                self.push_recent_action(PaneActionRecord {
                    sequence,
                    kind: PaneActionKind::PaneCreated,
                    summary,
                    command: startup_command.clone(),
                    source: PaneEvidenceSource::ActionHistory,
                    confidence: PaneConfidence::Advisory,
                    input_generation: None,
                    note: startup_command.as_deref().and_then(command_intent_note),
                });
            }
            PaneRuntimeEvent::VisibleShellExec {
                command,
                input_generation,
            } => {
                self.push_recent_action(PaneActionRecord {
                    sequence,
                    kind: PaneActionKind::VisibleShellExec,
                    summary: format!("con executed `{command}` in the visible shell"),
                    command: Some(command.clone()),
                    source: PaneEvidenceSource::ActionHistory,
                    confidence: PaneConfidence::Advisory,
                    input_generation: Some(input_generation),
                    note: command_intent_note(&command),
                });
            }
            PaneRuntimeEvent::RawInput {
                keys,
                input_generation,
            } => {
                self.push_recent_action(PaneActionRecord {
                    sequence,
                    kind: PaneActionKind::RawInput,
                    summary: format!("con sent raw input `{}`", summarize_raw_input(&keys)),
                    command: None,
                    source: PaneEvidenceSource::ActionHistory,
                    confidence: PaneConfidence::Advisory,
                    input_generation: Some(input_generation),
                    note: Some(
                        "Raw input describes what con sent to the pane, not what the foreground app proved in response."
                            .to_string(),
                    ),
                });
            }
            PaneRuntimeEvent::ShellProbe {
                result,
                captured_input_generation,
            } => {
                self.shell_context = Some(TrackedShellContext {
                    result: result.clone(),
                    captured_input_generation,
                });
                self.push_recent_action(PaneActionRecord {
                    sequence,
                    kind: PaneActionKind::ShellProbe,
                    summary: summarize_shell_probe(&result),
                    command: None,
                    source: PaneEvidenceSource::ShellProbe,
                    confidence: PaneConfidence::Strong,
                    input_generation: Some(captured_input_generation),
                    note: Some(
                        "This probe describes the shell frame that was visible when con ran the probe."
                            .to_string(),
                    ),
                });
            }
            PaneRuntimeEvent::ProcessExited => {
                self.shell_context = None;
                self.push_recent_action(PaneActionRecord {
                    sequence,
                    kind: PaneActionKind::ProcessExited,
                    summary: "the pane process exited".to_string(),
                    command: None,
                    source: PaneEvidenceSource::ActionHistory,
                    confidence: PaneConfidence::Strong,
                    input_generation: None,
                    note: None,
                });
            }
        }
    }

    pub fn observe(&mut self, observation: PaneObservationFrame) -> PaneRuntimeState {
        self.generation += 1;
        let generation = self.generation;

        let shell_metadata_fresh = shell_metadata_is_fresh(
            observation.has_shell_integration,
            observation.input_generation,
            observation.last_command_finished_input_generation,
        );

        let shell_context = self.shell_context.as_ref().map(|context| {
            PaneShellContext::from_probe(&context.result, context.captured_input_generation)
        });
        let shell_context_fresh = self.shell_context.as_ref().is_some_and(|context| {
            shell_metadata_fresh
                && observation.input_generation == context.captured_input_generation
        });
        let screen_prompt_like = observation
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput);
        let screen_tmux_like = observation
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
        let screen_ssh_disconnected = observation
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);

        let (
            remote_host,
            remote_host_confidence,
            remote_host_source,
            remote_scope,
            remote_host_evidence,
        ) = remote_host_from_shell_context(
            self.shell_context.as_ref(),
            shell_context_fresh,
            generation,
        );
        let tmux_scope = tmux_scope_from_shell_context(
            self.shell_context.as_ref(),
            shell_context_fresh,
            remote_host.as_deref(),
        );
        let action_tmux_session = self
            .recent_actions
            .iter()
            .rev()
            .find_map(tmux_session_from_action_record);
        let interactive_scope = if observation.support.alternate_screen && observation.is_alt_screen
        {
            Some(PaneRuntimeScope {
                kind: PaneScopeKind::InteractiveApp,
                label: None,
                host: remote_host.clone(),
                confidence: PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::SurfaceState,
            })
        } else {
            None
        };

        let mut last_verified_scope_stack =
            shell_context_scope_stack(self.shell_context.as_ref(), shell_context_fresh);
        let current_scope_stack = if shell_metadata_fresh {
            let mut stack = Vec::new();
            if let Some(scope) = remote_scope {
                stack.push(scope);
            }
            if let Some(scope) = tmux_scope.clone() {
                stack.push(scope);
            }
            stack.push(PaneRuntimeScope {
                kind: PaneScopeKind::Shell,
                label: None,
                host: remote_host.clone(),
                confidence: PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::ShellIntegration,
            });
            stack
        } else if let Some(scope) = interactive_scope.clone() {
            vec![scope]
        } else {
            Vec::new()
        };
        if last_verified_scope_stack.is_empty() && shell_metadata_fresh {
            last_verified_scope_stack = current_scope_stack.clone();
        }

        let front_scope = current_scope_stack.last().cloned();
        let front_state = if shell_metadata_fresh {
            PaneFrontState::ShellPrompt
        } else if observation.support.alternate_screen && observation.is_alt_screen {
            PaneFrontState::InteractiveSurface
        } else {
            PaneFrontState::Unknown
        };

        let mode = match front_state {
            PaneFrontState::ShellPrompt => PaneMode::Shell,
            PaneFrontState::InteractiveSurface => PaneMode::Tui,
            PaneFrontState::Unknown => PaneMode::Unknown,
        };

        let mut evidence = Vec::new();

        if shell_metadata_fresh {
            evidence.push(PaneEvidence {
                subject: "shell_prompt".to_string(),
                value: Some("confirmed".to_string()),
                source: PaneEvidenceSource::ShellIntegration,
                confidence: PaneConfidence::Strong,
                generation: self.generation,
                note: Some(
                    "Ghostty shell integration observed a clean shell prompt after the most recent input.".to_string(),
                ),
            });
        }

        if let Some(evidence_item) = remote_host_evidence {
            evidence.push(evidence_item);
        }

        if let Some(multiplexer) = tmux_scope.as_ref() {
            evidence.push(PaneEvidence {
                subject: "multiplexer".to_string(),
                value: multiplexer.label.clone(),
                source: multiplexer.evidence_source,
                confidence: multiplexer.confidence,
                generation,
                note: Some(
                    "A typed shell probe confirmed that the current shell prompt is nested inside tmux."
                        .to_string(),
                ),
            });
        }

        if let Some(scope) = interactive_scope.as_ref() {
            evidence.push(PaneEvidence {
                subject: "interactive_surface".to_string(),
                value: None,
                source: PaneEvidenceSource::SurfaceState,
                confidence: scope.confidence,
                generation,
                note: Some(
                    "Ghostty reports alternate-screen mode for the visible surface.".to_string(),
                ),
            });
        }

        if let Some(context) = shell_context.as_ref() {
            evidence.push(PaneEvidence {
                subject: "shell_probe".to_string(),
                value: context.host.clone().or_else(|| context.pwd.clone()),
                source: PaneEvidenceSource::ShellProbe,
                confidence: if shell_context_fresh {
                    PaneConfidence::Strong
                } else {
                    PaneConfidence::Advisory
                },
                generation,
                note: Some(if shell_context_fresh {
                    "A typed shell probe matches the current visible shell frame.".to_string()
                } else {
                    "The last typed shell probe describes an earlier shell frame.".to_string()
                }),
            });
        }

        if shell_metadata_fresh
            && shell_context
                .as_ref()
                .and_then(|context| context.tmux.as_ref())
                .is_none()
        {
            if let Some(session) = action_tmux_session.as_ref() {
                evidence.push(PaneEvidence {
                    subject: "tmux_shell_anchor".to_string(),
                    value: Some(session.clone()),
                    source: PaneEvidenceSource::ActionHistory,
                    confidence: PaneConfidence::Advisory,
                    generation,
                    note: Some(
                        "A recent con-executed tmux command targeted this session from the current fresh shell prompt. con can use that shell as a tmux control anchor while the prompt remains fresh."
                            .to_string(),
                    ),
                });
            }
        }

        let active_scope = front_scope;
        let tmux_session = current_scope_stack
            .iter()
            .find(|scope| scope.kind == PaneScopeKind::Multiplexer)
            .and_then(|scope| scope.label.clone());
        let last_verified_tmux_session = last_verified_scope_stack
            .iter()
            .find(|scope| scope.kind == PaneScopeKind::Multiplexer)
            .and_then(|scope| scope.label.clone())
            .or(action_tmux_session);
        let agent_cli =
            classify_screen_agent_cli(observation.title.as_deref(), &observation.recent_output)
                .map(ToString::to_string);

        let mut warnings = Vec::new();
        if !shell_metadata_fresh {
            warnings.push(
                "Visible shell prompt is not confirmed. Treat cwd and last_command as historical shell metadata, not foreground-app truth.".to_string(),
            );
        }
        if shell_context.is_some() && !shell_context_fresh {
            warnings.push(
                "The last shell probe is historical. It can explain the last verified shell frame, but it does not prove the current foreground target.".to_string(),
            );
        }
        if let Some(note) = observation.support.backend_limit_note() {
            warnings.push(note);
        }
        if current_scope_stack.is_empty() && !last_verified_scope_stack.is_empty() {
            warnings.push(format!(
                "Last verified shell frame: {}.",
                format_runtime_stack(&last_verified_scope_stack)
            ));
        }

        PaneRuntimeState {
            front_state,
            mode,
            shell_metadata_fresh,
            screen_prompt_like,
            screen_tmux_like,
            screen_ssh_disconnected,
            remote_host,
            remote_host_confidence,
            remote_host_source,
            agent_cli,
            tmux_session,
            last_verified_scope_stack,
            last_verified_tmux_session,
            shell_context,
            shell_context_fresh,
            active_scope,
            evidence,
            scope_stack: current_scope_stack,
            recent_actions: self.recent_actions.iter().cloned().collect(),
            warnings,
        }
    }

    fn push_recent_action(&mut self, action: PaneActionRecord) {
        const MAX_RECENT_ACTIONS: usize = 8;
        self.recent_actions.push_back(action);
        while self.recent_actions.len() > MAX_RECENT_ACTIONS {
            self.recent_actions.pop_front();
        }
    }
}

fn summarize_raw_input(keys: &str) -> String {
    let escaped: String = keys.chars().flat_map(char::escape_default).collect();
    let mut preview = escaped;
    if preview.len() > 48 {
        preview.truncate(48);
        preview.push_str("...");
    }
    preview
}

fn summarize_shell_probe(result: &ShellProbeResult) -> String {
    let mut parts = Vec::new();
    if let Some(host) = &result.host {
        parts.push(format!("host `{host}`"));
    }
    if let Some(tmux) = &result.tmux {
        if let Some(session) = &tmux.session_name {
            parts.push(format!("tmux session `{session}`"));
        } else {
            parts.push("tmux context".to_string());
        }
        if let Some(pane) = &tmux.pane_id {
            parts.push(format!("pane `{pane}`"));
        }
    }
    if let Some(path) = &result.nvim_listen_address {
        parts.push(format!("nvim socket `{path}`"));
    }

    if parts.is_empty() {
        "con probed the visible shell context".to_string()
    } else {
        format!(
            "con probed the visible shell context and captured {}",
            parts.join(", ")
        )
    }
}

fn command_intent_note(command: &str) -> Option<String> {
    if looks_like_tmux_command(command) {
        return Some(
            "This command targets tmux/tmate. Treat it as causal history about how con entered a multiplexer, not as proof that tmux is still front-most now."
                .to_string(),
        );
    }

    if is_agent_cli_command(command) {
        return Some(
            "This command launched a supported agent CLI. Treat it as causal history until newer pane evidence proves what is currently visible."
                .to_string(),
        );
    }

    if command_basename(command).as_deref() == Some("ssh") {
        return Some(
            "This command opened an SSH connection. It is useful history, but remote identity is only proven after a typed shell probe or backend export."
                .to_string(),
        );
    }

    None
}

fn remote_host_from_shell_context(
    shell_context: Option<&TrackedShellContext>,
    shell_context_fresh: bool,
    generation: u64,
) -> (
    Option<String>,
    Option<PaneConfidence>,
    Option<PaneEvidenceSource>,
    Option<PaneRuntimeScope>,
    Option<PaneEvidence>,
) {
    let Some(context) = shell_context else {
        return (None, None, None, None, None);
    };
    let Some(host) = context.result.host.clone() else {
        return (None, None, None, None, None);
    };
    if context.result.ssh_connection.is_none() {
        return (None, None, None, None, None);
    }

    let confidence = if shell_context_fresh {
        PaneConfidence::Strong
    } else {
        PaneConfidence::Advisory
    };
    let source = PaneEvidenceSource::ShellProbe;

    (
        if shell_context_fresh {
            Some(host.clone())
        } else {
            None
        },
        if shell_context_fresh {
            Some(confidence)
        } else {
            None
        },
        if shell_context_fresh {
            Some(source)
        } else {
            None
        },
        Some(PaneRuntimeScope {
            kind: PaneScopeKind::RemoteShell,
            label: Some(host.clone()),
            host: Some(host.clone()),
            confidence,
            evidence_source: source,
        }),
        Some(PaneEvidence {
            subject: "remote_host".to_string(),
            value: Some(host),
            source,
            confidence,
            generation,
            note: Some(if shell_context_fresh {
                "The last shell probe confirmed that the visible shell is running on a remote host."
                    .to_string()
            } else {
                "A historical shell probe previously confirmed remote shell context for this pane."
                    .to_string()
            }),
        }),
    )
}

fn shell_context_scope_stack(
    shell_context: Option<&TrackedShellContext>,
    shell_context_fresh: bool,
) -> Vec<PaneRuntimeScope> {
    let Some(context) = shell_context else {
        return Vec::new();
    };

    let confidence = if shell_context_fresh {
        PaneConfidence::Strong
    } else {
        PaneConfidence::Advisory
    };
    let host = context
        .result
        .host
        .clone()
        .filter(|_| context.result.ssh_connection.is_some());

    let mut scopes = Vec::new();
    if let Some(host) = host.clone() {
        scopes.push(PaneRuntimeScope {
            kind: PaneScopeKind::RemoteShell,
            label: Some(host.clone()),
            host: Some(host),
            confidence,
            evidence_source: PaneEvidenceSource::ShellProbe,
        });
    }
    if let Some(tmux) = &context.result.tmux {
        scopes.push(PaneRuntimeScope {
            kind: PaneScopeKind::Multiplexer,
            label: tmux
                .session_name
                .clone()
                .or_else(|| Some("tmux".to_string())),
            host: host.clone(),
            confidence,
            evidence_source: PaneEvidenceSource::ShellProbe,
        });
    }
    scopes.push(PaneRuntimeScope {
        kind: PaneScopeKind::Shell,
        label: None,
        host,
        confidence,
        evidence_source: PaneEvidenceSource::ShellProbe,
    });
    scopes
}

fn tmux_scope_from_shell_context(
    shell_context: Option<&TrackedShellContext>,
    shell_context_fresh: bool,
    remote_host: Option<&str>,
) -> Option<PaneRuntimeScope> {
    let context = shell_context?;
    let tmux = context.result.tmux.as_ref()?;
    if !shell_context_fresh {
        return None;
    }

    Some(PaneRuntimeScope {
        kind: PaneScopeKind::Multiplexer,
        label: tmux
            .session_name
            .clone()
            .or_else(|| Some("tmux".to_string())),
        host: remote_host.map(str::to_string),
        confidence: PaneConfidence::Strong,
        evidence_source: PaneEvidenceSource::ShellProbe,
    })
}

fn looks_like_tmux_command(command: &str) -> bool {
    command_basename(command)
        .as_deref()
        .is_some_and(|name| matches!(name, "tmux" | "tmate"))
}

fn is_env_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn command_basename(command: &str) -> Option<String> {
    let mut tokens = command.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        let token = token.trim_matches(&['"', '\''][..]);
        if token.is_empty() {
            continue;
        }
        if is_env_assignment(token) || matches!(token, "env" | "sudo" | "command" | "nohup") {
            continue;
        }
        let basename = token
            .rsplit('/')
            .next()
            .unwrap_or(token)
            .to_ascii_lowercase();
        if !basename.is_empty() {
            return Some(basename);
        }
    }
    None
}

fn parse_tmux_target(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for window in tokens.windows(2) {
        if matches!(window[0], "-t" | "-s") {
            return Some(window[1].trim_matches(&['"', '\''][..]).to_string());
        }
        if let Some(rest) = window[0].strip_prefix("-t") {
            if !rest.is_empty() {
                return Some(rest.trim_matches(&['"', '\''][..]).to_string());
            }
        }
        if let Some(rest) = window[0].strip_prefix("-s") {
            if !rest.is_empty() {
                return Some(rest.trim_matches(&['"', '\''][..]).to_string());
            }
        }
    }
    None
}

pub fn detect_tmux_session(last_command: Option<&str>) -> Option<String> {
    last_command
        .filter(|command| looks_like_tmux_command(command))
        .and_then(parse_tmux_target)
}

pub(crate) fn tmux_session_from_action_record(action: &PaneActionRecord) -> Option<String> {
    if !matches!(
        action.kind,
        PaneActionKind::PaneCreated | PaneActionKind::VisibleShellExec
    ) {
        return None;
    }
    detect_tmux_session(action.command.as_deref())
}

fn is_agent_cli_command(command: &str) -> bool {
    command_basename(command).as_deref().is_some_and(|name| {
        matches!(
            name,
            "claude" | "claude-code" | "codex" | "opencode" | "open-code"
        )
    })
}

pub fn infer_pane_mode(
    _last_command: Option<&str>,
    has_shell_integration: bool,
    is_alt_screen: bool,
    input_generation: u64,
    last_command_finished_input_generation: u64,
) -> PaneMode {
    if is_alt_screen {
        return PaneMode::Tui;
    }

    if shell_metadata_is_fresh(
        has_shell_integration,
        input_generation,
        last_command_finished_input_generation,
    ) {
        return PaneMode::Shell;
    }

    PaneMode::Unknown
}

pub fn shell_metadata_is_fresh(
    has_shell_integration: bool,
    input_generation: u64,
    last_command_finished_input_generation: u64,
) -> bool {
    has_shell_integration && input_generation == last_command_finished_input_generation
}

pub fn direct_terminal_exec_is_safe(runtime: &PaneRuntimeState) -> bool {
    PaneControlState::from_runtime(runtime).allows_visible_shell_exec()
}

/// Terminal context extracted for the AI agent.
/// This is what makes con's agent smarter than a generic chatbot —
/// it always knows what the user is doing in their terminal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalContext {
    /// 1-based index of the focused pane
    pub focused_pane_index: usize,
    /// Stable pane id for the focused pane within the current tab lifetime.
    pub focused_pane_id: usize,
    /// Effective remote hostname of the focused pane when detected.
    pub focused_hostname: Option<String>,
    /// Confidence for the focused remote hostname, when known.
    pub focused_hostname_confidence: Option<PaneConfidence>,
    /// Evidence source for the focused remote hostname, when known.
    pub focused_hostname_source: Option<PaneEvidenceSource>,
    /// Focused pane title if available.
    pub focused_title: Option<String>,
    /// Current verified front-state for the focused pane.
    pub focused_front_state: PaneFrontState,
    /// Whether the focused pane looks like a shell, multiplexer, or TUI.
    pub focused_pane_mode: PaneMode,
    /// Whether shell integration has been observed for the focused pane.
    pub focused_has_shell_integration: bool,
    /// Whether shell-derived metadata such as cwd and last_command should be treated as fresh.
    pub focused_shell_metadata_fresh: bool,
    /// What the backend can actually prove for the focused pane.
    pub focused_observation_support: PaneObservationSupport,
    /// Structured runtime scopes derived from the focused pane's current evidence.
    pub focused_runtime_stack: Vec<PaneRuntimeScope>,
    /// Last verified shell-frame stack for the focused pane.
    pub focused_last_verified_runtime_stack: Vec<PaneRuntimeScope>,
    /// Warnings that should constrain interpretation of the focused pane.
    pub focused_runtime_warnings: Vec<String>,
    /// Typed control contract for the focused pane.
    pub focused_control: PaneControlState,
    /// Last typed shell-context snapshot for the focused pane, when available.
    pub focused_shell_context: Option<PaneShellContext>,
    /// Whether the typed shell-context snapshot still matches the current shell frame.
    pub focused_shell_context_fresh: bool,
    /// Recent con-originated actions on the focused pane.
    pub focused_recent_actions: Vec<PaneActionRecord>,
    /// Best current remote workspace anchor for the focused pane.
    pub focused_remote_workspace: Option<RemoteWorkspaceAnchor>,
    /// Current working directory (from OSC 7 or manual detection)
    pub cwd: Option<String>,
    /// Last N lines of terminal output
    pub recent_output: Vec<String>,
    /// Weak observation hints derived from the current visible screen snapshot.
    pub focused_screen_hints: Vec<PaneObservationHint>,
    /// Structured tmux target inventory from a proven same-session tmux control anchor.
    pub focused_tmux_snapshot: Option<TmuxSnapshot>,
    /// Most recent command text when the backend can prove it.
    pub last_command: Option<String>,
    /// Last exit code
    pub last_exit_code: Option<i32>,
    /// Last command duration in seconds (from ghostty COMMAND_FINISHED)
    pub last_command_duration_secs: Option<f64>,
    /// Git branch if in a repo
    pub git_branch: Option<String>,
    /// Effective remote host for the focused pane, if detected.
    pub ssh_host: Option<String>,
    /// tmux session name, if inside tmux
    pub tmux_session: Option<String>,
    /// Contents of AGENTS.md in the cwd (if present)
    pub agents_md: Option<String>,
    /// Available skills: (name, description) pairs
    pub skills: Vec<(String, String)>,
    /// Recent command blocks from OSC 133 shell integration
    pub command_history: Vec<CommandBlockInfo>,
    /// Other (non-focused) panes in the current tab.
    /// Empty when there is only one pane.
    pub other_panes: Vec<PaneSummary>,
    /// Git diff output (from `git diff --stat` + `git diff`, truncated)
    pub git_diff: Option<String>,
    /// Project file structure (truncated directory listing)
    pub project_structure: Option<String>,
}

/// A completed command block from OSC 133 shell integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandBlockInfo {
    pub command: String,
    pub exit_code: Option<i32>,
}

/// Summary of a non-focused terminal pane's state.
/// Kept intentionally small to avoid bloating the system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneSummary {
    pub pane_index: usize,
    pub pane_id: usize,
    /// Effective remote hostname when detected.
    pub hostname: Option<String>,
    pub hostname_confidence: Option<PaneConfidence>,
    pub hostname_source: Option<PaneEvidenceSource>,
    pub remote_workspace: Option<RemoteWorkspaceAnchor>,
    pub title: Option<String>,
    pub front_state: PaneFrontState,
    pub mode: PaneMode,
    pub has_shell_integration: bool,
    pub shell_metadata_fresh: bool,
    pub observation_support: PaneObservationSupport,
    pub control: PaneControlState,
    pub agent_cli: Option<String>,
    pub active_scope: Option<PaneRuntimeScope>,
    pub evidence: Vec<PaneEvidence>,
    pub runtime_stack: Vec<PaneRuntimeScope>,
    pub last_verified_runtime_stack: Vec<PaneRuntimeScope>,
    pub runtime_warnings: Vec<String>,
    pub tmux_session: Option<String>,
    pub cwd: Option<String>,
    pub workspace_cwd_hint: Option<String>,
    pub workspace_agent_cli_hint: Option<String>,
    pub screen_hints: Vec<PaneObservationHint>,
    pub last_command: Option<String>,
    pub last_exit_code: Option<i32>,
    pub is_busy: bool,
    /// Last ~10 lines of visible output
    pub recent_output: Vec<String>,
}

fn format_runtime_stack(scopes: &[PaneRuntimeScope]) -> String {
    if scopes.is_empty() {
        "unknown".to_string()
    } else {
        scopes
            .iter()
            .map(PaneRuntimeScope::summary)
            .collect::<Vec<_>>()
            .join(" > ")
    }
}

pub fn ssh_target_from_recent_actions(actions: &[PaneActionRecord]) -> Option<String> {
    actions
        .iter()
        .rev()
        .filter_map(|action| action.command.as_deref())
        .find_map(parse_ssh_target)
}

fn parse_workspace_cwd_from_command(command: &str) -> Option<String> {
    let mut search_end = command.len();
    while let Some(idx) = command[..search_end].rfind("cd ") {
        let after_cd = &command[idx + 3..];
        let mut cwd = String::new();
        for ch in after_cd.chars() {
            if matches!(ch, '&' | ';' | '\n' | '\r') {
                break;
            }
            cwd.push(ch);
        }
        let cwd = cwd.trim().trim_matches(&['"', '\''][..]);
        if !cwd.is_empty() {
            return Some(cwd.to_string());
        }
        search_end = idx;
    }
    None
}

pub fn workspace_cwd_hint(
    cwd: Option<&str>,
    recent_actions: &[PaneActionRecord],
) -> Option<String> {
    recent_actions
        .iter()
        .rev()
        .filter_map(|action| action.command.as_deref())
        .find_map(parse_workspace_cwd_from_command)
        .or_else(|| cwd.map(ToString::to_string))
}

pub fn workspace_agent_cli_hint(
    visible_agent_cli: Option<&str>,
    recent_actions: &[PaneActionRecord],
) -> Option<String> {
    visible_agent_cli
        .and_then(canonical_agent_cli_name)
        .map(ToString::to_string)
        .or_else(|| classify_recent_agent_cli_action(recent_actions).map(ToString::to_string))
}

pub fn remote_workspace_anchor(
    runtime: &PaneRuntimeState,
    observation: &PaneObservationFrame,
) -> Option<RemoteWorkspaceAnchor> {
    if let (Some(host), Some(confidence), Some(source)) = (
        runtime.remote_host.clone(),
        runtime.remote_host_confidence,
        runtime.remote_host_source,
    ) {
        return Some(RemoteWorkspaceAnchor {
            host,
            source,
            confidence,
            note: "Remote host is directly anchored by pane-local runtime evidence.".to_string(),
        });
    }

    let host = ssh_target_from_recent_actions(&runtime.recent_actions)?;
    let prompt_like = observation
        .screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput);
    let has_tmux = runtime.tmux_session.is_some()
        || has_tmux_scope(&runtime.scope_stack)
        || has_tmux_scope(&runtime.last_verified_scope_stack);
    let has_interactive_front = matches!(runtime.front_state, PaneFrontState::InteractiveSurface)
        || runtime.mode == PaneMode::Tui
        || runtime.agent_cli.is_some();

    if observation.is_busy || has_tmux || has_interactive_front || !prompt_like {
        return None;
    }

    Some(RemoteWorkspaceAnchor {
        host,
        source: PaneEvidenceSource::ActionHistory,
        confidence: PaneConfidence::Advisory,
        note: "con created or used this pane for SSH recently, and the current screen still looks prompt-like without contradictory tmux/TUI evidence.".to_string(),
    })
}

fn title_looks_tmux_like(title: &str) -> bool {
    let lower = title.to_ascii_lowercase();
    if lower.contains("tmux") || lower.contains("tmate") {
        return true;
    }

    let has_window_box = title.chars().any(|ch| matches!(ch, '❐' | '❑' | '❏'));
    let has_window_state = title.chars().any(|ch| matches!(ch, '●' | '○' | '◉' | '*'));
    let has_numbered_window = title
        .split_whitespace()
        .any(|token| token.chars().next().is_some_and(|ch| ch.is_ascii_digit()));

    has_window_box && has_window_state && has_numbered_window
}

fn parse_ssh_target(command: &str) -> Option<String> {
    let mut tokens = command.split_whitespace().peekable();
    while let Some(token) = tokens.next() {
        let token = token.trim_matches(&['"', '\''][..]);
        if token.is_empty() {
            continue;
        }
        if token.contains('=') && !token.starts_with('-') {
            continue;
        }
        let basename = token.rsplit('/').next().unwrap_or(token);
        if basename != "ssh" {
            continue;
        }
        while let Some(arg) = tokens.next() {
            let arg = arg.trim_matches(&['"', '\''][..]);
            if arg.is_empty() {
                continue;
            }
            if arg == "--" {
                return tokens
                    .next()
                    .map(|target| target.trim_matches(&['"', '\''][..]).to_string());
            }
            if arg.starts_with('-') {
                let takes_value = matches!(
                    arg,
                    "-b" | "-c"
                        | "-D"
                        | "-E"
                        | "-e"
                        | "-F"
                        | "-I"
                        | "-i"
                        | "-J"
                        | "-L"
                        | "-l"
                        | "-m"
                        | "-O"
                        | "-o"
                        | "-p"
                        | "-Q"
                        | "-R"
                        | "-S"
                        | "-W"
                        | "-w"
                );
                if takes_value && !arg.contains('=') {
                    let _ = tokens.next();
                }
                continue;
            }
            return Some(arg.to_string());
        }
    }
    None
}

fn format_scope_summary(scope: &PaneRuntimeScope) -> String {
    scope.summary()
}

pub fn derive_screen_hints(title: Option<&str>, lines: &[String]) -> Vec<PaneObservationHint> {
    let mut hints = Vec::new();

    let non_empty: Vec<&str> = lines
        .iter()
        .map(|line| line.trim_end())
        .filter(|line| !line.trim().is_empty())
        .collect();

    if let Some(line) = non_empty
        .iter()
        .rev()
        .take(3)
        .find(|line| is_prompt_like_line(line))
    {
        hints.push(PaneObservationHint {
            kind: PaneObservationHintKind::PromptLikeInput,
            confidence: PaneConfidence::Advisory,
            detail: format!(
                "A prompt-like input line is visible near the bottom of the current screen: `{}`.",
                line.trim()
            ),
        });
    }

    let htop_markers = [
        "Load average:",
        "Tasks:",
        "PID USER",
        "TIME+  Command",
        "Swp[",
        "Mem[",
    ];
    let marker_count = htop_markers
        .iter()
        .filter(|marker| lines.iter().any(|line| line.contains(**marker)))
        .count();
    if marker_count >= 2 {
        hints.push(PaneObservationHint {
            kind: PaneObservationHintKind::HtopLikeScreen,
            confidence: PaneConfidence::Advisory,
            detail: "The current visible screen resembles htop output.".to_string(),
        });
    }

    if lines.iter().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("Last login:") || trimmed == "System information as of"
    }) {
        hints.push(PaneObservationHint {
            kind: PaneObservationHintKind::LoginBannerVisible,
            confidence: PaneConfidence::Advisory,
            detail: "The current visible screen includes a login banner or shell welcome text."
                .to_string(),
        });
    }

    if lines.iter().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("Connection to ") && trimmed.contains(" closed")
    }) {
        hints.push(PaneObservationHint {
            kind: PaneObservationHintKind::SshConnectionClosed,
            confidence: PaneConfidence::Advisory,
            detail: "The current visible screen shows that an SSH connection was closed."
                .to_string(),
        });
    }

    if title.is_some_and(title_looks_tmux_like) {
        hints.push(PaneObservationHint {
            kind: PaneObservationHintKind::TmuxLikeScreen,
            confidence: PaneConfidence::Advisory,
            detail: "The pane title resembles a tmux session or tmux status title. Treat this as an observation, not native tmux proof.".to_string(),
        });
    }

    hints
}

pub fn classify_screen_agent_cli(_title: Option<&str>, lines: &[String]) -> Option<&'static str> {
    let joined = lines.join("\n").to_ascii_lowercase();

    if joined.contains("openai codex") {
        return Some("codex");
    }

    if joined.contains("claude code") {
        return Some("claude");
    }

    let opencode_markers = [
        joined.contains("ask anything"),
        joined.contains("tab agents"),
        joined.contains("ctrl+p commands"),
    ];
    if opencode_markers
        .into_iter()
        .filter(|present| *present)
        .count()
        >= 2
    {
        return Some("opencode");
    }

    None
}

fn is_prompt_like_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.len() > 100 || trimmed.contains("Expected:") {
        return false;
    }
    if trimmed.contains("  ") {
        return false;
    }

    let starts_like_prompt = trimmed
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '$' | '#' | '%' | '>' | ')' | '❯'));
    let ends_like_prompt = trimmed
        .chars()
        .last()
        .is_some_and(|c| matches!(c, '$' | '#' | '%' | '>'));

    starts_like_prompt || ends_like_prompt
}

fn focused_screen_assessment(
    hints: &[PaneObservationHint],
    control: &PaneControlState,
    tmux_snapshot: Option<&TmuxSnapshot>,
) -> Option<String> {
    if hints.is_empty() {
        return None;
    }

    let has_prompt = hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput);
    let has_htop = hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::HtopLikeScreen);
    let has_login_banner = hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::LoginBannerVisible);
    let has_ssh_closed = hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
    let has_tmux_like = hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);

    let summary = match (has_prompt, has_htop) {
        _ if has_ssh_closed => {
            "The current visible screen shows an SSH connection that appears closed. Treat this pane as a disconnected remote workspace until a stronger fact source proves otherwise."
        }
        _ if has_tmux_like && has_prompt => {
            "The current visible screen looks like a tmux workspace sitting at a prompt-like line, but con does not yet have native tmux control or authoritative foreground-process proof."
        }
        _ if has_tmux_like => {
            "The current visible screen or pane title looks tmux-like, but that remains an observation rather than proof of a live tmux control channel."
        }
        _ if has_login_banner && has_prompt => {
            "The current visible screen looks like a logged-in shell with a recent login banner still visible."
        }
        (true, true) => {
            "The current visible screen shows prompt-like input near the bottom while htop-like content is also visible above. Treat this as mixed current-screen evidence, not proof of the true foreground app."
        }
        (true, false) => {
            "The current visible screen appears to be at a prompt-like input state, but backend facts still do not prove that the foreground target is truly a shell."
        }
        (false, true) => {
            "The current visible screen appears to resemble htop output, but that remains a screen observation rather than authoritative foreground-process proof."
        }
        (false, false) => return None,
    };

    let next_step = if tmux_snapshot.is_some() {
        "A stronger tmux-native fact source is already attached for this pane."
    } else if control
        .capabilities
        .contains(&PaneControlCapability::QueryTmux)
    {
        "A stronger next step is available: query tmux targets through the existing tmux control anchor."
    } else if control.allows_shell_probe() {
        "A stronger next step is available: run a shell probe to collect shell-scoped facts."
    } else {
        "No stronger read-only fact source is currently available because a proven fresh shell prompt or tmux control anchor is not established."
    };

    Some(format!("{summary} {next_step}"))
}

fn canonical_agent_cli_name(name: &str) -> Option<&'static str> {
    match name
        .trim()
        .to_ascii_lowercase()
        .replace('_', "-")
        .replace(' ', "-")
        .as_str()
    {
        "codex" => Some("codex"),
        "claude" | "claude-code" | "claudecode" => Some("claude"),
        "opencode" | "open-code" | "open-code-cli" => Some("opencode"),
        _ => None,
    }
}

fn classify_tmux_agent_cli_command(command: &str) -> Option<&'static str> {
    let basename = command.rsplit('/').next().unwrap_or(command);
    canonical_agent_cli_name(basename)
}

fn classify_recent_agent_cli_action(actions: &[PaneActionRecord]) -> Option<&'static str> {
    actions.iter().find_map(|action| {
        let command = action.command.as_deref()?;
        command
            .split_whitespace()
            .find_map(classify_tmux_agent_cli_command)
    })
}

fn has_tmux_scope(scopes: &[PaneRuntimeScope]) -> bool {
    scopes
        .iter()
        .any(|scope| scope.kind == PaneScopeKind::Multiplexer)
}

fn pane_screen_assessment_label(
    front_state: PaneFrontState,
    hints: &[PaneObservationHint],
) -> &'static str {
    match front_state {
        PaneFrontState::ShellPrompt => "visible shell prompt proven",
        PaneFrontState::InteractiveSurface => "proven interactive surface in front",
        PaneFrontState::Unknown => {
            let prompt_like = hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput);
            let htop_like = hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::HtopLikeScreen);
            let ssh_closed = hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
            let login_banner = hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::LoginBannerVisible);
            let tmux_like = hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
            match (prompt_like, htop_like) {
                _ if ssh_closed => "screen shows an SSH session that appears closed",
                _ if tmux_like && prompt_like => "screen looks like tmux at a prompt",
                _ if tmux_like => "screen looks tmux-like",
                _ if login_banner && prompt_like => "screen looks like a logged-in remote shell",
                (true, true) => {
                    "mixed current screen: prompt-like bottom with htop-like content above"
                }
                (true, false) => "screen currently looks prompt-like",
                (false, true) => "screen currently looks htop-like",
                (false, false) => "foreground target unproven",
            }
        }
    }
}

fn derive_workspace_kind(
    host: Option<&str>,
    tmux_session: Option<&str>,
    runtime_stack: &[PaneRuntimeScope],
    last_verified_runtime_stack: &[PaneRuntimeScope],
    screen_hints: &[PaneObservationHint],
) -> TabWorkspaceKind {
    if tmux_session.is_some()
        || has_tmux_scope(runtime_stack)
        || has_tmux_scope(last_verified_runtime_stack)
        || screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen)
    {
        TabWorkspaceKind::TmuxWorkspace
    } else if host.is_some() {
        TabWorkspaceKind::RemoteShell
    } else if runtime_stack
        .iter()
        .any(|scope| scope.kind == PaneScopeKind::Shell)
        || last_verified_runtime_stack
            .iter()
            .any(|scope| scope.kind == PaneScopeKind::Shell)
    {
        TabWorkspaceKind::LocalShell
    } else {
        TabWorkspaceKind::Unknown
    }
}

fn derive_workspace_state(
    kind: TabWorkspaceKind,
    front_state: PaneFrontState,
    screen_hints: &[PaneObservationHint],
    has_visible_shell_exec: bool,
    has_tmux_native: bool,
) -> TabWorkspaceState {
    if screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed)
    {
        return TabWorkspaceState::Disconnected;
    }
    if matches!(front_state, PaneFrontState::InteractiveSurface) {
        return TabWorkspaceState::Interactive;
    }
    let prompt_like = screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput);

    match kind {
        TabWorkspaceKind::TmuxWorkspace => {
            if has_tmux_native {
                TabWorkspaceState::Ready
            } else {
                TabWorkspaceState::NeedsInspection
            }
        }
        TabWorkspaceKind::RemoteShell | TabWorkspaceKind::LocalShell => {
            if has_visible_shell_exec || prompt_like {
                TabWorkspaceState::Ready
            } else {
                TabWorkspaceState::NeedsInspection
            }
        }
        TabWorkspaceKind::Unknown => TabWorkspaceState::Unknown,
    }
}

fn workspace_note(
    kind: TabWorkspaceKind,
    state: TabWorkspaceState,
    host: Option<&str>,
    tmux_session: Option<&str>,
    cwd: Option<&str>,
    agent_cli: Option<&str>,
) -> String {
    match (kind, state, host, tmux_session, cwd, agent_cli) {
        (_, TabWorkspaceState::Disconnected, Some(host), _, _, _) => {
            format!("SSH workspace for `{host}` appears disconnected.")
        }
        (TabWorkspaceKind::TmuxWorkspace, TabWorkspaceState::Ready, Some(host), None, _, _) => {
            format!("Remote tmux workspace on `{host}` looks ready.")
        }
        (
            TabWorkspaceKind::TmuxWorkspace,
            TabWorkspaceState::Ready,
            Some(host),
            Some(session),
            _,
            _,
        ) => {
            format!("Remote tmux workspace on `{host}` session `{session}` is ready.")
        }
        (TabWorkspaceKind::TmuxWorkspace, _, Some(host), None, _, _) => {
            format!(
                "Remote tmux workspace on `{host}` is visible, but needs inspection before control."
            )
        }
        (TabWorkspaceKind::TmuxWorkspace, _, Some(host), Some(session), _, _) => {
            format!(
                "Remote tmux workspace on `{host}` session `{session}` exists, but needs inspection before control."
            )
        }
        (TabWorkspaceKind::RemoteShell, TabWorkspaceState::Ready, Some(host), _, _, _) => {
            format!("Remote shell workspace on `{host}` looks ready.")
        }
        (TabWorkspaceKind::RemoteShell, _, Some(host), _, _, _) => {
            format!("Remote shell workspace on `{host}` exists, but needs inspection.")
        }
        (TabWorkspaceKind::LocalShell, TabWorkspaceState::Ready, _, _, Some(cwd), Some(agent)) => {
            format!("Local {agent} workspace at `{cwd}` looks reusable.")
        }
        (TabWorkspaceKind::LocalShell, TabWorkspaceState::Ready, _, _, Some(cwd), None) => {
            format!("Local shell workspace at `{cwd}` looks ready.")
        }
        (TabWorkspaceKind::LocalShell, TabWorkspaceState::Ready, _, _, None, Some(agent)) => {
            format!("Local {agent} workspace looks reusable.")
        }
        (TabWorkspaceKind::LocalShell, _, _, _, Some(cwd), Some(agent)) => {
            format!("Local {agent} workspace at `{cwd}` exists, but needs inspection.")
        }
        (TabWorkspaceKind::LocalShell, _, _, _, Some(cwd), None) => {
            format!("Local shell workspace at `{cwd}` exists, but needs inspection.")
        }
        (TabWorkspaceKind::LocalShell, _, _, _, None, Some(agent)) => {
            format!("Local {agent} workspace exists, but needs inspection.")
        }
        (TabWorkspaceKind::TmuxWorkspace, TabWorkspaceState::Ready, None, Some(session), _, _) => {
            format!("tmux workspace `{session}` is ready.")
        }
        (TabWorkspaceKind::LocalShell, _, _, _, None, None) => {
            "Local shell workspace exists, but needs inspection.".to_string()
        }
        _ => "Workspace state is not yet proven.".to_string(),
    }
}

fn summarize_focused_pane_layout(ctx: &TerminalContext) -> String {
    let mut parts = vec![format!(
        "pane {} (id {}) is focused",
        ctx.focused_pane_index, ctx.focused_pane_id
    )];
    if let Some(host) = &ctx.focused_hostname {
        parts.push(format!("host `{host}` is proven"));
    } else if let Some(anchor) = &ctx.focused_remote_workspace {
        parts.push(format!(
            "remote SSH workspace anchored to `{}` via {}",
            anchor.host,
            anchor.source.as_str()
        ));
    }

    if let Some(tmux) = &ctx.focused_control.tmux {
        let session = tmux
            .session_name
            .clone()
            .unwrap_or_else(|| "tmux".to_string());
        match tmux.mode {
            TmuxControlMode::Native => {
                parts.push(format!(
                    "inside tmux session `{session}` with native control"
                ));
            }
            TmuxControlMode::InspectOnly => {
                parts.push(format!("tmux layer present via session `{session}`, but native control is not established"));
            }
            TmuxControlMode::Unavailable => {
                parts.push(format!(
                    "tmux session `{session}` is known historically, but no current tmux control channel is available"
                ));
            }
        }
    } else if has_tmux_scope(&ctx.focused_runtime_stack)
        || has_tmux_scope(&ctx.focused_last_verified_runtime_stack)
    {
        parts.push(
            "tmux appears in the runtime stack, but no tmux control anchor is established"
                .to_string(),
        );
    } else if ctx
        .focused_screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen)
    {
        parts.push("screen looks tmux-like, but control is not established".to_string());
    }

    parts.push(
        pane_screen_assessment_label(ctx.focused_front_state, &ctx.focused_screen_hints)
            .to_string(),
    );

    if let Some(workspace) = tab_workspaces(ctx)
        .into_iter()
        .find(|workspace| workspace.pane_id == ctx.focused_pane_id)
    {
        parts.push(format!(
            "workspace={} state={}",
            workspace.kind.as_str(),
            workspace.state.as_str()
        ));
    }

    if !ctx.focused_shell_metadata_fresh && !ctx.focused_last_verified_runtime_stack.is_empty() {
        parts.push(format!(
            "historical shell frame only: {}",
            format_runtime_stack(&ctx.focused_last_verified_runtime_stack)
        ));
    }

    parts.join("; ")
}

fn summarize_peer_pane_layout(pane: &PaneSummary) -> String {
    let mut parts = vec![format!("pane {} (id {})", pane.pane_index, pane.pane_id)];
    if let Some(host) = &pane.hostname {
        parts.push(format!("host `{host}` is proven"));
    } else if let Some(anchor) = &pane.remote_workspace {
        parts.push(format!(
            "remote SSH workspace anchored to `{}` via {}",
            anchor.host,
            anchor.source.as_str()
        ));
    }

    if let Some(tmux) = &pane.control.tmux {
        let session = tmux
            .session_name
            .clone()
            .unwrap_or_else(|| "tmux".to_string());
        match tmux.mode {
            TmuxControlMode::Native => {
                parts.push(format!(
                    "inside tmux session `{session}` with native control"
                ));
            }
            TmuxControlMode::InspectOnly => {
                parts.push(format!("tmux layer present via session `{session}`, but native control is not established"));
            }
            TmuxControlMode::Unavailable => {
                parts.push(format!(
                    "tmux session `{session}` is known historically, but no current tmux control channel is available"
                ));
            }
        }
    } else if has_tmux_scope(&pane.runtime_stack)
        || has_tmux_scope(&pane.last_verified_runtime_stack)
    {
        parts.push(
            "tmux appears in the runtime stack, but no tmux control anchor is established"
                .to_string(),
        );
    } else if pane
        .screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen)
    {
        parts.push("screen looks tmux-like, but control is not established".to_string());
    }

    parts.push(pane_screen_assessment_label(pane.front_state, &pane.screen_hints).to_string());

    let host = pane.hostname.as_deref().or_else(|| {
        pane.remote_workspace
            .as_ref()
            .map(|anchor| anchor.host.as_str())
    });
    let kind = derive_workspace_kind(
        host,
        pane.control
            .tmux
            .as_ref()
            .and_then(|tmux| tmux.session_name.as_deref())
            .or(pane.tmux_session.as_deref()),
        &pane.runtime_stack,
        &pane.last_verified_runtime_stack,
        &pane.screen_hints,
    );
    let state = derive_workspace_state(
        kind,
        pane.front_state,
        &pane.screen_hints,
        pane_supports_visible_shell(pane) || pane.remote_workspace.is_some(),
        pane_has_native_tmux(pane),
    );
    parts.push(format!(
        "workspace={} state={}",
        kind.as_str(),
        state.as_str()
    ));

    if !pane.shell_metadata_fresh && !pane.last_verified_runtime_stack.is_empty() {
        parts.push(format!(
            "historical shell frame only: {}",
            format_runtime_stack(&pane.last_verified_runtime_stack)
        ));
    }

    parts.join("; ")
}

fn focused_supports_visible_shell(ctx: &TerminalContext) -> bool {
    ctx.focused_control
        .capabilities
        .contains(&PaneControlCapability::ExecVisibleShell)
}

fn pane_supports_visible_shell(pane: &PaneSummary) -> bool {
    pane.control
        .capabilities
        .contains(&PaneControlCapability::ExecVisibleShell)
}

fn focused_has_native_tmux(ctx: &TerminalContext) -> bool {
    ctx.focused_control
        .tmux
        .as_ref()
        .is_some_and(|tmux| tmux.mode == TmuxControlMode::Native)
}

fn pane_has_native_tmux(pane: &PaneSummary) -> bool {
    pane.control
        .tmux
        .as_ref()
        .is_some_and(|tmux| tmux.mode == TmuxControlMode::Native)
}

fn tab_workspaces(ctx: &TerminalContext) -> Vec<TabWorkspaceSummary> {
    let focused_workspace_cwd = workspace_cwd_hint(ctx.cwd.as_deref(), &ctx.focused_recent_actions);
    let focused_workspace_agent = workspace_agent_cli_hint(
        ctx.focused_control.visible_target.label.as_deref(),
        &ctx.focused_recent_actions,
    );
    let focused_host = ctx.focused_hostname.as_deref().or_else(|| {
        ctx.focused_remote_workspace
            .as_ref()
            .map(|anchor| anchor.host.as_str())
    });
    let focused_kind = derive_workspace_kind(
        focused_host,
        ctx.focused_control
            .tmux
            .as_ref()
            .and_then(|tmux| tmux.session_name.as_deref())
            .or(ctx.tmux_session.as_deref()),
        &ctx.focused_runtime_stack,
        &ctx.focused_last_verified_runtime_stack,
        &ctx.focused_screen_hints,
    );
    let focused_state = derive_workspace_state(
        focused_kind,
        ctx.focused_front_state,
        &ctx.focused_screen_hints,
        focused_supports_visible_shell(ctx) || ctx.focused_remote_workspace.is_some(),
        focused_has_native_tmux(ctx),
    );
    let mut workspaces = vec![TabWorkspaceSummary {
        pane_index: ctx.focused_pane_index,
        pane_id: ctx.focused_pane_id,
        host: focused_host.map(ToString::to_string),
        tmux_session: ctx
            .focused_control
            .tmux
            .as_ref()
            .and_then(|tmux| tmux.session_name.clone())
            .or_else(|| ctx.tmux_session.clone()),
        cwd: focused_workspace_cwd,
        agent_cli: focused_workspace_agent,
        kind: focused_kind,
        state: focused_state,
        note: String::new(),
    }];
    for pane in &ctx.other_panes {
        let host = pane.hostname.as_deref().or_else(|| {
            pane.remote_workspace
                .as_ref()
                .map(|anchor| anchor.host.as_str())
        });
        let kind = derive_workspace_kind(
            host,
            pane.control
                .tmux
                .as_ref()
                .and_then(|tmux| tmux.session_name.as_deref())
                .or(pane.tmux_session.as_deref()),
            &pane.runtime_stack,
            &pane.last_verified_runtime_stack,
            &pane.screen_hints,
        );
        let state = derive_workspace_state(
            kind,
            pane.front_state,
            &pane.screen_hints,
            pane_supports_visible_shell(pane) || pane.remote_workspace.is_some(),
            pane_has_native_tmux(pane),
        );
        workspaces.push(TabWorkspaceSummary {
            pane_index: pane.pane_index,
            pane_id: pane.pane_id,
            host: host.map(ToString::to_string),
            tmux_session: pane
                .control
                .tmux
                .as_ref()
                .and_then(|tmux| tmux.session_name.clone())
                .or_else(|| pane.tmux_session.clone()),
            cwd: pane.workspace_cwd_hint.clone(),
            agent_cli: pane.workspace_agent_cli_hint.clone(),
            kind,
            state,
            note: String::new(),
        });
    }

    for workspace in &mut workspaces {
        workspace.note = workspace_note(
            workspace.kind,
            workspace.state,
            workspace.host.as_deref(),
            workspace.tmux_session.as_deref(),
            workspace.cwd.as_deref(),
            workspace.agent_cli.as_deref(),
        );
    }

    workspaces
}

fn preferred_work_target_hints(ctx: &TerminalContext) -> Vec<String> {
    let mut hints = Vec::new();

    let mut best_remote_shell: Option<(usize, Option<&str>, bool)> = None;
    let focused_looks_tmux = ctx
        .focused_screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
    let focused_disconnected = ctx
        .focused_screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
    if !focused_looks_tmux
        && !focused_disconnected
        && (focused_supports_visible_shell(ctx) || ctx.focused_remote_workspace.is_some())
    {
        best_remote_shell = Some((
            ctx.focused_pane_index,
            ctx.focused_hostname.as_deref().or_else(|| {
                ctx.focused_remote_workspace
                    .as_ref()
                    .map(|anchor| anchor.host.as_str())
            }),
            ctx.focused_hostname.is_some(),
        ));
    }
    for pane in &ctx.other_panes {
        let tmux_like = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
        let disconnected = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
        if tmux_like || disconnected {
            continue;
        }
        if !pane_supports_visible_shell(pane) && pane.remote_workspace.is_none() {
            continue;
        }
        let candidate = (
            pane.pane_index,
            pane.hostname.as_deref().or_else(|| {
                pane.remote_workspace
                    .as_ref()
                    .map(|anchor| anchor.host.as_str())
            }),
            pane.hostname.is_some(),
        );
        match best_remote_shell {
            None => best_remote_shell = Some(candidate),
            Some((current_index, _current_host, current_remote)) => {
                if candidate.2 && !current_remote
                    || (candidate.2 == current_remote && candidate.0 < current_index)
                {
                    best_remote_shell = Some(candidate);
                }
            }
        }
    }
    if let Some((pane_index, host, _)) = best_remote_shell {
        hints.push(match host {
            Some(host) => format!(
                "Best visible remote shell target right now: pane {pane_index} on host `{host}`. Re-run list_panes if the layout changes because pane_index is positional."
            ),
            None => format!(
                "Best visible shell target right now: pane {pane_index}. Remote identity is not proven."
            ),
        });
    }

    let focused_prompt_like = ctx
        .focused_screen_hints
        .iter()
        .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput);
    let focused_local_shell = !focused_looks_tmux
        && !focused_disconnected
        && ctx.focused_hostname.is_none()
        && ctx.focused_remote_workspace.is_none()
        && (focused_supports_visible_shell(ctx) || focused_prompt_like);
    let mut best_local_shell: Option<(usize, Option<&str>, bool)> = None;
    if focused_local_shell {
        best_local_shell = Some((
            ctx.focused_pane_index,
            ctx.cwd.as_deref(),
            focused_supports_visible_shell(ctx),
        ));
    }
    for pane in &ctx.other_panes {
        let tmux_like = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
        let disconnected = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
        let prompt_like = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput);
        if tmux_like
            || disconnected
            || pane.hostname.is_some()
            || pane.remote_workspace.is_some()
            || (!pane_supports_visible_shell(pane) && !prompt_like)
        {
            continue;
        }
        let candidate = (
            pane.pane_index,
            pane.cwd.as_deref(),
            pane_supports_visible_shell(pane),
        );
        match best_local_shell {
            None => best_local_shell = Some(candidate),
            Some((current_index, _current_cwd, current_proven)) => {
                if candidate.2 && !current_proven
                    || (candidate.2 == current_proven && candidate.0 < current_index)
                {
                    best_local_shell = Some(candidate);
                }
            }
        }
    }
    if let Some((pane_index, cwd, proven)) = best_local_shell {
        hints.push(match (cwd, proven) {
            (Some(cwd), true) => format!(
                "Best local shell target right now: pane {pane_index} at `{cwd}`. Prefer ensure_local_shell_target or terminal_exec there before creating another shell pane."
            ),
            (Some(cwd), false) => format!(
                "A likely reusable local shell target is pane {pane_index} at `{cwd}`, but it is backed by continuity and prompt-like screen state rather than a fresh proven shell prompt."
            ),
            (None, true) => format!(
                "Best local shell target right now: pane {pane_index}. Prefer ensure_local_shell_target or terminal_exec there before creating another shell pane."
            ),
            (None, false) => format!(
                "A likely reusable local shell target is pane {pane_index}, but it is backed by continuity and prompt-like screen state rather than a fresh proven shell prompt."
            ),
        });
    }

    let mut remote_workspaces = Vec::new();
    if !focused_looks_tmux && !focused_disconnected {
        if let Some(anchor) = &ctx.focused_remote_workspace {
            remote_workspaces.push((ctx.focused_pane_index, anchor));
        }
    }
    for pane in &ctx.other_panes {
        let tmux_like = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
        let disconnected = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
        if tmux_like || disconnected {
            continue;
        }
        if let Some(anchor) = &pane.remote_workspace {
            remote_workspaces.push((pane.pane_index, anchor));
        }
    }
    remote_workspaces.sort_by(|(pane_a, anchor_a), (pane_b, anchor_b)| {
        anchor_a
            .host
            .cmp(&anchor_b.host)
            .then_with(|| pane_a.cmp(pane_b))
    });
    remote_workspaces.dedup_by(|(_, a), (_, b)| a.host == b.host);
    if !remote_workspaces.is_empty() {
        hints.push(format!(
            "Known remote SSH workspaces in this tab: {}.",
            remote_workspaces
                .iter()
                .map(|(pane_index, anchor)| format!(
                    "`{}` on pane {} via {}",
                    anchor.host,
                    pane_index,
                    anchor.source.as_str()
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let focused_local_agent = !focused_looks_tmux
        && !focused_disconnected
        && ctx.focused_hostname.is_none()
        && ctx.focused_remote_workspace.is_none()
        && (ctx.focused_control.visible_target.kind == PaneVisibleTargetKind::AgentCli
            || classify_recent_agent_cli_action(&ctx.focused_recent_actions).is_some());
    let mut best_local_agent: Option<(usize, &'static str)> = None;
    if focused_local_agent {
        if let Some(agent) = ctx
            .focused_control
            .visible_target
            .label
            .as_deref()
            .and_then(canonical_agent_cli_name)
            .or_else(|| classify_recent_agent_cli_action(&ctx.focused_recent_actions))
        {
            best_local_agent = Some((ctx.focused_pane_index, agent));
        }
    }
    for pane in &ctx.other_panes {
        let tmux_like = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
        let disconnected = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
        if tmux_like || disconnected || pane.hostname.is_some() || pane.remote_workspace.is_some() {
            continue;
        }
        let agent = if pane.control.visible_target.kind == PaneVisibleTargetKind::AgentCli {
            pane.control
                .visible_target
                .label
                .as_deref()
                .and_then(canonical_agent_cli_name)
                .or_else(|| pane.agent_cli.as_deref().and_then(canonical_agent_cli_name))
        } else {
            pane.agent_cli.as_deref().and_then(canonical_agent_cli_name)
        };
        if let Some(agent) = agent {
            match best_local_agent {
                None => best_local_agent = Some((pane.pane_index, agent)),
                Some((current_index, _)) if pane.pane_index < current_index => {
                    best_local_agent = Some((pane.pane_index, agent))
                }
                _ => {}
            }
        }
    }
    if let Some((pane_index, agent)) = best_local_agent {
        hints.push(format!(
            "Best local {agent} target right now: pane {pane_index}. Prefer ensure_local_agent_target or reuse this pane before launching another local agent CLI."
        ));
    }

    let mut best_tmux: Option<(usize, Option<&str>, bool)> = None;
    if ctx.focused_control.tmux.is_some() || focused_looks_tmux {
        best_tmux = Some((
            ctx.focused_pane_index,
            ctx.focused_control
                .tmux
                .as_ref()
                .and_then(|tmux| tmux.session_name.as_deref()),
            focused_has_native_tmux(ctx),
        ));
    }
    for pane in &ctx.other_panes {
        let looks_tmux = pane
            .screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
        if pane.control.tmux.is_none() && !looks_tmux {
            continue;
        }
        let candidate = (
            pane.pane_index,
            pane.control
                .tmux
                .as_ref()
                .and_then(|tmux| tmux.session_name.as_deref()),
            pane_has_native_tmux(pane),
        );
        match best_tmux {
            None => best_tmux = Some(candidate),
            Some((current_index, _, current_native)) => {
                if candidate.2 && !current_native
                    || (candidate.2 == current_native && candidate.0 < current_index)
                {
                    best_tmux = Some(candidate);
                }
            }
        }
    }
    if let Some((pane_index, session, native)) = best_tmux {
        hints.push(match (session, native) {
            (Some(session), true) => format!(
                "Best tmux workspace target right now: pane {pane_index}, tmux session `{session}`, with native tmux control."
            ),
            (Some(session), false) => format!(
                "tmux appears in pane {pane_index} via session `{session}`, but native tmux control is not established there yet."
            ),
            (None, true) => format!(
                "Best tmux workspace target right now: pane {pane_index} with native tmux control."
            ),
            (None, false) => format!(
                "tmux appears in pane {pane_index}, but native tmux control is not established there yet."
            ),
        });
    }

    if let Some(snapshot) = &ctx.focused_tmux_snapshot {
        let mut agent_targets = snapshot
            .panes
            .iter()
            .filter_map(|pane| {
                let agent = pane
                    .pane_current_command
                    .as_deref()
                    .and_then(classify_tmux_agent_cli_command)?;
                Some((agent, pane))
            })
            .collect::<Vec<_>>();
        agent_targets.sort_by(|(agent_a, pane_a), (agent_b, pane_b)| {
            pane_b
                .pane_active
                .cmp(&pane_a.pane_active)
                .then_with(|| pane_b.window_active.cmp(&pane_a.window_active))
                .then_with(|| agent_a.cmp(agent_b))
                .then_with(|| pane_a.window_index.cmp(&pane_b.window_index))
                .then_with(|| pane_a.pane_index.cmp(&pane_b.pane_index))
        });

        if let Some((agent, pane)) = agent_targets.first() {
            hints.push(format!(
                "Focused tmux workspace already has an `{agent}` target at `{}` (window `{}`, pane `{}`). Prefer tmux_capture_pane or tmux_send_keys there before creating another agent target.",
                pane.target, pane.window_name, pane.pane_index
            ));
        } else if focused_has_native_tmux(ctx) {
            hints.push(
                "Focused tmux workspace has native tmux control but no known Codex, Claude Code, or OpenCode target yet. Use tmux_ensure_agent_target when you need one."
                    .to_string(),
            );
        }
    } else if focused_has_native_tmux(ctx) {
        hints.push(
            "Focused pane has native tmux control. Use tmux target helpers instead of raw outer-pane input for shell or agent work inside tmux."
                .to_string(),
        );
    }

    hints
}

fn format_control_channels(channels: &[PaneControlChannel]) -> String {
    channels
        .iter()
        .map(|channel| channel.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn format_control_capabilities(capabilities: &[PaneControlCapability]) -> String {
    capabilities
        .iter()
        .map(|capability| capability.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

impl TerminalContext {
    pub fn empty() -> Self {
        Self {
            focused_pane_index: 1,
            focused_pane_id: 0,
            focused_hostname: None,
            focused_hostname_confidence: None,
            focused_hostname_source: None,
            focused_title: None,
            focused_front_state: PaneFrontState::Unknown,
            focused_pane_mode: PaneMode::Unknown,
            focused_has_shell_integration: false,
            focused_shell_metadata_fresh: false,
            focused_observation_support: PaneObservationSupport::default(),
            focused_runtime_stack: Vec::new(),
            focused_last_verified_runtime_stack: Vec::new(),
            focused_runtime_warnings: Vec::new(),
            focused_control: PaneControlState {
                address_space: PaneAddressSpace::ConPane,
                target_stack: vec![PaneVisibleTarget {
                    kind: PaneVisibleTargetKind::Unknown,
                    label: None,
                    host: None,
                }],
                visible_target: PaneVisibleTarget {
                    kind: PaneVisibleTargetKind::Unknown,
                    label: None,
                    host: None,
                },
                attachments: Vec::new(),
                tmux: None,
                channels: vec![
                    PaneControlChannel::ReadScreen,
                    PaneControlChannel::SearchScrollback,
                    PaneControlChannel::RawInput,
                ],
                capabilities: vec![
                    PaneControlCapability::ReadScreen,
                    PaneControlCapability::SearchScrollback,
                    PaneControlCapability::SendRawInput,
                ],
                notes: vec![
                    "Pane control state is unavailable; treat the visible target as unknown."
                        .to_string(),
                ],
            },
            focused_shell_context: None,
            focused_shell_context_fresh: false,
            focused_recent_actions: Vec::new(),
            focused_remote_workspace: None,
            cwd: None,
            recent_output: Vec::new(),
            focused_screen_hints: Vec::new(),
            focused_tmux_snapshot: None,
            last_command: None,
            last_exit_code: None,
            last_command_duration_secs: None,
            git_branch: None,
            ssh_host: None,
            tmux_session: None,
            agents_md: None,
            skills: Vec::new(),
            command_history: Vec::new(),
            other_panes: Vec::new(),
            git_diff: None,
            project_structure: None,
        }
    }

    /// Build a system prompt enriched with terminal context.
    ///
    /// Uses XML tags for structured context injection — models parse these
    /// more reliably than plain text blocks, and it prevents context from
    /// being confused with user instructions.
    pub fn to_system_prompt(&self) -> String {
        let mut prompt = String::with_capacity(4096);

        prompt.push_str(
            "You are con, a terminal AI assistant with full access to the user's terminal environment.\n\n\
             ## Decision framework\n\
             You are an orchestrator with direct API access to the terminal.\n\n\
             ### Keystroke categories — know the difference\n\
             1. **con application shortcuts** (Cmd+T, Cmd+W, Cmd+N): These control the con app itself.\n\
                You CANNOT send these. Use tools instead: create_pane for new terminals.\n\
             2. **Terminal protocol sequences** (\\x02c for tmux prefix+c, \\x1b for Escape, \\x03 for Ctrl-C):\n\
                These travel through the PTY to the remote program. send_keys is correct for direct TUI interaction, \
                but when a pane exposes tmux native control you should prefer tmux_list_targets / tmux_capture_pane / tmux_run_command / tmux_send_keys over outer-pane PTY keystrokes.\n\
             3. **Shell commands** (ls, apt update, git status):\n\
                Use terminal_exec when `exec_visible_shell` is available. When it is NOT, first observe the pane \
                and only use send_keys if a shell prompt is visibly present.\n\n\
             ### Choose the right tool\n\
             - SHELL COMMANDS on a pane with `exec_visible_shell` → terminal_exec / batch_exec.\n\
             - READ-ONLY SHELL INTROSPECTION on a pane with `probe_shell_context` → probe_shell_context.\n\
             - CURRENT TERMINAL SITUATION questions (\"where am I?\", \"am I in tmux?\", \"what host is this?\") → use the provided focused-pane context first, including any `shell_context`, `tmux_snapshot`, or `<tab_workspaces>`. Only call list_tab_workspaces / list_panes / probe_shell_context / tmux_list_targets when a stronger fact source is still needed.\n\
             - MULTI-PANE TARGET SELECTION (\"which pane should you use?\", \"which pane is safer?\", \"where should you run this?\") → resolve_work_target.\n\
             - FOLLOW-UP REMOTE WORK on hosts that already exist in `<remote_workspaces>` → reuse those workspaces by default. Do not create duplicate SSH panes unless the user asks for a new host or the existing pane is no longer reusable.\n\
             - DISCONNECTED SSH WORKSPACES → recover only the affected host with ensure_remote_shell_target. Do not recreate healthy remote panes when one host disconnects.\n\
             - REMOTE TMUX WORKSPACE PREPARATION (\"connect to haswell and prepare tmux\", \"ensure tmux session con-bench on host X\") → ensure_remote_tmux_workspace. Prefer this over manually chaining ensure_remote_shell_target + terminal_exec + tmux_list_targets when you only need the tmux session/bootstrap fact.\n\
             - REMOTE TMUX SHELL-WORK PREPARATION (\"connect to haswell, prepare tmux, and give me a clean shell target\", \"operate inside tmux on host X\") → ensure_remote_tmux_shell_target. Prefer this over manually attaching tmux in the visible pane when the user needs a reusable tmux shell target for file work.\n\
             - If ensure_remote_tmux_shell_target succeeds, describe the tmux workspace from its `tmux_snapshot` and `shell_target`. Do NOT attach tmux in the outer visible pane just to orient yourself unless the user explicitly wants to enter the attached tmux UI.\n\
             - FOLLOW-UP LOCAL CODING WORK in `<local_workspaces>` → reuse those local Codex / Claude / OpenCode / shell workspaces by default. Do not create duplicate local panes when a reusable workspace already exists for the same project path.\n\
             - When `<work_target_hints>` is present, use those typed hints before improvising pane choice from raw metadata.\n\
             - For CURRENT TERMINAL SITUATION answers, structure the response as: proven facts, current-screen assessment, and unknowns/limits. Use `screen_hints` and `terminal_output` to describe what appears on screen now without promoting it to backend truth.\n\
             - When you summarize evidence, name the actual evidence source you used. Do not describe a `tmux_shell_turn`, `terminal_exec`, `agent_cli_turn`, or `command -v` result as a `shell probe` unless `probe_shell_context` actually ran.\n\
             - When a command or install check produced decisive output, prefer quoting or closely paraphrasing that concrete output over upgrading it into a broader claim.\n\
             - When multiple panes are visible and the user asks about the terminal/session state, mention the pane count and summarize materially different peer panes. Do not collapse the whole tab into only the focused pane unless the user explicitly asks about that pane alone.\n\
             - Never restate stale shell metadata as if it were the current foreground runtime. If shell metadata is not fresh, label it as historical shell metadata or omit it.\n\
             - TMUX TARGET DISCOVERY on a pane with tmux native control → tmux_find_targets or tmux_list_targets, then tmux_capture_pane.\n\
             - LOCAL CODING CLI workflows (Codex / Claude Code / OpenCode on the local machine) → keep the interactive CLI target separate from shell work. Prefer ensure_local_coding_workspace when you need both sides of the pair. If you only need one side, use ensure_local_agent_target for the interactive CLI pane and ensure_local_shell_target for file edits, test runs, and other shell commands.\n\
             - In a paired LOCAL coding workspace, use the shell lane for deterministic file writes, test runs, git commands, and other direct shell work. Use the interactive agent CLI lane only when the user explicitly wants Codex / Claude / OpenCode to act or when you need a natural-language follow-up inside that CLI.\n\
             - If a local coding request asks you to create/edit files, run tests, or execute shell commands in a project path, do that work in the shell lane by default. Keep the interactive agent CLI target untouched unless the request explicitly asks you to consult it, or unless you must clear a trust/continue prompt that is blocking later work.\n\
             - Do NOT send the same local coding task to both the shell lane and the interactive agent CLI lane in the same turn.\n\
             - When collecting git output through the shell lane, prefer pager-safe forms such as `git --no-pager diff --stat`, `git --no-pager show`, or `GIT_PAGER=cat ...` so the shell returns to a prompt and the result stays verifiable.\n\
             - INTERACTIVE AGENT-CLI FOLLOW-UP (Codex / Claude Code / OpenCode already running in a known pane or tmux target) → agent_cli_turn. Prefer this over raw send_keys or tmux_send_keys so con can wait for the interactive target to settle and return a fresh snapshot before you continue.\n\
             - For interactive agent-CLI requests, \"prompt submitted\" is not completion. Stay with the same target until you have the requested answer, a clear blocking interstitial, or an explicit timeout/no-answer result.\n\
             - TMUX SHELL PREPARATION on a pane with tmux native control → tmux_ensure_shell_target to reuse or create a safe shell pane before remote file work or shell execution inside tmux.\n\
             - TMUX SHELL FOLLOW-UP in an existing tmux shell target → tmux_shell_turn. Prefer this over manually composing tmux_send_keys + tmux_capture_pane when you need deterministic shell work, file edits, test runs, or install checks inside tmux.\n\
             - TMUX AGENT TARGET PREPARATION on a pane with tmux native control → tmux_ensure_agent_target to reuse or create a Codex CLI, Claude Code, or OpenCode tmux pane before interacting with that agent.\n\
             - TMUX NATIVE COMMAND LAUNCH on a pane with tmux native control → tmux_run_command to create a new tmux window or split for a shell, Codex CLI, Claude Code, OpenCode, or a long-running command.\n\
             - TMUX NATIVE INTERACTION on a pane with tmux native control → tmux_send_keys to a specific tmux pane target.\n\
             - TMUX WITHOUT native control → read_pane first, then outer-pane send_keys only as a fallback.\n\
             - PARALLEL WORK across hosts → remote_exec or ensure_remote_shell_target for each host so existing SSH panes are reused across turns. Use create_pane only for low-level manual pane creation.\n\
             - SHELL COMMANDS on a pane WITHOUT `exec_visible_shell` → use read_pane first, then send_keys \"command\\n\" only if a shell prompt is visibly present.\n\
             - LONG-RUNNING commands → launch, then wait_for (not repeated read_pane).\n\
             - INTERACTIVE TUI (htop, menus, agent CLIs without a stronger attachment) → tmux_send_keys when a tmux target exists, otherwise send_keys + read_pane (follow playbooks).\n\
             - LOCAL FILE operations → file_read, file_write, edit_file, search, list_files.\n\
             - REMOTE FILE operations → send_keys in a remote shell (cat, heredoc, editor commands).\n\n\
             ## Turn efficiency\n\
             - read_pane default (50 lines) is usually sufficient. Only increase if needed.\n\
             - Use search_panes instead of reading full scrollback to find specific output.\n\
             - Batch keystrokes: e.g., send_keys \"\\x1bggdG\" is one turn, not three.\n\n\
             ## Tools\n\n\
             <tools>\n\
             - terminal_exec: Run a command visibly in a con pane. Requires `exec_visible_shell` capability.\n\
               `pane_index` addresses a con pane from list_panes — not a tmux pane/window or editor buffer.\n\
               Check exit_code: 0 = success, non-zero = failure, null = no shell integration or still running.\n\n\
             - batch_exec: Run commands on MULTIPLE panes in PARALLEL. Same `exec_visible_shell` requirement.\n\n\
             - shell_exec: Run a command in a hidden LOCAL subprocess. Output not shown to user.\n\
               For local-only tasks: git, file searches, package lookups. Never for remote environments.\n\n\
             - create_pane: Create a new terminal pane (split in current tab). Optionally run a startup command \
               (e.g. \"ssh host\"). The command executes automatically — do NOT re-send it. Supports split placement via `location` (`right` or `down`).\n\
               Returns both `pane_index` and stable `pane_id`, plus initial output (waits for output to settle). \
               Check the output to see what happened — no need for a separate read_pane.\n\n\
             - list_panes: List all panes with metadata, stable pane ids, control state, capabilities, and addressing notes.\n\n\
             - list_tab_workspaces: Summarize the current tab as typed workspaces such as remote shell, tmux workspace, disconnected SSH pane, local shell, or local coding-cli workspace.\n\n\
             - probe_shell_context: Run a read-only shell-scoped probe in a pane with a proven fresh shell prompt. \
               Use this to gather authoritative shell facts such as hostname, SSH env, tmux env, tmux session/window/pane ids, \
               and NVIM_LISTEN_ADDRESS. This is shell-scope truth, not foreground-app truth after control passes to a TUI.\n\n\
             - read_pane: Read last N lines from any pane (includes scrollback).\n\n\
             - send_keys: Send keystrokes to a pane's PTY. Use for:\n\
               (a) direct TUI interaction when there is no stronger native attachment.\n\
               (b) shell commands on panes without exec_visible_shell when a shell prompt is visibly present.\n\
               (c) tmux prefix sequences ONLY when tmux native control is unavailable.\n\
               NEVER use to simulate con application shortcuts (Cmd+T, Cmd+W).\n\n\
             - wait_for: Wait for a pane to become idle or for a pattern to appear. Use after launching \
               long-running commands instead of polling with read_pane. Idle mode (no pattern) works universally — \
               shell integration for precise detection, output quiescence as fallback. On timeout, \
               read_pane to check progress and call wait_for again.\n\n\
             - tmux_inspect: Inspect tmux adapter state for a pane containing a tmux session.\n\
             - tmux_list_targets: List tmux windows/panes through a proven same-session tmux shell anchor.\n\
             - tmux_find_targets: Find likely tmux shell panes, agent CLI panes, or other matching targets without hand-filtering tmux_list_targets.\n\
             - resolve_work_target: Choose the best con pane or tmux target for shell work, tmux work, or agent CLI interaction using the typed control plane. When it returns a con pane, carry its `pane_id` into the next tool call.\n\
             - ensure_local_coding_workspace: Prepare or reuse the preferred LOCAL coding pair for a project path: one interactive Codex / Claude Code / OpenCode pane plus one separate local shell pane.\n\
             - agent_cli_turn: Send one turn into an existing Codex / Claude Code / OpenCode target, wait for that interactive target to settle, and return a fresh snapshot. Use this for follow-up interactive CLI work instead of raw send_keys when the target already exists.\n\
             - ensure_local_agent_target: Reuse an existing LOCAL Codex / Claude Code / OpenCode pane, or create one if needed. Use this when you only need the interactive agent side.\n\
             - ensure_local_shell_target: Reuse an existing LOCAL shell pane, or create one if needed. Use this when you only need the shell companion for local coding workflows so shell work stays out of the interactive agent UI.\n\
             - ensure_remote_shell_target: Reuse an existing SSH pane for a host, or create one if needed. Prefer this over repeatedly creating duplicate SSH panes during multi-host work. Carry the returned `pane_id` into follow-up work.\n\
             - ensure_remote_tmux_shell_target: Reuse or create a remote SSH shell pane for a host, ensure a named tmux session exists there, verify tmux-native control on that pane, and then reuse or create a clean tmux shell target for file work. The result already contains `tmux_snapshot` plus the chosen shell target, so you usually do not need to attach tmux in the outer pane.\n\
             - ensure_remote_tmux_workspace: Reuse or create a remote SSH shell pane for a host, ensure a named tmux session exists there, and report whether tmux-native control is immediately available from that same pane. Use this when you only need tmux bootstrap, not a shell target.\n\
             - remote_exec: Reuse or create remote SSH workspaces for one or more hosts, then run the same command on them in parallel. Its per-host results include stable `pane_id` values for follow-up work.\n\
             - tmux_capture_pane: Capture the content of a specific tmux pane target without confusing it with the outer con pane.\n\
             - tmux_ensure_shell_target: Reuse or create a tmux shell target through a proven same-session tmux shell anchor.\n\
             - tmux_shell_turn: Run one deterministic shell command inside an existing tmux shell target, wait for that shell target to settle, and return a fresh capture. Use this for tmux shell-lane work instead of manually pairing tmux_send_keys with tmux_capture_pane.\n\
             - tmux_ensure_agent_target: Reuse or create a tmux target for Codex CLI, Claude Code, or OpenCode. This stays in tmux control; it does not imply a native Codex/OpenCode attachment unless con explicitly proves one.\n\
             - tmux_run_command: Create a new tmux window or split pane and run a command there through a proven same-session tmux shell anchor.\n\
             - tmux_send_keys: Send text or tmux key names to a specific tmux pane target through a proven same-session tmux shell anchor.\n\
             - search_panes: Search scrollback across panes by regex.\n\n\
             - file_read, file_write, edit_file: LOCAL filesystem only. Cannot access remote SSH hosts.\n\
             - list_files: List LOCAL directory. Respects .gitignore. Max 500 entries.\n\
             - search: Search LOCAL file contents by regex.\n\
             </tools>\n\n\
             <safety>\n\
             - NEVER execute rm -rf, DROP TABLE, or destructive commands without explicit user confirmation.\n\
             - Check is_alive before executing — false means PTY exited, commands will fail.\n\
             - If is_busy is true, a command is already running — wait or use a different pane.\n\
             - Observation is not control. Pane title and visible screen text are raw observations, not typed runtime facts.\n\
             - Action history is causal evidence, not foreground truth. Use recent con actions to understand how a pane was reached, but rely on current probes and control capabilities before acting.\n\
             - `runtime_stack` is current-only. `last_verified_shell_stack` describes the most recent shell frame con verified, and may be historical.\n\
             - `pane_index` is positional within the current pane layout snapshot and can change when panes are created or closed. `pane_id` is the stable pane identity within the lifetime of this tab.\n\
             - After list_panes, create_pane, ensure_remote_shell_target, resolve_work_target, or remote_exec, carry `pane_id` forward for follow-up actions. Use `pane_index` only as a human-readable snapshot.\n\
             - Prefer typed attachments and probes over inference. If `probe_shell_context` is available, use it before guessing about SSH, tmux, or editor context.\n\
             - `<remote_workspaces>` is not a guess. It is con's current host-workspace inventory for this tab, built from pane-local runtime facts and con-managed SSH continuity.\n\
             - `<local_workspaces>` is con's current local coding-workspace inventory for this tab. Use it to reuse local shell and local agent-cli panes for the same project path instead of opening duplicates.\n\
             - Backend support is explicit. If `supports_foreground_command`, `supports_alt_screen`, or `supports_remote_host_identity` is false, treat missing runtime data as unavailable backend truth, not as proof of absence.\n\
             - `screen_hints` are weak observations derived from the current visible screen snapshot. They can describe what appears to be on screen now, but they are not backend facts and must not unlock control.\n\
             - Do not end a session-state answer with a vague offer to \"inspect more closely\". Only propose a next step when a stronger fact source is concretely available, such as `probe_shell_context`, `tmux_snapshot`, or tmux native tools already present on the pane.\n\
             - Addressing is layered. A con pane index ≠ tmux pane id ≠ tmux window index ≠ editor buffer.\n\
             - If list_panes shows `query_tmux`, `exec_tmux_command`, or `send_tmux_keys`, treat tmux as a native attachment. Do NOT navigate tmux by outer-pane send_keys unless the native path is unavailable.\n\
             - When pane mode is not `shell` or shell metadata is stale, do NOT trust cwd/hostname/last_command and do NOT assume a shell prompt.\n\
             - If a command fails (exit_code != 0), diagnose the error before retrying.\n\
             - When editing files: always read first, ensure old_text is unique, verify the edit succeeded.\n\
             </safety>\n\n\
             <verify_before_act>\n\
             MANDATORY: Observe before acting. Never assume terminal state.\n\
             - Before send_keys: read_pane to see what is on screen and where keystrokes will go.\n\
             - After send_keys: read_pane to verify the action took effect.\n\
             - After create_pane: check the returned output to see what happened (output is included).\n\
             - Never chain multiple actions without observing between them.\n\
             </verify_before_act>\n\n",
        );

        self.emit_tui_guide(&mut prompt);

        prompt.push_str("<terminal_context>\n");
        prompt.push_str(&format!(
            "<focused_pane index=\"{}\" pane_id=\"{}\" front_state=\"{}\" mode=\"{}\" shell_integration=\"{}\" shell_metadata_fresh=\"{}\" shell_context_fresh=\"{}\"",
            self.focused_pane_index,
            self.focused_pane_id,
            self.focused_front_state.as_str(),
            self.focused_pane_mode.as_str(),
            self.focused_has_shell_integration,
            self.focused_shell_metadata_fresh,
            self.focused_shell_context_fresh,
        ));
        prompt.push_str(&format!(
            " supports_foreground_command=\"{}\" supports_alt_screen=\"{}\" supports_remote_host_identity=\"{}\"",
            self.focused_observation_support.foreground_command,
            self.focused_observation_support.alternate_screen,
            self.focused_observation_support.remote_host_identity,
        ));
        if let Some(host) = &self.focused_hostname {
            prompt.push_str(&format!(" host=\"{}\"", xml_escape(host)));
            if let Some(confidence) = self.focused_hostname_confidence {
                prompt.push_str(&format!(" host_confidence=\"{}\"", confidence.as_str()));
            }
            if let Some(source) = self.focused_hostname_source {
                prompt.push_str(&format!(" host_source=\"{}\"", source.as_str()));
            }
        }
        if let Some(title) = &self.focused_title {
            prompt.push_str(&format!(" title=\"{}\"", xml_escape(title)));
        }
        if let Some(cwd) = &self.cwd {
            prompt.push_str(&format!(" cwd=\"{}\"", xml_escape(cwd)));
        }
        if let Some(scope) = self.focused_runtime_stack.last() {
            prompt.push_str(&format!(
                " active_scope=\"{}\"",
                xml_escape(&format_scope_summary(scope))
            ));
        }
        prompt.push_str("/>\n");
        prompt.push_str(&format!(
            "<focused_control address_space=\"{}\" visible_target=\"{}\" target_stack=\"{}\" attachments=\"{}\" channels=\"{}\" capabilities=\"{}\"",
            self.focused_control.address_space.as_str(),
            xml_escape(&self.focused_control.visible_target.summary()),
            xml_escape(&format_target_stack(&self.focused_control.target_stack)),
            xml_escape(&format_control_attachments(&self.focused_control.attachments)),
            xml_escape(&format_control_channels(&self.focused_control.channels)),
            xml_escape(&format_control_capabilities(&self.focused_control.capabilities)),
        ));
        if let Some(host) = &self.focused_control.visible_target.host {
            prompt.push_str(&format!(" target_host=\"{}\"", xml_escape(host)));
        }
        if let Some(tmux) = &self.focused_control.tmux {
            prompt.push_str(&format!(" tmux_mode=\"{}\"", tmux.mode.as_str()));
            if let Some(session) = &tmux.session_name {
                prompt.push_str(&format!(" tmux_session=\"{}\"", xml_escape(session)));
            }
        }
        prompt.push_str("/>\n");
        if let Some(tmux) = &self.focused_control.tmux {
            let front_target = tmux
                .front_target
                .as_ref()
                .map(PaneVisibleTarget::summary)
                .unwrap_or_else(|| "unknown".to_string());
            prompt.push_str(&format!(
                "<tmux_control mode=\"{}\" front_target=\"{}\">{}</tmux_control>\n",
                tmux.mode.as_str(),
                xml_escape(&front_target),
                xml_escape(&tmux.reason)
            ));
        }
        if let Some(context) = &self.focused_shell_context {
            prompt.push_str("<shell_context");
            prompt.push_str(&format!(
                " captured_input_generation=\"{}\" fresh=\"{}\"",
                context.captured_input_generation, self.focused_shell_context_fresh
            ));
            if let Some(host) = &context.host {
                prompt.push_str(&format!(" host=\"{}\"", xml_escape(host)));
            }
            if let Some(pwd) = &context.pwd {
                prompt.push_str(&format!(" pwd=\"{}\"", xml_escape(pwd)));
            }
            if let Some(tmux) = &context.tmux {
                if let Some(session) = &tmux.session_name {
                    prompt.push_str(&format!(" tmux_session=\"{}\"", xml_escape(session)));
                }
                if let Some(pane_id) = &tmux.pane_id {
                    prompt.push_str(&format!(" tmux_pane=\"{}\"", xml_escape(pane_id)));
                }
                if let Some(command) = &tmux.pane_current_command {
                    prompt.push_str(&format!(
                        " tmux_pane_current_command=\"{}\"",
                        xml_escape(command)
                    ));
                }
            }
            if let Some(nvim) = &context.nvim_listen_address {
                prompt.push_str(&format!(" nvim_socket=\"{}\"", xml_escape(nvim)));
            }
            prompt.push_str("/>\n");
        }
        if let Some(snapshot) = &self.focused_tmux_snapshot {
            prompt.push_str("<tmux_snapshot>\n");
            for pane in &snapshot.panes {
                prompt.push_str(&format!(
                    "  <tmux_pane session=\"{}\" window_id=\"{}\" window_name=\"{}\" pane_id=\"{}\" pane_index=\"{}\" active=\"{}\"",
                    xml_escape(&pane.session_name),
                    xml_escape(&pane.window_id),
                    xml_escape(&pane.window_name),
                    xml_escape(&pane.pane_id),
                    xml_escape(&pane.pane_index),
                    pane.pane_active,
                ));
                if let Some(command) = &pane.pane_current_command {
                    prompt.push_str(&format!(" current_command=\"{}\"", xml_escape(command)));
                }
                if let Some(path) = &pane.pane_current_path {
                    prompt.push_str(&format!(" current_path=\"{}\"", xml_escape(path)));
                }
                prompt.push_str("/>\n");
            }
            prompt.push_str("</tmux_snapshot>\n");
        }
        if !self.focused_recent_actions.is_empty() {
            prompt.push_str("<recent_actions>\n");
            for action in &self.focused_recent_actions {
                prompt.push_str(&format!(
                    "  <action kind=\"{}\" source=\"{}\" confidence=\"{}\"",
                    action.kind.as_str(),
                    action.source.as_str(),
                    action.confidence.as_str(),
                ));
                if let Some(input_generation) = action.input_generation {
                    prompt.push_str(&format!(" input_generation=\"{}\"", input_generation));
                }
                prompt.push_str(">");
                prompt.push_str(&xml_escape(&action.summary));
                prompt.push_str("</action>\n");
            }
            prompt.push_str("</recent_actions>\n");
        }
        for note in &self.focused_control.notes {
            prompt.push_str(&format!(
                "<control_note>{}</control_note>\n",
                xml_escape(note)
            ));
        }
        if !self.focused_runtime_stack.is_empty() {
            prompt.push_str("<runtime_stack>\n");
            for scope in &self.focused_runtime_stack {
                prompt.push_str(&format!(
                    "  <scope kind=\"{}\" confidence=\"{}\" source=\"{}\"",
                    scope.kind.as_str(),
                    scope.confidence.as_str(),
                    scope.evidence_source.as_str(),
                ));
                if let Some(label) = &scope.label {
                    prompt.push_str(&format!(" label=\"{}\"", xml_escape(label)));
                }
                if let Some(host) = &scope.host {
                    prompt.push_str(&format!(" host=\"{}\"", xml_escape(host)));
                }
                prompt.push_str("/>\n");
            }
            prompt.push_str("</runtime_stack>\n");
        }
        if !self.focused_last_verified_runtime_stack.is_empty() {
            prompt.push_str("<last_verified_shell_stack>\n");
            for scope in &self.focused_last_verified_runtime_stack {
                prompt.push_str(&format!(
                    "  <scope kind=\"{}\" confidence=\"{}\" source=\"{}\"",
                    scope.kind.as_str(),
                    scope.confidence.as_str(),
                    scope.evidence_source.as_str(),
                ));
                if let Some(label) = &scope.label {
                    prompt.push_str(&format!(" label=\"{}\"", xml_escape(label)));
                }
                if let Some(host) = &scope.host {
                    prompt.push_str(&format!(" host=\"{}\"", xml_escape(host)));
                }
                prompt.push_str("/>\n");
            }
            prompt.push_str("</last_verified_shell_stack>\n");
        }
        for warning in &self.focused_runtime_warnings {
            prompt.push_str(&format!(
                "<metadata_warning>{}</metadata_warning>\n",
                xml_escape(warning)
            ));
        }

        let total_panes = 1 + self.other_panes.len();

        // When multiple panes are open, embed a full pane layout so the agent
        // can target the right pane(s) without needing to call list_panes first.
        if total_panes > 1 {
            prompt.push_str("<panes>\n");
            // Focused pane
            let cwd_label = self.cwd.as_deref().unwrap_or("?");
            prompt.push_str(&format!(
                "  <pane index=\"{}\" pane_id=\"{}\" focused=\"true\" cwd=\"{}\" front_state=\"{}\" mode=\"{}\" shell_metadata_fresh=\"{}\" runtime=\"{}\" last_verified_shell_stack=\"{}\" control_target=\"{}\" target_stack=\"{}\" control_attachments=\"{}\" control_channels=\"{}\" control_capabilities=\"{}\"",
                self.focused_pane_index,
                self.focused_pane_id,
                xml_escape(cwd_label),
                self.focused_front_state.as_str(),
                self.focused_pane_mode.as_str(),
                self.focused_shell_metadata_fresh,
                xml_escape(&format_runtime_stack(&self.focused_runtime_stack)),
                xml_escape(&format_runtime_stack(&self.focused_last_verified_runtime_stack)),
                xml_escape(&self.focused_control.visible_target.summary()),
                xml_escape(&format_target_stack(&self.focused_control.target_stack)),
                xml_escape(&format_control_attachments(&self.focused_control.attachments)),
                xml_escape(&format_control_channels(&self.focused_control.channels)),
                xml_escape(&format_control_capabilities(&self.focused_control.capabilities))
            ));
            prompt.push_str(&format!(
                " supports_foreground_command=\"{}\" supports_alt_screen=\"{}\" supports_remote_host_identity=\"{}\"",
                self.focused_observation_support.foreground_command,
                self.focused_observation_support.alternate_screen,
                self.focused_observation_support.remote_host_identity,
            ));
            if let Some(host) = &self.focused_hostname {
                prompt.push_str(&format!(" host=\"{}\"", xml_escape(host)));
                if let Some(confidence) = self.focused_hostname_confidence {
                    prompt.push_str(&format!(" host_confidence=\"{}\"", confidence.as_str()));
                }
            } else if let Some(anchor) = &self.focused_remote_workspace {
                prompt.push_str(&format!(
                    " remote_workspace_host=\"{}\" remote_workspace_source=\"{}\"",
                    xml_escape(&anchor.host),
                    anchor.source.as_str()
                ));
            } else {
                prompt.push_str(" host=\"unknown\"");
            }
            if let Some(scope) = self.focused_runtime_stack.last() {
                prompt.push_str(&format!(
                    " active_scope=\"{}\"",
                    xml_escape(&format_scope_summary(scope))
                ));
            }
            prompt.push_str("/>\n");
            // Other panes
            for pane in &self.other_panes {
                let cwd = pane.cwd.as_deref().unwrap_or("?");
                let busy = if pane.is_busy { " busy=\"true\"" } else { "" };
                let stale = if pane.shell_metadata_fresh {
                    " shell_metadata_fresh=\"true\""
                } else {
                    " shell_metadata_fresh=\"false\""
                };
                let tmux = pane
                    .tmux_session
                    .as_deref()
                    .map(|session| format!(" tmux=\"{}\"", xml_escape(session)))
                    .unwrap_or_default();
                let host = pane
                    .hostname
                    .as_deref()
                    .map(|host| format!(" host=\"{}\"", xml_escape(host)))
                    .unwrap_or_else(|| " host=\"unknown\"".to_string());
                let remote_workspace = pane
                    .remote_workspace
                    .as_ref()
                    .map(|anchor| {
                        format!(
                            " remote_workspace_host=\"{}\" remote_workspace_source=\"{}\"",
                            xml_escape(&anchor.host),
                            anchor.source.as_str()
                        )
                    })
                    .unwrap_or_default();
                let host_confidence = pane
                    .hostname_confidence
                    .map(|confidence| format!(" host_confidence=\"{}\"", confidence.as_str()))
                    .unwrap_or_default();
                let active_scope = pane
                    .active_scope
                    .as_ref()
                    .map(|scope| {
                        format!(
                            " active_scope=\"{}\"",
                            xml_escape(&format_scope_summary(scope))
                        )
                    })
                    .unwrap_or_default();
                let control_target = format!(
                    " control_target=\"{}\"",
                    xml_escape(&pane.control.visible_target.summary())
                );
                let target_stack = format!(
                    " target_stack=\"{}\"",
                    xml_escape(&format_target_stack(&pane.control.target_stack))
                );
                let control_channels = format!(
                    " control_channels=\"{}\"",
                    xml_escape(&format_control_channels(&pane.control.channels))
                );
                let control_attachments = format!(
                    " control_attachments=\"{}\"",
                    xml_escape(&format_control_attachments(&pane.control.attachments))
                );
                let control_capabilities = format!(
                    " control_capabilities=\"{}\"",
                    xml_escape(&format_control_capabilities(&pane.control.capabilities))
                );
                let tmux_control = pane
                    .control
                    .tmux
                    .as_ref()
                    .map(|tmux| format!(" tmux_mode=\"{}\"", tmux.mode.as_str()))
                    .unwrap_or_default();
                let support = format!(
                    " supports_foreground_command=\"{}\" supports_alt_screen=\"{}\" supports_remote_host_identity=\"{}\"",
                    pane.observation_support.foreground_command,
                    pane.observation_support.alternate_screen,
                    pane.observation_support.remote_host_identity,
                );
                prompt.push_str(&format!(
                    "  <pane index=\"{}\" pane_id=\"{}\" cwd=\"{}\" front_state=\"{}\" mode=\"{}\" runtime=\"{}\" last_verified_shell_stack=\"{}\"{}{}{}{}{}{}{}{}{}{}{}{}{}{}/>\n",
                    pane.pane_index,
                    pane.pane_id,
                    xml_escape(cwd),
                    pane.front_state.as_str(),
                    pane.mode.as_str(),
                    xml_escape(&format_runtime_stack(&pane.runtime_stack)),
                    xml_escape(&format_runtime_stack(&pane.last_verified_runtime_stack)),
                    host,
                    remote_workspace,
                    host_confidence,
                    active_scope,
                    busy,
                    stale,
                    tmux,
                    control_target,
                    target_stack,
                    tmux_control,
                    control_attachments,
                    control_channels,
                    control_capabilities,
                    support,
                ));
                for note in &pane.control.notes {
                    prompt.push_str(&format!(
                        "  <pane_control_note index=\"{}\">{}</pane_control_note>\n",
                        pane.pane_index,
                        xml_escape(note)
                    ));
                }
            }
            prompt.push_str("</panes>\n");
            prompt.push_str("<pane_layout_summary>\n");
            prompt.push_str(&xml_escape(&summarize_focused_pane_layout(self)));
            prompt.push('\n');
            for pane in &self.other_panes {
                prompt.push_str(&xml_escape(&summarize_peer_pane_layout(pane)));
                prompt.push('\n');
            }
            prompt.push_str("</pane_layout_summary>\n");
        } else if let Some(cwd) = &self.cwd {
            prompt.push_str(&format!("<cwd>{}</cwd>\n", xml_escape(cwd)));
        }

        let mut remote_workspaces = Vec::new();
        let focused_tmux_like = self
            .focused_screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
        let focused_disconnected = self
            .focused_screen_hints
            .iter()
            .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
        if !focused_tmux_like && !focused_disconnected {
            if let Some(anchor) = &self.focused_remote_workspace {
                remote_workspaces.push((self.focused_pane_index, self.focused_pane_id, anchor));
            }
        }
        for pane in &self.other_panes {
            let tmux_like = pane
                .screen_hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen);
            let disconnected = pane
                .screen_hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::SshConnectionClosed);
            if !tmux_like && !disconnected {
                if let Some(anchor) = &pane.remote_workspace {
                    remote_workspaces.push((pane.pane_index, pane.pane_id, anchor));
                }
            }
        }
        remote_workspaces.sort_by(|(pane_a, _, anchor_a), (pane_b, _, anchor_b)| {
            anchor_a
                .host
                .cmp(&anchor_b.host)
                .then_with(|| pane_a.cmp(pane_b))
        });
        remote_workspaces.dedup_by(|(_, _, a), (_, _, b)| a.host == b.host);
        if !remote_workspaces.is_empty() {
            prompt.push_str("<remote_workspaces>\n");
            for (pane_index, pane_id, anchor) in remote_workspaces {
                prompt.push_str(&format!(
                    "  <workspace host=\"{}\" pane_index=\"{}\" pane_id=\"{}\" source=\"{}\" confidence=\"{}\">{}</workspace>\n",
                    xml_escape(&anchor.host),
                    pane_index,
                    pane_id,
                    anchor.source.as_str(),
                    anchor.confidence.as_str(),
                    xml_escape(&anchor.note)
                ));
            }
            prompt.push_str("</remote_workspaces>\n");
        }

        let tab_workspaces = tab_workspaces(self);
        let mut local_workspaces: Vec<&TabWorkspaceSummary> = tab_workspaces
            .iter()
            .filter(|workspace| {
                workspace.kind == TabWorkspaceKind::LocalShell
                    && workspace.state != TabWorkspaceState::Disconnected
            })
            .collect();
        local_workspaces.sort_by(|a, b| {
            a.agent_cli
                .is_none()
                .cmp(&b.agent_cli.is_none())
                .then_with(|| a.pane_index.cmp(&b.pane_index))
        });
        if !local_workspaces.is_empty() {
            prompt.push_str("<local_workspaces>\n");
            for workspace in local_workspaces {
                prompt.push_str(&format!(
                    "  <workspace pane_index=\"{}\" pane_id=\"{}\" state=\"{}\"",
                    workspace.pane_index,
                    workspace.pane_id,
                    workspace.state.as_str()
                ));
                if let Some(agent) = &workspace.agent_cli {
                    prompt.push_str(&format!(" agent_cli=\"{}\"", xml_escape(agent)));
                }
                if let Some(cwd) = &workspace.cwd {
                    prompt.push_str(&format!(" cwd=\"{}\"", xml_escape(cwd)));
                }
                prompt.push_str(">");
                prompt.push_str(&xml_escape(&workspace.note));
                prompt.push_str("</workspace>\n");
            }
            prompt.push_str("</local_workspaces>\n");
        }

        if !tab_workspaces.is_empty() {
            prompt.push_str("<tab_workspaces>\n");
            for workspace in &tab_workspaces {
                prompt.push_str(&format!(
                    "  <workspace pane_index=\"{}\" pane_id=\"{}\" kind=\"{}\" state=\"{}\"",
                    workspace.pane_index,
                    workspace.pane_id,
                    workspace.kind.as_str(),
                    workspace.state.as_str()
                ));
                if let Some(host) = &workspace.host {
                    prompt.push_str(&format!(" host=\"{}\"", xml_escape(host)));
                }
                if let Some(session) = &workspace.tmux_session {
                    prompt.push_str(&format!(" tmux_session=\"{}\"", xml_escape(session)));
                }
                if let Some(cwd) = &workspace.cwd {
                    prompt.push_str(&format!(" cwd=\"{}\"", xml_escape(cwd)));
                }
                if let Some(agent) = &workspace.agent_cli {
                    prompt.push_str(&format!(" agent_cli=\"{}\"", xml_escape(agent)));
                }
                prompt.push_str(">");
                prompt.push_str(&xml_escape(&workspace.note));
                prompt.push_str("</workspace>\n");
            }
            prompt.push_str("</tab_workspaces>\n");
        }

        let work_target_hints = preferred_work_target_hints(self);
        if !work_target_hints.is_empty() {
            prompt.push_str("<work_target_hints>\n");
            for hint in work_target_hints {
                prompt.push_str(&xml_escape(&hint));
                prompt.push('\n');
            }
            prompt.push_str("</work_target_hints>\n");
        }

        if let Some(branch) = &self.git_branch {
            prompt.push_str(&format!(
                "<git_branch>{}</git_branch>\n",
                xml_escape(branch)
            ));
        }

        if let Some(host) = &self.ssh_host {
            prompt.push_str(&format!("<ssh_host>{}</ssh_host>\n", xml_escape(host)));
        }

        if let Some(session) = &self.tmux_session {
            prompt.push_str(&format!(
                "<tmux_session>{}</tmux_session>\n",
                xml_escape(session)
            ));
        }

        if let Some(cmd) = &self.last_command {
            let mut attrs = String::new();
            if let Some(code) = self.last_exit_code {
                attrs.push_str(&format!(" exit_code=\"{}\"", code));
            }
            if let Some(dur) = self.last_command_duration_secs {
                attrs.push_str(&format!(" duration=\"{:.1}s\"", dur));
            }
            prompt.push_str(&format!(
                "<last_command{}>{}</last_command>\n",
                attrs,
                xml_escape(cmd)
            ));
        }

        if !self.command_history.is_empty() {
            prompt.push_str("<command_history>\n");
            for block in &self.command_history {
                match block.exit_code {
                    Some(code) => prompt.push_str(&format!(
                        "$ {} (exit {})\n",
                        xml_escape(&block.command),
                        code
                    )),
                    None => prompt.push_str(&format!("$ {}\n", xml_escape(&block.command))),
                }
            }
            prompt.push_str("</command_history>\n");
        }

        if !self.recent_output.is_empty() {
            prompt.push_str("<terminal_output source=\"current_visible_screen\">\n");
            for line in &self.recent_output {
                prompt.push_str(&xml_escape(line));
                prompt.push('\n');
            }
            prompt.push_str("</terminal_output>\n");
        }
        if !self.focused_screen_hints.is_empty() {
            prompt.push_str("<screen_hints>\n");
            for hint in &self.focused_screen_hints {
                prompt.push_str(&format!(
                    "  <hint kind=\"{}\" confidence=\"{}\">{}</hint>\n",
                    hint.kind.as_str(),
                    hint.confidence.as_str(),
                    xml_escape(&hint.detail)
                ));
            }
            prompt.push_str("</screen_hints>\n");
        }
        if let Some(assessment) = focused_screen_assessment(
            &self.focused_screen_hints,
            &self.focused_control,
            self.focused_tmux_snapshot.as_ref(),
        ) {
            prompt.push_str("<current_screen_assessment>");
            prompt.push_str(&xml_escape(&assessment));
            prompt.push_str("</current_screen_assessment>\n");
        }

        if let Some(diff) = &self.git_diff {
            prompt.push_str("<git_diff>\n");
            prompt.push_str(&xml_escape(diff));
            if !diff.ends_with('\n') {
                prompt.push('\n');
            }
            prompt.push_str("</git_diff>\n");
        }

        if let Some(structure) = &self.project_structure {
            prompt.push_str("<project_structure>\n");
            prompt.push_str(&xml_escape(structure));
            if !structure.ends_with('\n') {
                prompt.push('\n');
            }
            prompt.push_str("</project_structure>\n");
        }

        prompt.push_str("</terminal_context>\n");

        if let Some(agents_md) = &self.agents_md {
            prompt.push_str("\n<agents_md>\n");
            prompt.push_str(&xml_escape(agents_md));
            if !agents_md.ends_with('\n') {
                prompt.push('\n');
            }
            prompt.push_str("</agents_md>\n");
        }

        if !self.skills.is_empty() {
            prompt.push_str("\n<skills>\nThe user can invoke these skills with /name. When a skill is invoked, follow its intent:\n");
            for (name, desc) in &self.skills {
                prompt.push_str(&format!("  /{} — {}\n", xml_escape(name), xml_escape(desc)));
            }
            prompt.push_str("</skills>\n");
        }

        prompt
    }

    /// Emit contextual TUI interaction guidance when any visible pane has a TUI target.
    /// Only adds content when a TUI is detected — shell-only sessions are unchanged.
    fn emit_tui_guide(&self, prompt: &mut String) {
        use crate::control::PaneVisibleTargetKind;
        use crate::playbooks;

        let focused_is_tui = matches!(
            self.focused_control.visible_target.kind,
            PaneVisibleTargetKind::InteractiveApp
                | PaneVisibleTargetKind::TmuxSession
                | PaneVisibleTargetKind::AgentCli
        );
        let any_other_pane_tui = self.other_panes.iter().any(|p| {
            matches!(
                p.control.visible_target.kind,
                PaneVisibleTargetKind::InteractiveApp
                    | PaneVisibleTargetKind::TmuxSession
                    | PaneVisibleTargetKind::AgentCli
            )
        });
        let is_remote = self.focused_hostname.is_some()
            || self.ssh_host.is_some()
            || self
                .focused_shell_context
                .as_ref()
                .is_some_and(|context| context.ssh_connection.is_some());
        let has_tmux = self
            .focused_control
            .target_stack
            .iter()
            .any(|t| t.kind == PaneVisibleTargetKind::TmuxSession)
            || self
                .focused_last_verified_runtime_stack
                .iter()
                .any(|scope| scope.kind == PaneScopeKind::Multiplexer)
            || self.other_panes.iter().any(|p| {
                p.control
                    .target_stack
                    .iter()
                    .any(|t| t.kind == PaneVisibleTargetKind::TmuxSession)
                    || p.last_verified_runtime_stack
                        .iter()
                        .any(|scope| scope.kind == PaneScopeKind::Multiplexer)
            });
        let has_local_agent_cli = (self.focused_control.visible_target.kind
            == PaneVisibleTargetKind::AgentCli
            && !is_remote
            && !has_tmux)
            || self.other_panes.iter().any(|p| {
                p.control.visible_target.kind == PaneVisibleTargetKind::AgentCli
                    && p.hostname.is_none()
                    && p.remote_workspace.is_none()
                    && !p
                        .control
                        .target_stack
                        .iter()
                        .any(|t| t.kind == PaneVisibleTargetKind::TmuxSession)
            });

        if !focused_is_tui && !any_other_pane_tui && !is_remote && !has_tmux {
            return;
        }

        prompt.push_str("<tui_interaction_guide>\n");

        // Remote work rules come first — they change what tools are valid
        if is_remote {
            prompt.push_str(playbooks::REMOTE_WORK);
            prompt.push('\n');
        }
        if has_local_agent_cli {
            prompt.push_str(playbooks::LOCAL_AGENT_CLI_WORK);
            prompt.push('\n');
        }

        if focused_is_tui || any_other_pane_tui || has_tmux {
            prompt.push_str(playbooks::VERIFY_AFTER_ACT);
            prompt.push('\n');
        }

        // Check for tmux anywhere in focused stack or other panes
        if has_tmux {
            prompt.push_str(playbooks::TMUX_PLAYBOOK);
            prompt.push('\n');
        }
        if (focused_is_tui || any_other_pane_tui) && !has_tmux {
            prompt.push_str(playbooks::GENERAL_TUI);
            prompt.push('\n');
        }

        prompt.push_str("</tui_interaction_guide>\n\n");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn classify_screen_agent_cli_detects_coding_clis_and_skips_plain_shell() {
        let claude = vec!["✻ Ideating…".to_string(), "Claude Code".to_string()];
        assert_eq!(super::classify_screen_agent_cli(None, &claude), Some("claude"));
        let codex = vec!["OpenAI Codex".to_string()];
        assert_eq!(super::classify_screen_agent_cli(None, &codex), Some("codex"));
        let plain_shell = vec!["tony@host ~ %".to_string(), "ls -la".to_string()];
        assert_eq!(super::classify_screen_agent_cli(None, &plain_shell), None);
    }

    use crate::control::{PaneControlState, TmuxControlMode, TmuxControlState};
    use crate::shell_probe::{ShellProbeResult, ShellProbeTmuxContext};
    use crate::tmux::{TmuxPaneInfo, TmuxSnapshot};

    use super::{
        PaneActionKind, PaneActionRecord, PaneConfidence, PaneEvidenceSource, PaneFrontState,
        PaneMode, PaneObservationFrame, PaneObservationHint, PaneObservationHintKind,
        PaneObservationSupport, PaneRuntimeEvent, PaneRuntimeScope, PaneRuntimeState,
        PaneRuntimeTracker, PaneScopeKind, PaneSummary, TerminalContext, derive_screen_hints,
        detect_tmux_session, direct_terminal_exec_is_safe, infer_pane_mode,
        remote_workspace_anchor, shell_metadata_is_fresh,
    };

    #[test]
    fn detects_tmux_from_command_target() {
        let session = detect_tmux_session(Some("tmux attach -t model-serving"));
        assert_eq!(session.as_deref(), Some("model-serving"));
    }

    #[test]
    fn detects_tmux_from_new_session_name() {
        let session = detect_tmux_session(Some("tmux new-session -d -s con-bench"));
        assert_eq!(session.as_deref(), Some("con-bench"));
    }

    #[test]
    fn workspace_cwd_hint_parses_cd_inside_startup_chain() {
        let actions = vec![PaneActionRecord {
            sequence: 1,
            kind: PaneActionKind::PaneCreated,
            summary: "startup".to_string(),
            command: Some(
                "mkdir -p /Users/weyl/dev/temp/con-bench-twosum && cd /Users/weyl/dev/temp/con-bench-twosum && codex"
                    .to_string(),
            ),
            source: PaneEvidenceSource::ActionHistory,
            confidence: PaneConfidence::Advisory,
            input_generation: None,
            note: None,
        }];

        let hint = super::workspace_cwd_hint(None, &actions);
        assert_eq!(
            hint.as_deref(),
            Some("/Users/weyl/dev/temp/con-bench-twosum")
        );
    }

    #[test]
    fn shell_metadata_is_fresh_requires_command_boundary() {
        assert!(shell_metadata_is_fresh(true, 0, 0));
        assert!(!shell_metadata_is_fresh(true, 2, 1));
        assert!(!shell_metadata_is_fresh(false, 0, 0));
    }

    #[test]
    fn infer_pane_mode_requires_confirmed_shell_prompt() {
        assert_eq!(infer_pane_mode(None, true, false, 0, 0), PaneMode::Shell);
        assert_eq!(infer_pane_mode(None, true, false, 2, 1), PaneMode::Unknown);
    }

    #[test]
    fn title_and_screen_do_not_create_structured_tmux_state() {
        let observation = PaneObservationFrame {
            title: Some("haswell ❐ 0 ● 4 nvim".to_string()),
            cwd: None,
            recent_output: vec![
                "❐ 0  ↑ 63d 6h 21m  <4 nvim      ↗  | 11:31 | 05 Apr  w  haswell".to_string(),
            ],
            screen_hints: Vec::new(),
            last_command: None,
            last_exit_code: None,
            last_command_duration_secs: None,
            support: PaneObservationSupport::default(),
            has_shell_integration: false,
            is_alt_screen: false,
            is_busy: false,
            input_generation: 0,
            last_command_finished_input_generation: 0,
        };

        let runtime = PaneRuntimeState::from_observation(&observation);
        assert_eq!(runtime.mode, PaneMode::Unknown);
        assert!(runtime.scope_stack.is_empty());
        assert_eq!(runtime.tmux_session, None);
    }

    #[test]
    fn runtime_state_keeps_tmux_command_history_out_of_foreground_state() {
        let observation = PaneObservationFrame {
            title: Some("tmux".to_string()),
            cwd: Some("/home/w".to_string()),
            recent_output: vec!["".to_string()],
            screen_hints: Vec::new(),
            last_command: Some("tmux attach -t deploy".to_string()),
            last_exit_code: Some(0),
            last_command_duration_secs: Some(1.2),
            support: PaneObservationSupport {
                foreground_command: true,
                ..PaneObservationSupport::default()
            },
            has_shell_integration: true,
            is_alt_screen: false,
            is_busy: true,
            input_generation: 1,
            last_command_finished_input_generation: 0,
        };

        let runtime = PaneRuntimeState::from_observation(&observation);
        assert_eq!(runtime.front_state, PaneFrontState::Unknown);
        assert_eq!(runtime.mode, PaneMode::Unknown);
        assert!(runtime.scope_stack.is_empty());
        assert!(runtime.last_verified_scope_stack.is_empty());
    }

    #[test]
    fn observer_does_not_promote_tmux_command_history_to_foreground_state() {
        let mut observer = PaneRuntimeTracker::default();

        let tmux = PaneObservationFrame {
            title: Some("tmux".to_string()),
            cwd: Some("/home/w".to_string()),
            recent_output: vec!["".to_string()],
            screen_hints: Vec::new(),
            last_command: Some("tmux a -t work".to_string()),
            last_exit_code: None,
            last_command_duration_secs: None,
            support: PaneObservationSupport {
                foreground_command: true,
                ..PaneObservationSupport::default()
            },
            has_shell_integration: true,
            is_alt_screen: false,
            is_busy: true,
            input_generation: 1,
            last_command_finished_input_generation: 0,
        };
        let sparse = PaneObservationFrame {
            title: Some("tmux".to_string()),
            cwd: Some("/home/w".to_string()),
            recent_output: vec!["".to_string()],
            screen_hints: Vec::new(),
            last_command: None,
            last_exit_code: None,
            last_command_duration_secs: None,
            support: PaneObservationSupport::default(),
            has_shell_integration: true,
            is_alt_screen: false,
            is_busy: true,
            input_generation: 1,
            last_command_finished_input_generation: 0,
        };
        let shell = PaneObservationFrame {
            title: Some("bash".to_string()),
            cwd: Some("/Users/weyl/conductor/workspaces/con/kingston".to_string()),
            recent_output: vec!["$".to_string()],
            screen_hints: Vec::new(),
            last_command: Some("cargo test".to_string()),
            last_exit_code: Some(0),
            last_command_duration_secs: Some(1.0),
            support: PaneObservationSupport {
                foreground_command: true,
                ..PaneObservationSupport::default()
            },
            has_shell_integration: true,
            is_alt_screen: false,
            is_busy: false,
            input_generation: 1,
            last_command_finished_input_generation: 1,
        };

        let tmux_runtime = observer.observe(tmux);
        let sparse_runtime = observer.observe(sparse);
        let shell_runtime = observer.observe(shell);

        assert_eq!(tmux_runtime.mode, PaneMode::Unknown);
        assert_eq!(sparse_runtime.mode, PaneMode::Unknown);
        assert_eq!(shell_runtime.mode, PaneMode::Shell);
        assert_eq!(shell_runtime.tmux_session, None);
    }

    #[test]
    fn runtime_state_keeps_agent_cli_history_out_of_foreground_state() {
        let observation = PaneObservationFrame {
            title: Some("Codex".to_string()),
            cwd: Some("/tmp".to_string()),
            recent_output: vec!["".to_string()],
            screen_hints: Vec::new(),
            last_command: Some("codex".to_string()),
            last_exit_code: None,
            last_command_duration_secs: None,
            support: PaneObservationSupport {
                foreground_command: true,
                ..PaneObservationSupport::default()
            },
            has_shell_integration: true,
            is_alt_screen: false,
            is_busy: true,
            input_generation: 1,
            last_command_finished_input_generation: 0,
        };

        let runtime = PaneRuntimeState::from_observation(&observation);
        assert_eq!(runtime.front_state, PaneFrontState::Unknown);
        assert_eq!(runtime.mode, PaneMode::Unknown);
        assert_eq!(runtime.agent_cli, None);
        assert_eq!(runtime.active_scope, None);
    }

    #[test]
    fn runtime_state_detects_codex_from_visible_screen() {
        let observation = PaneObservationFrame {
            title: Some("con-bench-codex-git".to_string()),
            cwd: Some("/tmp".to_string()),
            recent_output: vec![
                "╭────────────────────────────────────────────╮".to_string(),
                "│ >_ OpenAI Codex (v0.118.0)                 │".to_string(),
                "│ model:     gpt-5.4 high   /model to change │".to_string(),
                "╰────────────────────────────────────────────╯".to_string(),
            ],
            screen_hints: derive_screen_hints(
                Some("con-bench-codex-git"),
                &[
                    "╭────────────────────────────────────────────╮".to_string(),
                    "│ >_ OpenAI Codex (v0.118.0)                 │".to_string(),
                    "│ model:     gpt-5.4 high   /model to change │".to_string(),
                    "╰────────────────────────────────────────────╯".to_string(),
                ],
            ),
            last_command: None,
            last_exit_code: None,
            last_command_duration_secs: None,
            support: PaneObservationSupport {
                foreground_command: true,
                ..PaneObservationSupport::default()
            },
            has_shell_integration: true,
            is_alt_screen: false,
            is_busy: true,
            input_generation: 1,
            last_command_finished_input_generation: 0,
        };

        let runtime = PaneRuntimeState::from_observation(&observation);
        assert_eq!(runtime.agent_cli.as_deref(), Some("codex"));
    }

    #[test]
    fn shell_probe_turns_tmux_shell_into_nested_runtime_stack() {
        let mut tracker = PaneRuntimeTracker::default();
        tracker.record_action(PaneRuntimeEvent::ShellProbe {
            result: ShellProbeResult {
                host: Some("haswell".to_string()),
                pwd: Some("/home/weyl".to_string()),
                term: Some("xterm-ghostty".to_string()),
                term_program: Some("con".to_string()),
                ssh_connection: Some("1.2.3.4 5555 5.6.7.8 22".to_string()),
                ssh_tty: Some("/dev/pts/7".to_string()),
                tmux_env: Some("/tmp/tmux-1000/default,123,0".to_string()),
                nvim_listen_address: None,
                tmux: Some(ShellProbeTmuxContext {
                    session_name: Some("work".to_string()),
                    window_id: Some("@3".to_string()),
                    window_name: Some("shell".to_string()),
                    pane_id: Some("%17".to_string()),
                    pane_current_command: Some("zsh".to_string()),
                    pane_current_path: Some("/home/weyl".to_string()),
                    client_tty: Some("/dev/pts/7".to_string()),
                }),
                facts: Default::default(),
            },
            captured_input_generation: 3,
        });

        let observation = PaneObservationFrame {
            title: Some("zsh".to_string()),
            cwd: Some("/home/weyl".to_string()),
            recent_output: vec!["$".to_string()],
            screen_hints: Vec::new(),
            last_command: Some("ls".to_string()),
            last_exit_code: Some(0),
            last_command_duration_secs: Some(0.1),
            support: PaneObservationSupport::default(),
            has_shell_integration: true,
            is_alt_screen: false,
            is_busy: false,
            input_generation: 3,
            last_command_finished_input_generation: 3,
        };

        let runtime = tracker.observe(observation);
        let control = PaneControlState::from_runtime(&runtime);

        assert_eq!(runtime.mode, PaneMode::Shell);
        assert_eq!(runtime.front_state, PaneFrontState::ShellPrompt);
        assert_eq!(runtime.remote_host.as_deref(), Some("haswell"));
        assert_eq!(runtime.tmux_session.as_deref(), Some("work"));
        assert!(runtime.shell_context_fresh);
        assert_eq!(
            runtime
                .scope_stack
                .iter()
                .map(PaneRuntimeScope::summary)
                .collect::<Vec<_>>(),
            vec![
                "remote_shell(haswell)".to_string(),
                "multiplexer(work)".to_string(),
                "shell".to_string(),
            ]
        );
        assert_eq!(
            runtime
                .last_verified_scope_stack
                .iter()
                .map(PaneRuntimeScope::summary)
                .collect::<Vec<_>>(),
            vec![
                "remote_shell(haswell)".to_string(),
                "multiplexer(work)".to_string(),
                "shell".to_string(),
            ]
        );
        assert_eq!(
            crate::control::format_target_stack(&control.target_stack),
            "remote_shell(haswell) -> tmux_session(work) -> shell_prompt(haswell)"
        );
        assert!(direct_terminal_exec_is_safe(&runtime));
    }

    #[test]
    fn alt_screen_creates_strong_interactive_scope() {
        let observation = PaneObservationFrame {
            title: Some("nvim test.sh".to_string()),
            cwd: Some("/tmp".to_string()),
            recent_output: vec!["".to_string()],
            screen_hints: Vec::new(),
            last_command: Some("nvim test.sh".to_string()),
            last_exit_code: None,
            last_command_duration_secs: None,
            support: PaneObservationSupport {
                foreground_command: true,
                alternate_screen: true,
                ..PaneObservationSupport::default()
            },
            has_shell_integration: true,
            is_alt_screen: true,
            is_busy: true,
            input_generation: 1,
            last_command_finished_input_generation: 0,
        };

        let runtime = PaneRuntimeState::from_observation(&observation);
        assert_eq!(runtime.front_state, PaneFrontState::InteractiveSurface);
        assert_eq!(runtime.mode, PaneMode::Tui);
        assert_eq!(
            runtime.active_scope,
            Some(PaneRuntimeScope {
                kind: PaneScopeKind::InteractiveApp,
                label: None,
                host: None,
                confidence: PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::SurfaceState,
            })
        );
    }

    #[test]
    fn system_prompt_marks_unknown_host_as_unknown_not_local() {
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Shell,
            shell_metadata_fresh: false,
            screen_prompt_like: false,
            screen_tmux_like: false,
            screen_ssh_disconnected: false,
            remote_host: None,
            remote_host_confidence: None,
            remote_host_source: None,
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: Vec::new(),
            last_verified_tmux_session: None,
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: Vec::new(),
            warnings: Vec::new(),
        };
        let prompt = TerminalContext {
            focused_pane_index: 1,
            focused_pane_id: 1,
            focused_hostname: None,
            focused_hostname_confidence: None,
            focused_hostname_source: None,
            focused_title: Some("Terminal".to_string()),
            focused_front_state: PaneFrontState::Unknown,
            focused_pane_mode: PaneMode::Shell,
            focused_has_shell_integration: false,
            focused_shell_metadata_fresh: false,
            focused_observation_support: PaneObservationSupport::default(),
            focused_runtime_stack: Vec::new(),
            focused_last_verified_runtime_stack: Vec::new(),
            focused_runtime_warnings: Vec::new(),
            focused_control: PaneControlState::from_runtime(&runtime),
            focused_shell_context: None,
            focused_shell_context_fresh: false,
            focused_recent_actions: Vec::new(),
            focused_remote_workspace: None,
            cwd: Some("/tmp".to_string()),
            recent_output: Vec::new(),
            focused_screen_hints: Vec::new(),
            focused_tmux_snapshot: None,
            last_command: None,
            last_exit_code: None,
            last_command_duration_secs: None,
            git_branch: None,
            ssh_host: None,
            tmux_session: None,
            agents_md: None,
            skills: Vec::new(),
            command_history: Vec::new(),
            other_panes: vec![],
            git_diff: None,
            project_structure: None,
        }
        .to_system_prompt();

        assert!(!prompt.contains("host=\"local\""));
    }

    #[test]
    fn direct_terminal_exec_requires_fresh_shell() {
        let shell_runtime = PaneRuntimeState {
            front_state: PaneFrontState::ShellPrompt,
            mode: PaneMode::Shell,
            shell_metadata_fresh: true,
            screen_prompt_like: true,
            screen_tmux_like: false,
            screen_ssh_disconnected: false,
            remote_host: None,
            remote_host_confidence: None,
            remote_host_source: None,
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: Vec::new(),
            last_verified_tmux_session: None,
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: Vec::new(),
            warnings: Vec::new(),
        };
        let tmux_runtime = PaneRuntimeState {
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
            shell_metadata_fresh: false,
            screen_prompt_like: false,
            screen_tmux_like: true,
            screen_ssh_disconnected: false,
            remote_host: None,
            remote_host_confidence: None,
            remote_host_source: None,
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: vec![PaneRuntimeScope {
                kind: PaneScopeKind::Multiplexer,
                label: Some("work".to_string()),
                host: None,
                confidence: PaneConfidence::Advisory,
                evidence_source: PaneEvidenceSource::ShellProbe,
            }],
            last_verified_tmux_session: Some("work".to_string()),
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: Vec::new(),
            warnings: Vec::new(),
        };

        assert!(direct_terminal_exec_is_safe(&shell_runtime));
        assert!(!direct_terminal_exec_is_safe(&tmux_runtime));
    }

    fn make_shell_context() -> TerminalContext {
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::ShellPrompt,
            mode: PaneMode::Shell,
            shell_metadata_fresh: true,
            screen_prompt_like: true,
            screen_tmux_like: false,
            screen_ssh_disconnected: false,
            remote_host: None,
            remote_host_confidence: None,
            remote_host_source: None,
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: Vec::new(),
            last_verified_tmux_session: None,
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: Vec::new(),
            warnings: Vec::new(),
        };
        TerminalContext {
            focused_pane_index: 1,
            focused_pane_id: 1,
            focused_hostname: None,
            focused_hostname_confidence: None,
            focused_hostname_source: None,
            focused_title: None,
            focused_front_state: PaneFrontState::ShellPrompt,
            focused_pane_mode: PaneMode::Shell,
            focused_has_shell_integration: true,
            focused_shell_metadata_fresh: true,
            focused_observation_support: PaneObservationSupport::default(),
            focused_runtime_stack: Vec::new(),
            focused_last_verified_runtime_stack: Vec::new(),
            focused_runtime_warnings: Vec::new(),
            focused_control: PaneControlState::from_runtime(&runtime),
            focused_shell_context: None,
            focused_shell_context_fresh: false,
            focused_recent_actions: Vec::new(),
            focused_remote_workspace: None,
            cwd: Some("/home/user".to_string()),
            recent_output: Vec::new(),
            focused_screen_hints: Vec::new(),
            focused_tmux_snapshot: None,
            last_command: None,
            last_exit_code: None,
            last_command_duration_secs: None,
            git_branch: None,
            ssh_host: None,
            tmux_session: None,
            agents_md: None,
            skills: Vec::new(),
            command_history: Vec::new(),
            other_panes: Vec::new(),
            git_diff: None,
            project_structure: None,
        }
    }

    #[test]
    fn tui_guide_absent_for_plain_shell() {
        let ctx = make_shell_context();
        let prompt = ctx.to_system_prompt();
        assert!(
            !prompt.contains("<tui_interaction_guide>"),
            "TUI guide should not appear for plain shell pane"
        );
    }

    #[test]
    fn tui_guide_emitted_for_tmux_focused_pane() {
        let mut ctx = make_shell_context();
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
            shell_metadata_fresh: false,
            screen_prompt_like: false,
            screen_tmux_like: true,
            screen_ssh_disconnected: false,
            remote_host: None,
            remote_host_confidence: None,
            remote_host_source: None,
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: vec![
                PaneRuntimeScope {
                    kind: PaneScopeKind::RemoteShell,
                    label: Some("haswell".to_string()),
                    host: Some("haswell".to_string()),
                    confidence: PaneConfidence::Advisory,
                    evidence_source: PaneEvidenceSource::ShellProbe,
                },
                PaneRuntimeScope {
                    kind: PaneScopeKind::Multiplexer,
                    label: Some("work".to_string()),
                    host: Some("haswell".to_string()),
                    confidence: PaneConfidence::Advisory,
                    evidence_source: PaneEvidenceSource::ShellProbe,
                },
                PaneRuntimeScope {
                    kind: PaneScopeKind::Shell,
                    label: None,
                    host: Some("haswell".to_string()),
                    confidence: PaneConfidence::Advisory,
                    evidence_source: PaneEvidenceSource::ShellProbe,
                },
            ],
            last_verified_tmux_session: Some("work".to_string()),
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: Vec::new(),
            warnings: Vec::new(),
        };
        ctx.focused_front_state = PaneFrontState::Unknown;
        ctx.focused_pane_mode = PaneMode::Unknown;
        ctx.focused_shell_metadata_fresh = false;
        ctx.focused_last_verified_runtime_stack = runtime.last_verified_scope_stack.clone();
        ctx.focused_control = PaneControlState::from_runtime(&runtime);
        let prompt = ctx.to_system_prompt();
        assert!(
            prompt.contains("<tui_interaction_guide>"),
            "TUI guide should appear when focused pane is tmux"
        );
        assert!(
            prompt.contains("tmux prefix"),
            "Tmux playbook should be included"
        );
        assert!(
            prompt.contains("prefer tmux-native")
                || prompt.contains("Prefer tmux-native")
                || prompt.contains("tmux native control"),
            "Prompt should prefer tmux-native tools when tmux is involved"
        );
        assert!(
            prompt.contains("Verify-after-act"),
            "Verify-after-act should always be included"
        );
    }

    #[test]
    fn derive_screen_hints_marks_visible_prompt_and_htop_as_observations() {
        let hints = derive_screen_hints(
            None,
            &[
                "Tasks: 105, 738 thr, 692 kthr; 1 running".to_string(),
                "Load average: 0.08 0.09 0.06".to_string(),
                "  PID USER      PRI  NI  VIRT   RES   SHR S CPU% MEM%   TIME+  Command"
                    .to_string(),
                ") htop".to_string(),
            ],
        );

        assert!(
            hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::HtopLikeScreen)
        );
        assert!(
            hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::PromptLikeInput)
        );
        assert!(
            hints
                .iter()
                .all(|hint| hint.confidence == PaneConfidence::Advisory)
        );
    }

    #[test]
    fn derive_screen_hints_marks_tmux_like_titles_as_observations() {
        let hints = derive_screen_hints(
            Some("haswell ❐ 0 ● 4 zsh"),
            &["~".to_string(), "❯".to_string()],
        );

        assert!(
            hints
                .iter()
                .any(|hint| hint.kind == PaneObservationHintKind::TmuxLikeScreen)
        );
    }

    #[test]
    fn remote_workspace_anchor_uses_ssh_history_for_prompt_like_remote_shells() {
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
            shell_metadata_fresh: false,
            screen_prompt_like: true,
            screen_tmux_like: false,
            screen_ssh_disconnected: false,
            remote_host: None,
            remote_host_confidence: None,
            remote_host_source: None,
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: Vec::new(),
            last_verified_tmux_session: None,
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: vec![PaneActionRecord {
                sequence: 1,
                kind: PaneActionKind::PaneCreated,
                summary: "con created this pane with startup command `ssh haswell`".to_string(),
                command: Some("ssh haswell".to_string()),
                source: PaneEvidenceSource::ActionHistory,
                confidence: PaneConfidence::Advisory,
                input_generation: None,
                note: None,
            }],
            warnings: Vec::new(),
        };
        let observation = PaneObservationFrame {
            title: Some("ssh haswell".to_string()),
            cwd: None,
            recent_output: vec![">".to_string()],
            screen_hints: vec![PaneObservationHint {
                kind: PaneObservationHintKind::PromptLikeInput,
                confidence: PaneConfidence::Advisory,
                detail: "Prompt-like input is visible near the bottom.".to_string(),
            }],
            last_command: None,
            last_exit_code: None,
            last_command_duration_secs: None,
            support: PaneObservationSupport::default(),
            has_shell_integration: false,
            is_alt_screen: false,
            is_busy: false,
            input_generation: 1,
            last_command_finished_input_generation: 0,
        };

        let anchor = remote_workspace_anchor(&runtime, &observation).expect("anchor");
        assert_eq!(anchor.host, "haswell");
        assert_eq!(anchor.source, PaneEvidenceSource::ActionHistory);
        assert_eq!(anchor.confidence, PaneConfidence::Advisory);
    }

    #[test]
    fn prompt_emits_tmux_snapshot_when_attached() {
        let mut ctx = make_shell_context();
        ctx.focused_tmux_snapshot = Some(TmuxSnapshot {
            panes: vec![TmuxPaneInfo {
                session_name: "work".to_string(),
                window_id: "@4".to_string(),
                window_index: "4".to_string(),
                window_name: "shell".to_string(),
                window_target: "work:4".to_string(),
                pane_id: "%17".to_string(),
                pane_index: "0".to_string(),
                target: "work:4.0".to_string(),
                pane_active: true,
                window_active: true,
                pane_current_command: Some("zsh".to_string()),
                pane_current_path: Some("/home/w/repo".to_string()),
            }],
        });
        let prompt = ctx.to_system_prompt();
        assert!(prompt.contains("<tmux_snapshot>"));
        assert!(prompt.contains("pane_id=\"%17\""));
        assert!(prompt.contains("current_command=\"zsh\""));
    }

    #[test]
    fn prompt_emits_multi_pane_layout_summary() {
        let mut ctx = make_shell_context();
        ctx.focused_screen_hints = vec![PaneObservationHint {
            kind: PaneObservationHintKind::PromptLikeInput,
            confidence: PaneConfidence::Advisory,
            detail: "Prompt-like input is visible near the bottom.".to_string(),
        }];
        ctx.other_panes.push(PaneSummary {
            pane_index: 2,
            pane_id: 2,
            hostname: Some("haswell".to_string()),
            hostname_confidence: Some(PaneConfidence::Strong),
            hostname_source: Some(PaneEvidenceSource::ShellProbe),
            remote_workspace: None,
            title: Some("ssh haswell".to_string()),
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
            has_shell_integration: false,
            shell_metadata_fresh: false,
            observation_support: PaneObservationSupport::default(),
            control: PaneControlState::from_runtime(&PaneRuntimeState {
                front_state: PaneFrontState::Unknown,
                mode: PaneMode::Unknown,
                shell_metadata_fresh: false,
                screen_prompt_like: false,
                screen_tmux_like: true,
                screen_ssh_disconnected: false,
                remote_host: Some("haswell".to_string()),
                remote_host_confidence: Some(PaneConfidence::Strong),
                remote_host_source: Some(PaneEvidenceSource::ShellProbe),
                agent_cli: None,
                tmux_session: Some("work".to_string()),
                last_verified_scope_stack: vec![
                    PaneRuntimeScope {
                        kind: PaneScopeKind::RemoteShell,
                        label: Some("haswell".to_string()),
                        host: Some("haswell".to_string()),
                        confidence: PaneConfidence::Strong,
                        evidence_source: PaneEvidenceSource::ShellProbe,
                    },
                    PaneRuntimeScope {
                        kind: PaneScopeKind::Multiplexer,
                        label: Some("work".to_string()),
                        host: Some("haswell".to_string()),
                        confidence: PaneConfidence::Strong,
                        evidence_source: PaneEvidenceSource::ShellProbe,
                    },
                ],
                last_verified_tmux_session: Some("work".to_string()),
                shell_context: None,
                shell_context_fresh: false,
                active_scope: None,
                evidence: Vec::new(),
                scope_stack: Vec::new(),
                recent_actions: Vec::new(),
                warnings: Vec::new(),
            }),
            agent_cli: None,
            active_scope: None,
            evidence: Vec::new(),
            runtime_stack: Vec::new(),
            last_verified_runtime_stack: vec![
                PaneRuntimeScope {
                    kind: PaneScopeKind::RemoteShell,
                    label: Some("haswell".to_string()),
                    host: Some("haswell".to_string()),
                    confidence: PaneConfidence::Strong,
                    evidence_source: PaneEvidenceSource::ShellProbe,
                },
                PaneRuntimeScope {
                    kind: PaneScopeKind::Multiplexer,
                    label: Some("work".to_string()),
                    host: Some("haswell".to_string()),
                    confidence: PaneConfidence::Strong,
                    evidence_source: PaneEvidenceSource::ShellProbe,
                },
            ],
            runtime_warnings: Vec::new(),
            tmux_session: Some("work".to_string()),
            cwd: None,
            workspace_cwd_hint: None,
            workspace_agent_cli_hint: None,
            screen_hints: Vec::new(),
            last_command: None,
            last_exit_code: None,
            is_busy: false,
            recent_output: Vec::new(),
        });
        let prompt = ctx.to_system_prompt();
        assert!(prompt.contains("<pane_layout_summary>"));
        assert!(prompt.contains("pane 1 (id 1) is focused"));
        assert!(prompt.contains("pane 2"));
        assert!(prompt.contains("historical shell frame only"));
    }

    #[test]
    fn prompt_emits_work_target_hints_for_multi_pane_tabs() {
        let mut ctx = make_shell_context();
        ctx.focused_hostname = Some("cinnamon".to_string());
        ctx.other_panes.push(PaneSummary {
            pane_index: 2,
            pane_id: 2,
            hostname: Some("haswell".to_string()),
            hostname_confidence: Some(PaneConfidence::Strong),
            hostname_source: Some(PaneEvidenceSource::ShellProbe),
            remote_workspace: None,
            title: Some("ops".to_string()),
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
            has_shell_integration: false,
            shell_metadata_fresh: false,
            observation_support: PaneObservationSupport::default(),
            control: PaneControlState::from_runtime(&PaneRuntimeState {
                front_state: PaneFrontState::Unknown,
                mode: PaneMode::Unknown,
                shell_metadata_fresh: false,
                screen_prompt_like: true,
                screen_tmux_like: true,
                screen_ssh_disconnected: false,
                remote_host: Some("haswell".to_string()),
                remote_host_confidence: Some(PaneConfidence::Strong),
                remote_host_source: Some(PaneEvidenceSource::ShellProbe),
                agent_cli: None,
                tmux_session: Some("work".to_string()),
                last_verified_scope_stack: Vec::new(),
                last_verified_tmux_session: Some("work".to_string()),
                shell_context: None,
                shell_context_fresh: false,
                active_scope: None,
                evidence: Vec::new(),
                scope_stack: vec![PaneRuntimeScope {
                    kind: PaneScopeKind::Multiplexer,
                    label: Some("work".to_string()),
                    host: Some("haswell".to_string()),
                    confidence: PaneConfidence::Strong,
                    evidence_source: PaneEvidenceSource::ShellProbe,
                }],
                recent_actions: Vec::new(),
                warnings: Vec::new(),
            }),
            agent_cli: None,
            active_scope: None,
            evidence: Vec::new(),
            runtime_stack: vec![PaneRuntimeScope {
                kind: PaneScopeKind::Multiplexer,
                label: Some("work".to_string()),
                host: Some("haswell".to_string()),
                confidence: PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::ShellProbe,
            }],
            last_verified_runtime_stack: Vec::new(),
            runtime_warnings: Vec::new(),
            tmux_session: Some("work".to_string()),
            cwd: None,
            workspace_cwd_hint: None,
            workspace_agent_cli_hint: None,
            screen_hints: Vec::new(),
            last_command: None,
            last_exit_code: None,
            is_busy: false,
            recent_output: Vec::new(),
        });
        let prompt = ctx.to_system_prompt();
        assert!(prompt.contains("<work_target_hints>"));
        assert!(prompt.contains("Best visible remote shell target"));
        assert!(
            prompt.contains("Best tmux workspace target")
                || prompt.contains("tmux appears in pane 2")
        );
    }

    #[test]
    fn prompt_emits_work_target_hints_for_single_native_tmux_pane() {
        let mut ctx = make_shell_context();
        ctx.focused_control.tmux = Some(TmuxControlState {
            session_name: Some("work".to_string()),
            mode: TmuxControlMode::Native,
            front_target: None,
            reason: "native".to_string(),
        });
        ctx.focused_tmux_snapshot = Some(TmuxSnapshot {
            panes: vec![TmuxPaneInfo {
                session_name: "work".to_string(),
                window_id: "@1".to_string(),
                window_index: "1".to_string(),
                window_name: "agent".to_string(),
                window_target: "work:1".to_string(),
                pane_id: "%17".to_string(),
                pane_index: "0".to_string(),
                target: "work:1.0".to_string(),
                pane_active: true,
                window_active: true,
                pane_current_command: Some("codex".to_string()),
                pane_current_path: Some("/repo".to_string()),
            }],
        });

        let prompt = ctx.to_system_prompt();
        assert!(prompt.contains("<work_target_hints>"));
        assert!(prompt.contains("Focused tmux workspace already has an `codex` target"));
        assert!(prompt.contains("tmux_send_keys"));
    }
}
