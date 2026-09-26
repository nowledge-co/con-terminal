use serde::{Deserialize, Serialize};

use crate::context::{
    PaneFrontState, PaneMode, PaneRuntimeState, PaneScopeKind, tmux_session_from_action_record,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneAddressSpace {
    ConPane,
}

impl PaneAddressSpace {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConPane => "con_pane",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneVisibleTargetKind {
    ShellPrompt,
    RemoteShell,
    TmuxSession,
    AgentCli,
    InteractiveApp,
    Unknown,
}

impl PaneVisibleTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ShellPrompt => "shell_prompt",
            Self::RemoteShell => "remote_shell",
            Self::TmuxSession => "tmux_session",
            Self::AgentCli => "agent_cli",
            Self::InteractiveApp => "interactive_app",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneControlChannel {
    ReadScreen,
    SearchScrollback,
    RawInput,
    ShellProbe,
    VisibleShellExec,
    TmuxQuery,
    TmuxSendKeys,
    TmuxExec,
}

impl PaneControlChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadScreen => "read_screen",
            Self::SearchScrollback => "search_scrollback",
            Self::RawInput => "raw_input",
            Self::ShellProbe => "shell_probe",
            Self::VisibleShellExec => "visible_shell_exec",
            Self::TmuxQuery => "tmux_query",
            Self::TmuxSendKeys => "tmux_send_keys",
            Self::TmuxExec => "tmux_exec",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneControlCapability {
    ReadScreen,
    SearchScrollback,
    SendRawInput,
    ProbeShellContext,
    ExecVisibleShell,
    QueryTmux,
    SendTmuxKeys,
    ExecTmuxCommand,
}

impl PaneControlCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadScreen => "read_screen",
            Self::SearchScrollback => "search_scrollback",
            Self::SendRawInput => "send_raw_input",
            Self::ProbeShellContext => "probe_shell_context",
            Self::ExecVisibleShell => "exec_visible_shell",
            Self::QueryTmux => "query_tmux",
            Self::SendTmuxKeys => "send_tmux_keys",
            Self::ExecTmuxCommand => "exec_tmux_command",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneAttachmentKind {
    GhosttySurface,
    ShellPrompt,
    TmuxControl,
    NeovimRpc,
    OsPty,
}

impl PaneAttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GhosttySurface => "ghostty_surface",
            Self::ShellPrompt => "shell_prompt",
            Self::TmuxControl => "tmux_control",
            Self::NeovimRpc => "neovim_rpc",
            Self::OsPty => "os_pty",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneAttachmentAuthority {
    BackendFact,
    ProtocolFact,
    ShellProbe,
    ActionHistory,
}

impl PaneAttachmentAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BackendFact => "backend_fact",
            Self::ProtocolFact => "protocol_fact",
            Self::ShellProbe => "shell_probe",
            Self::ActionHistory => "action_history",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneAttachmentTransport {
    EmbeddedSurface,
    VisibleShell,
    TmuxProtocol,
    MsgpackRpc,
    ProcessInspection,
}

impl PaneAttachmentTransport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EmbeddedSurface => "embedded_surface",
            Self::VisibleShell => "visible_shell",
            Self::TmuxProtocol => "tmux_protocol",
            Self::MsgpackRpc => "msgpack_rpc",
            Self::ProcessInspection => "process_inspection",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneProtocolAttachment {
    pub id: String,
    pub kind: PaneAttachmentKind,
    pub authority: PaneAttachmentAuthority,
    pub transport: PaneAttachmentTransport,
    pub label: Option<String>,
    pub capabilities: Vec<PaneControlCapability>,
    pub note: Option<String>,
}

impl PaneProtocolAttachment {
    pub fn summary(&self) -> String {
        match self.label.as_deref() {
            Some(label) => format!("{}({label})", self.kind.as_str()),
            None => self.kind.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TmuxControlMode {
    Unavailable,
    InspectOnly,
    Native,
}

impl TmuxControlMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::InspectOnly => "inspect_only",
            Self::Native => "native",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneVisibleTarget {
    pub kind: PaneVisibleTargetKind,
    pub label: Option<String>,
    pub host: Option<String>,
}

impl PaneVisibleTarget {
    pub fn summary(&self) -> String {
        match (self.kind, self.label.as_deref(), self.host.as_deref()) {
            (PaneVisibleTargetKind::ShellPrompt, _, Some(host)) => {
                format!("shell_prompt({host})")
            }
            (PaneVisibleTargetKind::RemoteShell, _, Some(host)) => {
                format!("remote_shell({host})")
            }
            (_, Some(label), _) => format!("{}({label})", self.kind.as_str()),
            _ => self.kind.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PaneControlState {
    pub address_space: PaneAddressSpace,
    pub target_stack: Vec<PaneVisibleTarget>,
    pub visible_target: PaneVisibleTarget,
    pub attachments: Vec<PaneProtocolAttachment>,
    pub tmux: Option<TmuxControlState>,
    pub channels: Vec<PaneControlChannel>,
    pub capabilities: Vec<PaneControlCapability>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TmuxControlState {
    pub session_name: Option<String>,
    pub mode: TmuxControlMode,
    pub front_target: Option<PaneVisibleTarget>,
    pub reason: String,
}

impl PaneControlState {
    pub fn from_runtime(runtime: &PaneRuntimeState) -> Self {
        let target_stack = target_stack_from_runtime(runtime);
        let visible_target = target_stack
            .last()
            .cloned()
            .unwrap_or_else(|| fallback_visible_target(runtime));
        let attachments = attachments_from_runtime(runtime, &target_stack);
        let tmux = tmux_control_from_runtime(runtime, &target_stack);

        let mut channels = vec![
            PaneControlChannel::ReadScreen,
            PaneControlChannel::SearchScrollback,
            PaneControlChannel::RawInput,
        ];
        let mut capabilities = vec![
            PaneControlCapability::ReadScreen,
            PaneControlCapability::SearchScrollback,
            PaneControlCapability::SendRawInput,
        ];
        let has_native_tmux_anchor = runtime_has_native_tmux_anchor(runtime);
        if runtime.front_state == PaneFrontState::ShellPrompt && runtime.shell_metadata_fresh {
            channels.push(PaneControlChannel::ShellProbe);
            channels.push(PaneControlChannel::VisibleShellExec);
            capabilities.push(PaneControlCapability::ProbeShellContext);
            capabilities.push(PaneControlCapability::ExecVisibleShell);
        }
        if has_native_tmux_anchor {
            channels.push(PaneControlChannel::TmuxQuery);
            channels.push(PaneControlChannel::TmuxSendKeys);
            channels.push(PaneControlChannel::TmuxExec);
            capabilities.push(PaneControlCapability::QueryTmux);
            capabilities.push(PaneControlCapability::SendTmuxKeys);
            capabilities.push(PaneControlCapability::ExecTmuxCommand);
        }

        let mut notes = Vec::new();
        if !runtime.shell_metadata_fresh {
            notes.push(
                "Visible shell prompt is not confirmed. cwd and last_command may describe an earlier shell frame, not the current foreground target.".to_string(),
            );
        }
        if let Some(host) = &runtime.remote_host
            && visible_target.kind == PaneVisibleTargetKind::ShellPrompt
        {
            notes.push(format!(
                "Visible shell execution in this pane will run on host `{host}`."
            ));
        }
        if runtime.shell_context_fresh {
            if let Some(tmux) = runtime
                .shell_context
                .as_ref()
                .and_then(|context| context.tmux.as_ref())
            {
                let session = tmux
                    .session_name
                    .clone()
                    .unwrap_or_else(|| "tmux".to_string());
                let pane = tmux
                    .pane_id
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                notes.push(format!(
                    "Fresh shell probe confirmed that the visible shell prompt is inside tmux session `{session}` pane `{pane}`."
                ));
                notes.push(
                    "This pane now has a same-session tmux control anchor. Prefer tmux-native query/capture/run/send tools over raw outer-pane input when operating inside tmux."
                        .to_string(),
                );
            }
        } else if has_native_tmux_anchor && runtime.screen_prompt_like {
            let session = tmux_session_hint(runtime).unwrap_or_else(|| "tmux".to_string());
            if runtime.screen_tmux_like {
                notes.push(format!(
                    "The current screen still looks like a prompt inside tmux session `{session}`, and recent con-caused tmux setup keeps a durable same-session tmux shell anchor alive."
                ));
            } else {
                notes.push(format!(
                    "The current screen still looks like a prompt on the shell that prepared tmux session `{session}`, and recent con-caused tmux setup keeps a durable same-session tmux control anchor alive."
                ));
            }
            notes.push(
                "Prefer tmux-native query/capture/run/send tools over raw tmux shell commands or outer-pane keystrokes while that prompt-like tmux shell remains visible."
                    .to_string(),
            );
        } else if has_native_tmux_anchor {
            let session = tmux_session_hint(runtime).unwrap_or_else(|| "tmux".to_string());
            notes.push(format!(
                "A recent con-executed tmux command targeted session `{session}` from the current fresh shell prompt."
            ));
            notes.push(
                "This pane can use that fresh shell prompt as a tmux control anchor. Prefer tmux-native query/capture/run/send tools over raw outer-pane input while the prompt remains fresh."
                    .to_string(),
            );
        }
        if target_stack.len() > 1 {
            notes.push(format!(
                "Nested visible target stack: {}.",
                format_target_stack(&target_stack)
            ));
        }
        if !runtime.last_verified_scope_stack.is_empty() {
            notes.push(format!(
                "Last verified shell frame: {}.",
                runtime
                    .last_verified_scope_stack
                    .iter()
                    .map(|scope| scope.summary())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ));
        }
        if let Some(action) = runtime.recent_actions.last() {
            notes.push(format!("Recent con action: {}.", action.summary));
        }

        match visible_target.kind {
            PaneVisibleTargetKind::TmuxSession => notes.push(
                "This pane shows tmux. `pane_index` addresses the outer con pane only, not a tmux window or tmux pane."
                    .to_string(),
            ),
            PaneVisibleTargetKind::AgentCli => {
                let label = visible_target.label.as_deref().unwrap_or("agent_cli");
                notes.push(format!(
                    "The visible target is `{label}`. Raw input affects that agent UI, not a shell prompt."
                ));
                match label.to_ascii_lowercase().as_str() {
                    "codex" => notes.push(
                        "Codex has a separate app-server mode, but con only has direct Codex-native control when an explicit app-server attachment is proven. Otherwise treat it as an interactive terminal target."
                            .to_string(),
                    ),
                    "opencode" | "open-code" => notes.push(
                        "OpenCode has a separate server mode, but con only has direct OpenCode-native control when an explicit server attachment is proven. Otherwise treat it as an interactive terminal target."
                            .to_string(),
                    ),
                    _ => {}
                }
                if target_stack
                    .iter()
                    .any(|target| target.kind == PaneVisibleTargetKind::TmuxSession)
                {
                    notes.push(
                        "That agent CLI is nested inside tmux. con still does not have tmux-native pane targeting here."
                            .to_string(),
                    );
                }
            }
            PaneVisibleTargetKind::InteractiveApp => notes.push(
                "This pane shows a proven interactive app. con can inspect screen state and send raw input, but it does not have app-native control for this target yet."
                    .to_string(),
            ),
            PaneVisibleTargetKind::Unknown => notes.push(
                "The visible foreground target is not proven. Inspect the pane before sending raw input, and do not assume a shell prompt or a TUI-specific control channel."
                    .to_string(),
            ),
            PaneVisibleTargetKind::ShellPrompt | PaneVisibleTargetKind::RemoteShell => {}
        }

        Self {
            address_space: PaneAddressSpace::ConPane,
            target_stack,
            visible_target,
            attachments,
            tmux,
            channels,
            capabilities,
            notes,
        }
    }

    pub fn allows_visible_shell_exec(&self) -> bool {
        self.capabilities
            .contains(&PaneControlCapability::ExecVisibleShell)
    }

    pub fn allows_shell_probe(&self) -> bool {
        self.capabilities
            .contains(&PaneControlCapability::ProbeShellContext)
    }
}

pub fn format_target_stack(targets: &[PaneVisibleTarget]) -> String {
    if targets.is_empty() {
        "unknown".to_string()
    } else {
        targets
            .iter()
            .map(PaneVisibleTarget::summary)
            .collect::<Vec<_>>()
            .join(" -> ")
    }
}

pub fn format_control_attachments(attachments: &[PaneProtocolAttachment]) -> String {
    if attachments.is_empty() {
        "none".to_string()
    } else {
        attachments
            .iter()
            .map(PaneProtocolAttachment::summary)
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn attachments_from_runtime(
    runtime: &PaneRuntimeState,
    target_stack: &[PaneVisibleTarget],
) -> Vec<PaneProtocolAttachment> {
    let mut attachments = vec![PaneProtocolAttachment {
        id: "ghostty_surface".to_string(),
        kind: PaneAttachmentKind::GhosttySurface,
        authority: PaneAttachmentAuthority::BackendFact,
        transport: PaneAttachmentTransport::EmbeddedSurface,
        label: None,
        capabilities: vec![
            PaneControlCapability::ReadScreen,
            PaneControlCapability::SearchScrollback,
            PaneControlCapability::SendRawInput,
        ],
        note: Some(
            "Ghostty surface attachment provides screen reads, scrollback search, and raw visible input."
                .to_string(),
        ),
    }];

    if runtime.front_state == PaneFrontState::ShellPrompt && runtime.shell_metadata_fresh {
        attachments.push(PaneProtocolAttachment {
            id: "shell_prompt".to_string(),
            kind: PaneAttachmentKind::ShellPrompt,
            authority: PaneAttachmentAuthority::BackendFact,
            transport: PaneAttachmentTransport::VisibleShell,
            label: target_stack.last().and_then(|target| target.host.clone()),
            capabilities: vec![
                PaneControlCapability::ProbeShellContext,
                PaneControlCapability::ExecVisibleShell,
            ],
            note: Some(
                "Fresh shell prompt is proven in the visible pane. Read-only shell probes and visible shell execution are available."
                    .to_string(),
            ),
        });
    }

    if runtime_has_native_tmux_anchor(runtime)
        && let Some(session_name) = tmux_session_hint(runtime)
    {
        let (authority, note) = if runtime.shell_context_fresh
            && runtime
                .shell_context
                .as_ref()
                .and_then(|context| context.tmux.as_ref())
                .is_some()
        {
            (
                    PaneAttachmentAuthority::ShellProbe,
                    "A fresh shell probe confirmed a same-session tmux shell anchor. con can query tmux state, create tmux targets, and send tmux-native keys through that shell."
                        .to_string(),
                )
        } else if runtime.screen_prompt_like && runtime.screen_tmux_like {
            (
                    PaneAttachmentAuthority::ActionHistory,
                    "A recent con-executed tmux command and the current prompt-like tmux screen keep a durable same-session tmux control anchor alive."
                        .to_string(),
                )
        } else {
            (
                    PaneAttachmentAuthority::ActionHistory,
                    "A recent con-executed tmux command and the current prompt-like shell keep a durable same-session tmux control anchor alive."
                        .to_string(),
                )
        };
        attachments.push(PaneProtocolAttachment {
            id: "tmux_control".to_string(),
            kind: PaneAttachmentKind::TmuxControl,
            authority,
            transport: PaneAttachmentTransport::TmuxProtocol,
            label: Some(session_name),
            capabilities: vec![
                PaneControlCapability::QueryTmux,
                PaneControlCapability::SendTmuxKeys,
                PaneControlCapability::ExecTmuxCommand,
            ],
            note: Some(note),
        });
    }

    attachments
}

fn tmux_control_from_runtime(
    runtime: &PaneRuntimeState,
    target_stack: &[PaneVisibleTarget],
) -> Option<TmuxControlState> {
    let has_native_tmux_anchor = runtime_has_native_tmux_anchor(runtime);
    let has_current_tmux = target_stack
        .iter()
        .any(|target| target.kind == PaneVisibleTargetKind::TmuxSession);
    let last_verified_stack = last_verified_target_stack(runtime);
    let has_historical_tmux = last_verified_stack
        .iter()
        .any(|target| target.kind == PaneVisibleTargetKind::TmuxSession);

    if !has_current_tmux && !has_historical_tmux && !has_native_tmux_anchor {
        return None;
    }

    let tmux_stack = if has_current_tmux {
        target_stack
    } else {
        &last_verified_stack
    };

    let tmux_index = tmux_stack
        .iter()
        .position(|target| target.kind == PaneVisibleTargetKind::TmuxSession);

    Some(TmuxControlState {
        session_name: tmux_session_hint(runtime).or_else(|| {
            tmux_index.and_then(|idx| tmux_stack.get(idx).and_then(|target| target.label.clone()))
        }),
        mode: if has_native_tmux_anchor {
            TmuxControlMode::Native
        } else {
            TmuxControlMode::InspectOnly
        },
        front_target: tmux_index.and_then(|idx| tmux_stack.get(idx + 1).cloned()),
        reason: if has_current_tmux
            && runtime.shell_context_fresh
            && runtime
                .shell_context
                .as_ref()
                .and_then(|context| context.tmux.as_ref())
                .is_some()
        {
            "con has a typed same-session tmux shell anchor for this pane. Native tmux query, capture, run-command, and send-keys are available through that anchor.".to_string()
        } else if has_native_tmux_anchor && runtime.screen_prompt_like {
            if runtime.screen_tmux_like {
                "The current screen still looks like a prompt inside tmux, and recent Con-caused tmux setup from this pane keeps a durable same-session tmux shell anchor alive. Native tmux query, capture, run-command, and send-keys remain available through that prompt-like tmux shell.".to_string()
            } else {
                "The current screen still looks like a prompt on the shell that prepared tmux, and recent Con-caused tmux setup from this pane keeps a durable same-session tmux control anchor alive. Native tmux query, capture, run-command, and send-keys remain available through that prompt-like shell.".to_string()
            }
        } else if has_native_tmux_anchor {
            "A recent con-executed tmux command and the current fresh shell prompt give con a native tmux control anchor for this session, even though tmux is not yet the proven front-most target.".to_string()
        } else if has_current_tmux {
            "con can identify a current tmux layer in this pane, but it does not yet have a same-session tmux control channel. Native tmux pane/window targeting and tmux command execution are unavailable.".to_string()
        } else {
            "The last verified shell frame for this pane was inside tmux, but the current foreground target is not proven. Native tmux pane/window targeting and tmux command execution remain unavailable.".to_string()
        },
    })
}

fn tmux_session_hint(runtime: &PaneRuntimeState) -> Option<String> {
    runtime
        .shell_context
        .as_ref()
        .and_then(|context| context.tmux.as_ref())
        .and_then(|tmux| tmux.session_name.clone())
        .or_else(|| runtime.tmux_session.clone())
        .or_else(|| runtime.last_verified_tmux_session.clone())
        .or_else(|| {
            runtime
                .recent_actions
                .iter()
                .rev()
                .find_map(tmux_session_from_action_record)
        })
}

fn runtime_has_native_tmux_anchor(runtime: &PaneRuntimeState) -> bool {
    if runtime.shell_context_fresh
        && runtime
            .shell_context
            .as_ref()
            .and_then(|context| context.tmux.as_ref())
            .is_some()
    {
        return true;
    }

    let recent_tmux_session = runtime
        .recent_actions
        .iter()
        .rev()
        .find_map(tmux_session_from_action_record)
        .is_some();

    if runtime.front_state == PaneFrontState::ShellPrompt && runtime.shell_metadata_fresh {
        return recent_tmux_session;
    }

    if runtime.screen_ssh_disconnected || !runtime.screen_prompt_like {
        return false;
    }

    recent_tmux_session
        || runtime.tmux_session.is_some()
        || runtime.last_verified_tmux_session.is_some()
}

fn target_stack_from_runtime(runtime: &PaneRuntimeState) -> Vec<PaneVisibleTarget> {
    let mut stack = Vec::new();

    for scope in &runtime.scope_stack {
        let target = match scope.kind {
            PaneScopeKind::Shell => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::ShellPrompt,
                label: scope.label.clone(),
                host: runtime.remote_host.clone(),
            },
            PaneScopeKind::RemoteShell => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::RemoteShell,
                label: scope.label.clone(),
                host: scope.host.clone().or_else(|| runtime.remote_host.clone()),
            },
            PaneScopeKind::Multiplexer => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::TmuxSession,
                label: scope
                    .label
                    .clone()
                    .or_else(|| runtime.tmux_session.clone())
                    .or_else(|| Some("tmux".to_string())),
                host: runtime.remote_host.clone(),
            },
            PaneScopeKind::AgentCli => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::AgentCli,
                label: scope.label.clone().or_else(|| runtime.agent_cli.clone()),
                host: runtime.remote_host.clone(),
            },
            PaneScopeKind::InteractiveApp => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::InteractiveApp,
                label: scope.label.clone(),
                host: runtime.remote_host.clone(),
            },
        };

        if stack.last() != Some(&target) {
            stack.push(target);
        }
    }

    if stack.is_empty() {
        stack.push(fallback_visible_target(runtime));
    }

    stack
}

fn last_verified_target_stack(runtime: &PaneRuntimeState) -> Vec<PaneVisibleTarget> {
    let mut stack = Vec::new();

    for scope in &runtime.last_verified_scope_stack {
        let target = match scope.kind {
            PaneScopeKind::Shell => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::ShellPrompt,
                label: scope.label.clone(),
                host: scope.host.clone(),
            },
            PaneScopeKind::RemoteShell => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::RemoteShell,
                label: scope.label.clone(),
                host: scope.host.clone(),
            },
            PaneScopeKind::Multiplexer => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::TmuxSession,
                label: scope
                    .label
                    .clone()
                    .or_else(|| runtime.last_verified_tmux_session.clone())
                    .or_else(|| Some("tmux".to_string())),
                host: scope.host.clone(),
            },
            PaneScopeKind::AgentCli => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::AgentCli,
                label: scope.label.clone(),
                host: scope.host.clone(),
            },
            PaneScopeKind::InteractiveApp => PaneVisibleTarget {
                kind: PaneVisibleTargetKind::InteractiveApp,
                label: scope.label.clone(),
                host: scope.host.clone(),
            },
        };

        if stack.last() != Some(&target) {
            stack.push(target);
        }
    }

    stack
}

fn fallback_visible_target(runtime: &PaneRuntimeState) -> PaneVisibleTarget {
    match runtime.mode {
        PaneMode::Shell => PaneVisibleTarget {
            kind: PaneVisibleTargetKind::ShellPrompt,
            label: runtime
                .active_scope
                .as_ref()
                .and_then(|scope| scope.label.clone()),
            host: runtime.remote_host.clone(),
        },
        PaneMode::Multiplexer => PaneVisibleTarget {
            kind: PaneVisibleTargetKind::TmuxSession,
            label: runtime.tmux_session.clone().or_else(|| {
                runtime
                    .active_scope
                    .as_ref()
                    .and_then(|scope| scope.label.clone())
                    .or_else(|| Some("tmux".to_string()))
            }),
            host: runtime.remote_host.clone(),
        },
        PaneMode::Tui => {
            if let Some(agent_cli) = &runtime.agent_cli {
                PaneVisibleTarget {
                    kind: PaneVisibleTargetKind::AgentCli,
                    label: Some(agent_cli.clone()),
                    host: runtime.remote_host.clone(),
                }
            } else if runtime
                .active_scope
                .as_ref()
                .is_some_and(|scope| scope.kind == PaneScopeKind::InteractiveApp)
            {
                PaneVisibleTarget {
                    kind: PaneVisibleTargetKind::InteractiveApp,
                    label: runtime
                        .active_scope
                        .as_ref()
                        .and_then(|scope| scope.label.clone()),
                    host: runtime.remote_host.clone(),
                }
            } else {
                PaneVisibleTarget {
                    kind: PaneVisibleTargetKind::InteractiveApp,
                    label: runtime
                        .active_scope
                        .as_ref()
                        .and_then(|scope| scope.label.clone()),
                    host: runtime.remote_host.clone(),
                }
            }
        }
        PaneMode::Unknown => PaneVisibleTarget {
            kind: PaneVisibleTargetKind::Unknown,
            label: runtime
                .active_scope
                .as_ref()
                .and_then(|scope| scope.label.clone()),
            host: runtime.remote_host.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PaneAttachmentKind, PaneControlCapability, PaneControlState, PaneVisibleTargetKind,
        TmuxControlMode, fallback_visible_target, format_control_attachments, format_target_stack,
    };
    use crate::context::{
        PaneActionKind, PaneActionRecord, PaneConfidence, PaneEvidenceSource, PaneFrontState,
        PaneMode, PaneRuntimeScope, PaneRuntimeState, PaneScopeKind,
    };

    #[test]
    fn control_state_marks_tmux_as_raw_input_only() {
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
            last_verified_scope_stack: vec![PaneRuntimeScope {
                kind: PaneScopeKind::Multiplexer,
                label: Some("work".to_string()),
                host: None,
                confidence: crate::context::PaneConfidence::Advisory,
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

        let control = PaneControlState::from_runtime(&runtime);
        assert_eq!(control.visible_target.kind, PaneVisibleTargetKind::Unknown);
        assert!(!control.allows_visible_shell_exec());
        assert!(
            !control
                .capabilities
                .contains(&PaneControlCapability::ExecVisibleShell)
        );
    }

    #[test]
    fn control_state_marks_fresh_shell_as_visible_exec_capable() {
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
            active_scope: Some(PaneRuntimeScope {
                kind: PaneScopeKind::Shell,
                label: None,
                host: None,
                confidence: crate::context::PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::ShellIntegration,
            }),
            evidence: Vec::new(),
            scope_stack: vec![PaneRuntimeScope {
                kind: PaneScopeKind::Shell,
                label: None,
                host: None,
                confidence: crate::context::PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::ShellIntegration,
            }],
            recent_actions: Vec::new(),
            warnings: Vec::new(),
        };

        let control = PaneControlState::from_runtime(&runtime);
        assert_eq!(
            fallback_visible_target(&runtime).kind,
            PaneVisibleTargetKind::ShellPrompt
        );
        assert!(control.allows_visible_shell_exec());
        assert!(control.allows_shell_probe());
        assert!(
            control
                .capabilities
                .contains(&PaneControlCapability::ProbeShellContext)
        );
        assert!(
            control
                .attachments
                .iter()
                .any(|attachment| attachment.kind == PaneAttachmentKind::ShellPrompt)
        );
    }

    #[test]
    fn control_state_preserves_nested_tmux_and_agent_cli_stack() {
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
                    kind: PaneScopeKind::Multiplexer,
                    label: Some("work".to_string()),
                    host: None,
                    confidence: crate::context::PaneConfidence::Advisory,
                    evidence_source: PaneEvidenceSource::ShellProbe,
                },
                PaneRuntimeScope {
                    kind: PaneScopeKind::Shell,
                    label: None,
                    host: None,
                    confidence: crate::context::PaneConfidence::Advisory,
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

        let control = PaneControlState::from_runtime(&runtime);
        assert_eq!(control.visible_target.kind, PaneVisibleTargetKind::Unknown);
        assert_eq!(format_target_stack(&control.target_stack), "unknown");
        assert_eq!(
            control.tmux.as_ref().map(|tmux| tmux.mode),
            Some(TmuxControlMode::InspectOnly)
        );
        assert_eq!(
            format_control_attachments(&control.attachments),
            "ghostty_surface"
        );
        assert_eq!(
            control
                .tmux
                .as_ref()
                .and_then(|tmux| tmux.front_target.as_ref())
                .map(|target| target.summary()),
            Some("shell_prompt".to_string())
        );
        assert!(
            control
                .notes
                .iter()
                .any(|note| note.contains("Last verified shell frame"))
        );
    }

    #[test]
    fn unknown_runtime_stays_unknown() {
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
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

        let control = PaneControlState::from_runtime(&runtime);
        assert_eq!(control.visible_target.kind, PaneVisibleTargetKind::Unknown);
        assert!(!control.allows_visible_shell_exec());
        assert_eq!(
            fallback_visible_target(&runtime).kind,
            PaneVisibleTargetKind::Unknown
        );
    }

    #[test]
    fn fresh_shell_with_recent_tmux_command_gets_native_tmux_anchor() {
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::ShellPrompt,
            mode: PaneMode::Shell,
            shell_metadata_fresh: true,
            screen_prompt_like: true,
            screen_tmux_like: false,
            screen_ssh_disconnected: false,
            remote_host: Some("haswell".to_string()),
            remote_host_confidence: Some(PaneConfidence::Advisory),
            remote_host_source: Some(PaneEvidenceSource::ActionHistory),
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: vec![PaneRuntimeScope {
                kind: PaneScopeKind::RemoteShell,
                label: None,
                host: Some("haswell".to_string()),
                confidence: PaneConfidence::Advisory,
                evidence_source: PaneEvidenceSource::ActionHistory,
            }],
            last_verified_tmux_session: Some("con-bench".to_string()),
            shell_context: None,
            shell_context_fresh: false,
            active_scope: Some(PaneRuntimeScope {
                kind: PaneScopeKind::Shell,
                label: None,
                host: Some("haswell".to_string()),
                confidence: PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::ShellIntegration,
            }),
            evidence: Vec::new(),
            scope_stack: vec![PaneRuntimeScope {
                kind: PaneScopeKind::Shell,
                label: None,
                host: Some("haswell".to_string()),
                confidence: PaneConfidence::Strong,
                evidence_source: PaneEvidenceSource::ShellIntegration,
            }],
            recent_actions: vec![PaneActionRecord {
                sequence: 1,
                kind: PaneActionKind::VisibleShellExec,
                summary: "con executed `tmux has-session -t con-bench 2>/dev/null || tmux new-session -d -s con-bench` in the visible shell".to_string(),
                command: Some(
                    "tmux has-session -t con-bench 2>/dev/null || tmux new-session -d -s con-bench"
                        .to_string(),
                ),
                source: PaneEvidenceSource::ActionHistory,
                confidence: PaneConfidence::Advisory,
                input_generation: Some(4),
                note: None,
            }],
            warnings: Vec::new(),
        };

        let control = PaneControlState::from_runtime(&runtime);
        assert!(
            control
                .capabilities
                .contains(&PaneControlCapability::QueryTmux)
        );
        assert_eq!(
            control.tmux.as_ref().map(|tmux| tmux.mode),
            Some(TmuxControlMode::Native)
        );
        assert_eq!(
            control
                .tmux
                .as_ref()
                .and_then(|tmux| tmux.session_name.as_deref()),
            Some("con-bench")
        );
        assert!(
            control
                .attachments
                .iter()
                .any(|attachment| attachment.kind == PaneAttachmentKind::TmuxControl)
        );
    }

    #[test]
    fn prompt_like_tmux_screen_retains_native_tmux_anchor_after_setup() {
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
            shell_metadata_fresh: false,
            screen_prompt_like: true,
            screen_tmux_like: true,
            screen_ssh_disconnected: false,
            remote_host: Some("haswell".to_string()),
            remote_host_confidence: Some(PaneConfidence::Advisory),
            remote_host_source: Some(PaneEvidenceSource::ActionHistory),
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: Vec::new(),
            last_verified_tmux_session: Some("con-bench".to_string()),
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: vec![PaneActionRecord {
                sequence: 1,
                kind: PaneActionKind::VisibleShellExec,
                summary: "con executed `tmux has-session -t con-bench 2>/dev/null || tmux new-session -d -s con-bench` in the visible shell".to_string(),
                command: Some(
                    "tmux has-session -t con-bench 2>/dev/null || tmux new-session -d -s con-bench"
                        .to_string(),
                ),
                source: PaneEvidenceSource::ActionHistory,
                confidence: PaneConfidence::Advisory,
                input_generation: Some(4),
                note: None,
            }],
            warnings: Vec::new(),
        };

        let control = PaneControlState::from_runtime(&runtime);
        assert_eq!(
            control.tmux.as_ref().map(|tmux| tmux.mode),
            Some(TmuxControlMode::Native)
        );
        assert!(
            control
                .capabilities
                .contains(&PaneControlCapability::QueryTmux)
        );
        assert!(
            control
                .notes
                .iter()
                .any(|note| note.contains("prompt inside tmux"))
        );
    }

    #[test]
    fn prompt_like_shell_retains_native_tmux_anchor_after_setup() {
        let runtime = PaneRuntimeState {
            front_state: PaneFrontState::Unknown,
            mode: PaneMode::Unknown,
            shell_metadata_fresh: false,
            screen_prompt_like: true,
            screen_tmux_like: false,
            screen_ssh_disconnected: false,
            remote_host: Some("haswell".to_string()),
            remote_host_confidence: Some(PaneConfidence::Advisory),
            remote_host_source: Some(PaneEvidenceSource::ActionHistory),
            agent_cli: None,
            tmux_session: None,
            last_verified_scope_stack: Vec::new(),
            last_verified_tmux_session: Some("con-bench".to_string()),
            shell_context: None,
            shell_context_fresh: false,
            active_scope: None,
            evidence: Vec::new(),
            scope_stack: Vec::new(),
            recent_actions: vec![PaneActionRecord {
                sequence: 1,
                kind: PaneActionKind::VisibleShellExec,
                summary: "con executed `tmux has-session -t con-bench 2>/dev/null || tmux new-session -d -s con-bench` in the visible shell".to_string(),
                command: Some(
                    "tmux has-session -t con-bench 2>/dev/null || tmux new-session -d -s con-bench"
                        .to_string(),
                ),
                source: PaneEvidenceSource::ActionHistory,
                confidence: PaneConfidence::Advisory,
                input_generation: Some(4),
                note: None,
            }],
            warnings: Vec::new(),
        };

        let control = PaneControlState::from_runtime(&runtime);
        assert_eq!(
            control.tmux.as_ref().map(|tmux| tmux.mode),
            Some(TmuxControlMode::Native)
        );
        assert!(
            control
                .capabilities
                .contains(&PaneControlCapability::QueryTmux)
        );
        assert!(
            control
                .notes
                .iter()
                .any(|note| note.contains("prompt on the shell"))
        );
        assert!(
            control
                .attachments
                .iter()
                .any(|attachment| attachment.kind == PaneAttachmentKind::TmuxControl)
        );
    }
}
