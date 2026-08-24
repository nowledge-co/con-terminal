use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

use con_agent::{PaneCreateLocation, TmuxExecLocation};

/// Default control endpoint location.
///
/// On Unix this is a filesystem-backed Unix domain socket. On Windows it is a
/// Named Pipe — the host portion `\\.\pipe\` is required by the Win32
/// `CreateNamedPipeW` API.
///
/// Debug builds intentionally use a distinct endpoint so `cargo run -p con`
/// can coexist with an installed production/beta Con without making startup
/// believe the dev process is a second window for the installed app. The
/// default can be overridden with the `CON_SOCKET_PATH` env var on every
/// platform.
#[cfg(all(unix, not(debug_assertions)))]
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/con.sock";
#[cfg(all(unix, debug_assertions))]
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/con-debug.sock";
#[cfg(all(windows, not(debug_assertions)))]
pub const DEFAULT_SOCKET_PATH: &str = r"\\.\pipe\con";
#[cfg(all(windows, debug_assertions))]
pub const DEFAULT_SOCKET_PATH: &str = r"\\.\pipe\con-debug";
#[cfg(all(not(any(unix, windows)), not(debug_assertions)))]
pub const DEFAULT_SOCKET_PATH: &str = "con.sock";
#[cfg(all(not(any(unix, windows)), debug_assertions))]
pub const DEFAULT_SOCKET_PATH: &str = "con-debug.sock";

pub const JSON_RPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PaneTarget {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
}

impl PaneTarget {
    pub const fn new(pane_index: Option<usize>, pane_id: Option<usize>) -> Self {
        Self {
            pane_index,
            pane_id,
        }
    }

    pub fn describe(self) -> String {
        match (self.pane_index, self.pane_id) {
            (Some(index), Some(id)) => format!("pane {index} (id {id})"),
            (Some(index), None) => format!("pane {index}"),
            (None, Some(id)) => format!("pane id {id}"),
            (None, None) => "focused pane".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SurfaceTarget {
    pub pane_index: Option<usize>,
    pub pane_id: Option<usize>,
    pub surface_id: Option<usize>,
}

impl SurfaceTarget {
    pub const fn new(
        pane_index: Option<usize>,
        pane_id: Option<usize>,
        surface_id: Option<usize>,
    ) -> Self {
        Self {
            pane_index,
            pane_id,
            surface_id,
        }
    }

    pub fn pane_target(self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }

    pub fn describe(self) -> String {
        match (self.surface_id, self.pane_index, self.pane_id) {
            (Some(surface_id), _, _) => format!("surface id {surface_id}"),
            (None, Some(index), Some(id)) => format!("active surface in pane {index} (id {id})"),
            (None, Some(index), None) => format!("active surface in pane {index}"),
            (None, None, Some(id)) => format!("active surface in pane id {id}"),
            (None, None, None) => "active surface in focused pane".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ControlMethodInfo {
    pub method: &'static str,
    pub description: &'static str,
}

const CONTROL_METHODS: &[(&str, &str)] = &[
    (
        "system.identify",
        "Return app identity, active tab, socket path, and method inventory.",
    ),
    (
        "system.capabilities",
        "List supported control-plane methods.",
    ),
    (
        "tabs.list",
        "List tabs and their conversation/session metadata.",
    ),
    ("tabs.new", "Create a new active tab."),
    (
        "tabs.close",
        "Close a tab by index or close the active tab.",
    ),
    (
        "panes.list",
        "List panes for a tab with control/runtime metadata.",
    ),
    ("panes.read", "Read recent content from a pane."),
    (
        "panes.exec",
        "Execute a visible shell command in a pane when allowed.",
    ),
    ("panes.send_keys", "Send raw keys to a pane."),
    ("panes.create", "Create a new pane split in a tab."),
    (
        "panes.wait",
        "Wait for a pane to become idle or match a pattern.",
    ),
    (
        "panes.probe_shell",
        "Run a read-only shell probe against a proven shell prompt.",
    ),
    (
        "tree.get",
        "Return the tab, pane, and pane-local surface tree.",
    ),
    (
        "surfaces.list",
        "List pane-local terminal surfaces without changing pane semantics.",
    ),
    (
        "surfaces.create",
        "Create a terminal surface inside an existing pane.",
    ),
    (
        "surfaces.split",
        "Create a split pane with an initial terminal surface.",
    ),
    ("surfaces.focus", "Focus a pane-local terminal surface."),
    ("surfaces.rename", "Rename a pane-local terminal surface."),
    ("surfaces.close", "Close a pane-local terminal surface."),
    (
        "surfaces.read",
        "Read recent content from a terminal surface.",
    ),
    (
        "surfaces.send_text",
        "Send text bytes to a terminal surface.",
    ),
    (
        "surfaces.send_key",
        "Send a named key to a terminal surface.",
    ),
    (
        "surfaces.wait_ready",
        "Wait for a terminal surface to become ready and return readiness metadata.",
    ),
    ("tmux.inspect", "Inspect tmux control state for a pane."),
    (
        "tmux.list",
        "List tmux panes/windows through a native tmux anchor.",
    ),
    ("tmux.capture", "Capture content from a tmux pane target."),
    ("tmux.send_keys", "Send keys to a tmux pane target."),
    ("tmux.run", "Launch a command via tmux."),
    (
        "agent.ask",
        "Send a prompt to a tab's built-in agent session and wait for the response.",
    ),
    (
        "agent.new_conversation",
        "Start a fresh built-in agent conversation for a tab.",
    ),
    (
        "agent.open_panel_for_request",
        "Open the agent panel via the agent-request path (drives motion state) and return panel state.",
    ),
    (
        "agent.panel_state",
        "Return the current agent panel open/visible state.",
    ),
];

pub fn control_methods() -> Vec<ControlMethodInfo> {
    CONTROL_METHODS
        .iter()
        .map(|(method, description)| ControlMethodInfo {
            method,
            description,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct SystemIdentifyResult {
    pub app: String,
    pub version: String,
    pub socket_path: String,
    pub active_tab_index: usize,
    pub tab_count: usize,
    pub methods: Vec<ControlMethodInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TabInfo {
    pub index: usize,
    pub title: String,
    pub is_active: bool,
    pub pane_count: usize,
    pub focused_pane_id: usize,
    pub needs_attention: bool,
    pub conversation_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentAskResult {
    pub tab_index: usize,
    pub conversation_id: String,
    pub prompt: String,
    pub message: con_agent::Message,
}

#[derive(Debug, Clone)]
pub enum ControlCommand {
    SystemIdentify,
    SystemCapabilities,
    TabsList,
    TabsNew,
    TabsClose {
        tab_index: Option<usize>,
    },
    PanesList {
        tab_index: Option<usize>,
    },
    PanesRead {
        tab_index: Option<usize>,
        target: PaneTarget,
        lines: usize,
    },
    PanesExec {
        tab_index: Option<usize>,
        target: PaneTarget,
        command: String,
    },
    PanesSendKeys {
        tab_index: Option<usize>,
        target: PaneTarget,
        keys: String,
    },
    PanesCreate {
        tab_index: Option<usize>,
        location: PaneCreateLocation,
        command: Option<String>,
    },
    PanesWait {
        tab_index: Option<usize>,
        target: PaneTarget,
        timeout_secs: Option<u64>,
        pattern: Option<String>,
    },
    PanesProbeShell {
        tab_index: Option<usize>,
        target: PaneTarget,
    },
    TreeGet {
        tab_index: Option<usize>,
    },
    SurfacesList {
        tab_index: Option<usize>,
        pane: PaneTarget,
    },
    SurfacesCreate {
        tab_index: Option<usize>,
        pane: PaneTarget,
        title: Option<String>,
        command: Option<String>,
        owner: Option<String>,
        close_pane_when_last: bool,
    },
    SurfacesSplit {
        tab_index: Option<usize>,
        source: SurfaceTarget,
        location: PaneCreateLocation,
        title: Option<String>,
        command: Option<String>,
        owner: Option<String>,
        close_pane_when_last: bool,
    },
    SurfacesFocus {
        tab_index: Option<usize>,
        target: SurfaceTarget,
    },
    SurfacesRename {
        tab_index: Option<usize>,
        target: SurfaceTarget,
        title: String,
    },
    SurfacesClose {
        tab_index: Option<usize>,
        target: SurfaceTarget,
        close_empty_owned_pane: bool,
    },
    SurfacesRead {
        tab_index: Option<usize>,
        target: SurfaceTarget,
        lines: usize,
    },
    SurfacesSendText {
        tab_index: Option<usize>,
        target: SurfaceTarget,
        text: String,
    },
    SurfacesSendKey {
        tab_index: Option<usize>,
        target: SurfaceTarget,
        key: String,
    },
    SurfacesWaitReady {
        tab_index: Option<usize>,
        target: SurfaceTarget,
        timeout_secs: Option<u64>,
    },
    TmuxInspect {
        tab_index: Option<usize>,
        target: PaneTarget,
    },
    TmuxList {
        tab_index: Option<usize>,
        target: PaneTarget,
    },
    TmuxCapture {
        tab_index: Option<usize>,
        pane: PaneTarget,
        target: Option<String>,
        lines: usize,
    },
    TmuxSendKeys {
        tab_index: Option<usize>,
        pane: PaneTarget,
        target: String,
        literal_text: Option<String>,
        key_names: Vec<String>,
        append_enter: bool,
    },
    TmuxRun {
        tab_index: Option<usize>,
        pane: PaneTarget,
        target: Option<String>,
        location: TmuxExecLocation,
        command: String,
        window_name: Option<String>,
        cwd: Option<String>,
        detached: bool,
    },
    AgentAsk {
        tab_index: Option<usize>,
        prompt: String,
        auto_approve_tools: bool,
        timeout_secs: Option<u64>,
    },
    AgentNewConversation {
        tab_index: Option<usize>,
    },
    /// Open the agent panel via the "Ask AI" / agent-request path (drives motion state).
    AgentOpenPanelForRequest {
        tab_index: Option<usize>,
    },
    /// Return the current agent panel open/visible state.
    AgentPanelState {
        tab_index: Option<usize>,
    },
}

impl ControlCommand {
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::SystemIdentify => "system.identify",
            Self::SystemCapabilities => "system.capabilities",
            Self::TabsList => "tabs.list",
            Self::TabsNew => "tabs.new",
            Self::TabsClose { .. } => "tabs.close",
            Self::PanesList { .. } => "panes.list",
            Self::PanesRead { .. } => "panes.read",
            Self::PanesExec { .. } => "panes.exec",
            Self::PanesSendKeys { .. } => "panes.send_keys",
            Self::PanesCreate { .. } => "panes.create",
            Self::PanesWait { .. } => "panes.wait",
            Self::PanesProbeShell { .. } => "panes.probe_shell",
            Self::TreeGet { .. } => "tree.get",
            Self::SurfacesList { .. } => "surfaces.list",
            Self::SurfacesCreate { .. } => "surfaces.create",
            Self::SurfacesSplit { .. } => "surfaces.split",
            Self::SurfacesFocus { .. } => "surfaces.focus",
            Self::SurfacesRename { .. } => "surfaces.rename",
            Self::SurfacesClose { .. } => "surfaces.close",
            Self::SurfacesRead { .. } => "surfaces.read",
            Self::SurfacesSendText { .. } => "surfaces.send_text",
            Self::SurfacesSendKey { .. } => "surfaces.send_key",
            Self::SurfacesWaitReady { .. } => "surfaces.wait_ready",
            Self::TmuxInspect { .. } => "tmux.inspect",
            Self::TmuxList { .. } => "tmux.list",
            Self::TmuxCapture { .. } => "tmux.capture",
            Self::TmuxSendKeys { .. } => "tmux.send_keys",
            Self::TmuxRun { .. } => "tmux.run",
            Self::AgentAsk { .. } => "agent.ask",
            Self::AgentNewConversation { .. } => "agent.new_conversation",
            Self::AgentOpenPanelForRequest { .. } => "agent.open_panel_for_request",
            Self::AgentPanelState { .. } => "agent.panel_state",
        }
    }

    pub fn params_json(&self) -> Value {
        match self {
            Self::SystemIdentify | Self::SystemCapabilities | Self::TabsList | Self::TabsNew => {
                json!({})
            }
            Self::TabsClose { tab_index } => json!({ "tab_index": tab_index }),
            Self::PanesList { tab_index } => json!({ "tab_index": tab_index }),
            Self::PanesRead {
                tab_index,
                target,
                lines,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "lines": lines,
            }),
            Self::PanesExec {
                tab_index,
                target,
                command,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "command": command,
            }),
            Self::PanesSendKeys {
                tab_index,
                target,
                keys,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "keys": keys,
            }),
            Self::PanesCreate {
                tab_index,
                location,
                command,
            } => json!({
                "tab_index": tab_index,
                "location": location,
                "command": command,
            }),
            Self::PanesWait {
                tab_index,
                target,
                timeout_secs,
                pattern,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "timeout_secs": timeout_secs,
                "pattern": pattern,
            }),
            Self::PanesProbeShell { tab_index, target }
            | Self::TmuxInspect { tab_index, target }
            | Self::TmuxList { tab_index, target } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
            }),
            Self::TreeGet { tab_index } => json!({ "tab_index": tab_index }),
            Self::SurfacesList { tab_index, pane } => json!({
                "tab_index": tab_index,
                "pane_index": pane.pane_index,
                "pane_id": pane.pane_id,
            }),
            Self::SurfacesCreate {
                tab_index,
                pane,
                title,
                command,
                owner,
                close_pane_when_last,
            } => json!({
                "tab_index": tab_index,
                "pane_index": pane.pane_index,
                "pane_id": pane.pane_id,
                "title": title,
                "command": command,
                "owner": owner,
                "close_pane_when_last": close_pane_when_last,
            }),
            Self::SurfacesSplit {
                tab_index,
                source,
                location,
                title,
                command,
                owner,
                close_pane_when_last,
            } => json!({
                "tab_index": tab_index,
                "pane_index": source.pane_index,
                "pane_id": source.pane_id,
                "surface_id": source.surface_id,
                "location": location,
                "title": title,
                "command": command,
                "owner": owner,
                "close_pane_when_last": close_pane_when_last,
            }),
            Self::SurfacesFocus { tab_index, target } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "surface_id": target.surface_id,
            }),
            Self::SurfacesWaitReady {
                tab_index,
                target,
                timeout_secs,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "surface_id": target.surface_id,
                "timeout_secs": timeout_secs,
            }),
            Self::SurfacesRename {
                tab_index,
                target,
                title,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "surface_id": target.surface_id,
                "title": title,
            }),
            Self::SurfacesClose {
                tab_index,
                target,
                close_empty_owned_pane,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "surface_id": target.surface_id,
                "close_empty_owned_pane": close_empty_owned_pane,
            }),
            Self::SurfacesRead {
                tab_index,
                target,
                lines,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "surface_id": target.surface_id,
                "lines": lines,
            }),
            Self::SurfacesSendText {
                tab_index,
                target,
                text,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "surface_id": target.surface_id,
                "text": text,
            }),
            Self::SurfacesSendKey {
                tab_index,
                target,
                key,
            } => json!({
                "tab_index": tab_index,
                "pane_index": target.pane_index,
                "pane_id": target.pane_id,
                "surface_id": target.surface_id,
                "key": key,
            }),
            Self::TmuxCapture {
                tab_index,
                pane,
                target,
                lines,
            } => json!({
                "tab_index": tab_index,
                "pane_index": pane.pane_index,
                "pane_id": pane.pane_id,
                "target": target,
                "lines": lines,
            }),
            Self::TmuxSendKeys {
                tab_index,
                pane,
                target,
                literal_text,
                key_names,
                append_enter,
            } => json!({
                "tab_index": tab_index,
                "pane_index": pane.pane_index,
                "pane_id": pane.pane_id,
                "target": target,
                "literal_text": literal_text,
                "key_names": key_names,
                "append_enter": append_enter,
            }),
            Self::TmuxRun {
                tab_index,
                pane,
                target,
                location,
                command,
                window_name,
                cwd,
                detached,
            } => json!({
                "tab_index": tab_index,
                "pane_index": pane.pane_index,
                "pane_id": pane.pane_id,
                "target": target,
                "location": location,
                "command": command,
                "window_name": window_name,
                "cwd": cwd,
                "detached": detached,
            }),
            Self::AgentAsk {
                tab_index,
                prompt,
                auto_approve_tools,
                timeout_secs,
            } => json!({
                "tab_index": tab_index,
                "prompt": prompt,
                "auto_approve_tools": auto_approve_tools,
                "timeout_secs": timeout_secs,
            }),
            Self::AgentNewConversation { tab_index } => json!({ "tab_index": tab_index }),
            Self::AgentOpenPanelForRequest { tab_index } => json!({ "tab_index": tab_index }),
            Self::AgentPanelState { tab_index } => json!({ "tab_index": tab_index }),
        }
    }

    pub fn from_rpc(method: &str, params: Value) -> Result<Self, ControlError> {
        match method {
            "system.identify" => Ok(Self::SystemIdentify),
            "system.capabilities" => Ok(Self::SystemCapabilities),
            "tabs.list" => Ok(Self::TabsList),
            "tabs.new" => Ok(Self::TabsNew),
            "tabs.close" => {
                let params: TabScopedParams = decode_params(params)?;
                Ok(Self::TabsClose {
                    tab_index: params.tab_index,
                })
            }
            "panes.list" => {
                let params: TabScopedParams = decode_params(params)?;
                Ok(Self::PanesList {
                    tab_index: params.tab_index,
                })
            }
            "panes.read" => {
                let params: PaneReadParams = decode_params(params)?;
                Ok(Self::PanesRead {
                    tab_index: params.tab_index,
                    target: params.target(),
                    lines: params.lines,
                })
            }
            "panes.exec" => {
                let params: PaneExecParams = decode_params(params)?;
                Ok(Self::PanesExec {
                    tab_index: params.tab_index,
                    target: params.target(),
                    command: params.command,
                })
            }
            "panes.send_keys" => {
                let params: PaneSendKeysParams = decode_params(params)?;
                Ok(Self::PanesSendKeys {
                    tab_index: params.tab_index,
                    target: params.target(),
                    keys: params.keys,
                })
            }
            "panes.create" => {
                let params: PaneCreateParams = decode_params(params)?;
                Ok(Self::PanesCreate {
                    tab_index: params.tab_index,
                    location: params.location,
                    command: params.command,
                })
            }
            "panes.wait" => {
                let params: PaneWaitParams = decode_params(params)?;
                Ok(Self::PanesWait {
                    tab_index: params.tab_index,
                    target: params.target(),
                    timeout_secs: params.timeout_secs,
                    pattern: params.pattern,
                })
            }
            "panes.probe_shell" => {
                let params: PaneTargetParams = decode_params(params)?;
                Ok(Self::PanesProbeShell {
                    tab_index: params.tab_index,
                    target: params.target(),
                })
            }
            "tree.get" => {
                let params: TabScopedParams = decode_params(params)?;
                Ok(Self::TreeGet {
                    tab_index: params.tab_index,
                })
            }
            "surfaces.list" => {
                let params: PaneTargetParams = decode_params(params)?;
                Ok(Self::SurfacesList {
                    tab_index: params.tab_index,
                    pane: params.target(),
                })
            }
            "surfaces.create" => {
                let params: SurfaceCreateParams = decode_params(params)?;
                Ok(Self::SurfacesCreate {
                    tab_index: params.tab_index,
                    pane: params.pane_target(),
                    title: params.title,
                    command: params.command,
                    owner: params.owner,
                    close_pane_when_last: params.close_pane_when_last,
                })
            }
            "surfaces.split" => {
                let params: SurfaceSplitParams = decode_params(params)?;
                Ok(Self::SurfacesSplit {
                    tab_index: params.tab_index,
                    source: params.surface_target(),
                    location: params.location,
                    title: params.title,
                    command: params.command,
                    owner: params.owner,
                    close_pane_when_last: params.close_pane_when_last,
                })
            }
            "surfaces.focus" => {
                let params: SurfaceTargetParams = decode_params(params)?;
                Ok(Self::SurfacesFocus {
                    tab_index: params.tab_index,
                    target: params.target(),
                })
            }
            "surfaces.rename" => {
                let params: SurfaceRenameParams = decode_params(params)?;
                Ok(Self::SurfacesRename {
                    tab_index: params.tab_index,
                    target: params.target(),
                    title: params.title,
                })
            }
            "surfaces.close" => {
                let params: SurfaceCloseParams = decode_params(params)?;
                Ok(Self::SurfacesClose {
                    tab_index: params.tab_index,
                    target: params.target(),
                    close_empty_owned_pane: params.close_empty_owned_pane,
                })
            }
            "surfaces.read" => {
                let params: SurfaceReadParams = decode_params(params)?;
                Ok(Self::SurfacesRead {
                    tab_index: params.tab_index,
                    target: params.target(),
                    lines: params.lines,
                })
            }
            "surfaces.send_text" => {
                let params: SurfaceSendTextParams = decode_params(params)?;
                Ok(Self::SurfacesSendText {
                    tab_index: params.tab_index,
                    target: params.target(),
                    text: params.text,
                })
            }
            "surfaces.send_key" => {
                let params: SurfaceSendKeyParams = decode_params(params)?;
                Ok(Self::SurfacesSendKey {
                    tab_index: params.tab_index,
                    target: params.target(),
                    key: params.key,
                })
            }
            "surfaces.wait_ready" => {
                let params: SurfaceWaitReadyParams = decode_params(params)?;
                Ok(Self::SurfacesWaitReady {
                    tab_index: params.tab_index,
                    target: params.target(),
                    timeout_secs: params.timeout_secs,
                })
            }
            "tmux.inspect" => {
                let params: PaneTargetParams = decode_params(params)?;
                Ok(Self::TmuxInspect {
                    tab_index: params.tab_index,
                    target: params.target(),
                })
            }
            "tmux.list" => {
                let params: PaneTargetParams = decode_params(params)?;
                Ok(Self::TmuxList {
                    tab_index: params.tab_index,
                    target: params.target(),
                })
            }
            "tmux.capture" => {
                let params: TmuxCaptureParams = decode_params(params)?;
                Ok(Self::TmuxCapture {
                    tab_index: params.tab_index,
                    pane: params.target(),
                    target: params.target,
                    lines: params.lines,
                })
            }
            "tmux.send_keys" => {
                let params: TmuxSendKeysParams = decode_params(params)?;
                Ok(Self::TmuxSendKeys {
                    tab_index: params.tab_index,
                    pane: params.target(),
                    target: params.target,
                    literal_text: params.literal_text,
                    key_names: params.key_names,
                    append_enter: params.append_enter,
                })
            }
            "tmux.run" => {
                let params: TmuxRunParams = decode_params(params)?;
                Ok(Self::TmuxRun {
                    tab_index: params.tab_index,
                    pane: params.target(),
                    target: params.target,
                    location: params.location,
                    command: params.command,
                    window_name: params.window_name,
                    cwd: params.cwd,
                    detached: params.detached,
                })
            }
            "agent.ask" => {
                let params: AgentAskParams = decode_params(params)?;
                Ok(Self::AgentAsk {
                    tab_index: params.tab_index,
                    prompt: params.prompt,
                    auto_approve_tools: params.auto_approve_tools,
                    timeout_secs: params.timeout_secs,
                })
            }
            "agent.new_conversation" => {
                let params: TabScopedParams = decode_params(params)?;
                Ok(Self::AgentNewConversation {
                    tab_index: params.tab_index,
                })
            }
            "agent.open_panel_for_request" => {
                let params: TabScopedParams = decode_params(params)?;
                Ok(Self::AgentOpenPanelForRequest {
                    tab_index: params.tab_index,
                })
            }
            "agent.panel_state" => {
                let params: TabScopedParams = decode_params(params)?;
                Ok(Self::AgentPanelState {
                    tab_index: params.tab_index,
                })
            }
            _ => Err(ControlError::method_not_found(method)),
        }
    }
}

#[derive(Debug)]
pub struct ControlRequestEnvelope {
    pub command: ControlCommand,
    pub response_tx: oneshot::Sender<ControlResult>,
}

pub type ControlResult = Result<Value, ControlError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone)]
pub struct ControlError {
    pub code: i32,
    pub message: String,
}

impl ControlError {
    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
        }
    }

    pub fn method_not_found(method: impl AsRef<str>) -> Self {
        Self {
            code: -32601,
            message: format!("Unknown method: {}", method.as_ref()),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
        }
    }

    fn into_json_rpc(self) -> JsonRpcError {
        JsonRpcError {
            code: self.code,
            message: self.message,
        }
    }
}

pub struct ControlSocketHandle {
    path: PathBuf,
}

impl ControlSocketHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ControlSocketHandle {
    fn drop(&mut self) {
        // Unix sockets leave a filesystem entry that we created; on Windows,
        // a Named Pipe disappears once the last server handle drops, so the
        // remove_file is both unnecessary and would silently fail on the
        // `\\.\pipe\…` namespace.
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub fn control_socket_path() -> PathBuf {
    if let Ok(value) = env::var("CON_SOCKET_PATH") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    #[cfg(unix)]
    {
        if let Ok(runtime_dir) = env::var("XDG_RUNTIME_DIR") {
            if !runtime_dir.trim().is_empty() {
                let socket_name = if cfg!(debug_assertions) {
                    "con-debug.sock"
                } else {
                    "con.sock"
                };
                return PathBuf::from(runtime_dir).join("con").join(socket_name);
            }
        }
    }
    PathBuf::from(DEFAULT_SOCKET_PATH)
}

#[cfg(unix)]
pub fn spawn_control_socket_server(
    runtime: Arc<Runtime>,
    request_tx: Sender<ControlRequestEnvelope>,
) -> Result<ControlSocketHandle> {
    use tokio::net::UnixListener;

    let path = control_socket_path();
    prepare_socket_path(&path)?;

    let listener = {
        let _guard = runtime.enter();
        UnixListener::bind(&path)
            .with_context(|| format!("failed to bind control socket at {}", path.display()))?
    };
    set_socket_permissions(&path)?;

    runtime.spawn(async move {
        if let Err(err) = unix_accept_loop(listener, request_tx).await {
            log::error!("[control] socket accept loop failed: {err}");
        }
    });

    Ok(ControlSocketHandle { path })
}

#[cfg(windows)]
pub fn spawn_control_socket_server(
    runtime: Arc<Runtime>,
    request_tx: Sender<ControlRequestEnvelope>,
) -> Result<ControlSocketHandle> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let path = control_socket_path();
    let pipe_name = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("named pipe path must be valid UTF-8"))?
        .to_string();

    // Bind one server eagerly so callers can detect "address in use" before
    // spawn returns. ServerOptions::create with `first_pipe_instance(true)`
    // is the Windows analog of Unix's "another listener already owns this
    // path" check.
    let initial_server = {
        let _guard = runtime.enter();
        ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .with_context(|| format!("failed to bind control pipe at {pipe_name}"))?
    };

    let pipe_name_for_loop = pipe_name.clone();
    runtime.spawn(async move {
        if let Err(err) = windows_accept_loop(initial_server, pipe_name_for_loop, request_tx).await
        {
            log::error!("[control] pipe accept loop failed: {err}");
        }
    });

    Ok(ControlSocketHandle { path })
}

#[cfg(not(any(unix, windows)))]
pub fn spawn_control_socket_server(
    _runtime: Arc<Runtime>,
    _request_tx: Sender<ControlRequestEnvelope>,
) -> Result<ControlSocketHandle> {
    anyhow::bail!(
        "con control socket is not supported on this target — \
         see docs/impl/windows-port.md"
    )
}

#[cfg(unix)]
async fn unix_accept_loop(
    listener: tokio::net::UnixListener,
    request_tx: Sender<ControlRequestEnvelope>,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let request_tx = request_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_client(stream, request_tx).await {
                log::warn!("[control] client connection failed: {err}");
            }
        });
    }
}

#[cfg(windows)]
async fn windows_accept_loop(
    initial_server: tokio::net::windows::named_pipe::NamedPipeServer,
    pipe_name: String,
    request_tx: Sender<ControlRequestEnvelope>,
) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    // The Win32 named-pipe pattern: each connection owns one server
    // instance. Before accepting the current connection, eagerly create the
    // *next* server so a fast-reconnecting client can never observe an
    // empty namespace.
    let mut server = initial_server;
    loop {
        server
            .connect()
            .await
            .context("named pipe connect failed")?;
        let connected = server;

        server = ServerOptions::new()
            .create(&pipe_name)
            .with_context(|| format!("failed to spawn next pipe instance at {pipe_name}"))?;

        let request_tx = request_tx.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_client(connected, request_tx).await {
                log::warn!("[control] client connection failed: {err}");
            }
        });
    }
}

/// Generic over any duplex byte stream so the same JSON-RPC framing serves
/// Unix sockets and Windows Named Pipes.
async fn handle_client<S>(stream: S, request_tx: Sender<ControlRequestEnvelope>) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut lines = BufReader::new(read_half).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => handle_json_rpc_request(request, &request_tx).await,
            Err(err) => JsonRpcResponse {
                jsonrpc: JSON_RPC_VERSION.to_string(),
                id: Value::Null,
                result: None,
                error: Some(ControlError::parse(err.to_string()).into_json_rpc()),
            },
        };

        let encoded = serde_json::to_string(&response)?;
        write_half.write_all(encoded.as_bytes()).await?;
        write_half.write_all(b"\n").await?;
    }

    Ok(())
}

async fn handle_json_rpc_request(
    request: JsonRpcRequest,
    request_tx: &Sender<ControlRequestEnvelope>,
) -> JsonRpcResponse {
    let id = request.id.clone().unwrap_or(Value::Null);

    if request.jsonrpc != JSON_RPC_VERSION {
        return JsonRpcResponse {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(
                ControlError::invalid_request("Only JSON-RPC 2.0 requests are supported")
                    .into_json_rpc(),
            ),
        };
    }

    let command = match ControlCommand::from_rpc(&request.method, request.params) {
        Ok(command) => command,
        Err(err) => {
            return JsonRpcResponse {
                jsonrpc: JSON_RPC_VERSION.to_string(),
                id,
                result: None,
                error: Some(err.into_json_rpc()),
            };
        }
    };

    let (response_tx, response_rx) = oneshot::channel();
    if request_tx
        .send(ControlRequestEnvelope {
            command,
            response_tx,
        })
        .is_err()
    {
        return JsonRpcResponse {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(
                ControlError::internal("con control bridge is unavailable").into_json_rpc(),
            ),
        };
    }

    match response_rx.await {
        Ok(Ok(result)) => JsonRpcResponse {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id,
            result: Some(result),
            error: None,
        },
        Ok(Err(err)) => JsonRpcResponse {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(err.into_json_rpc()),
        },
        Err(_) => JsonRpcResponse {
            jsonrpc: JSON_RPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(ControlError::internal("con control request dropped").into_json_rpc()),
        },
    }
}

/// Sanitize the filesystem path for a Unix-socket-backed listener.
///
/// Stale socket files are removed; live ones cause a hard error. Windows
/// Named Pipes don't live on the filesystem, so this is a no-op there
/// (the equivalent collision check is `ServerOptions::first_pipe_instance`
/// inside [`spawn_control_socket_server`]).
#[cfg(unix)]
fn prepare_socket_path(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if parent.exists() && !parent.is_dir() {
            anyhow::bail!(
                "control socket parent is not a directory: {}",
                parent.display()
            );
        }
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(parent, permissions)?;
    }

    if path.exists() {
        match std::os::unix::net::UnixStream::connect(path) {
            Ok(_) => {
                anyhow::bail!(
                    "another con control socket is already listening at {}",
                    path.display()
                );
            }
            Err(_) => {
                std::fs::remove_file(path).with_context(|| {
                    format!("failed to remove stale control socket {}", path.display())
                })?;
            }
        }
    }

    Ok(())
}

#[cfg(unix)]
fn set_socket_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, permissions).with_context(|| {
        format!(
            "failed to set control socket permissions on {}",
            path.display()
        )
    })
}

fn default_read_lines() -> usize {
    80
}

fn default_capture_lines() -> usize {
    120
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct TabScopedParams {
    tab_index: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct PaneTargetParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
}

impl PaneTargetParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceTargetParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
}

impl SurfaceTargetParams {
    fn target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceWaitReadyParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
    timeout_secs: Option<u64>,
}

impl SurfaceWaitReadyParams {
    fn target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceCreateParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    title: Option<String>,
    command: Option<String>,
    owner: Option<String>,
    close_pane_when_last: bool,
}

impl SurfaceCreateParams {
    fn pane_target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceSplitParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
    location: PaneCreateLocation,
    title: Option<String>,
    command: Option<String>,
    owner: Option<String>,
    close_pane_when_last: bool,
}

impl Default for SurfaceSplitParams {
    fn default() -> Self {
        Self {
            tab_index: None,
            pane_index: None,
            pane_id: None,
            surface_id: None,
            location: PaneCreateLocation::Right,
            title: None,
            command: None,
            owner: None,
            close_pane_when_last: true,
        }
    }
}

impl SurfaceSplitParams {
    fn surface_target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceRenameParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
    title: String,
}

impl SurfaceRenameParams {
    fn target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceCloseParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
    close_empty_owned_pane: bool,
}

impl SurfaceCloseParams {
    fn target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceReadParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
    #[serde(default = "default_read_lines")]
    lines: usize,
}

impl Default for SurfaceReadParams {
    fn default() -> Self {
        Self {
            tab_index: None,
            pane_index: None,
            pane_id: None,
            surface_id: None,
            lines: default_read_lines(),
        }
    }
}

impl SurfaceReadParams {
    fn target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceSendTextParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
    text: String,
}

impl SurfaceSendTextParams {
    fn target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct SurfaceSendKeyParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    surface_id: Option<usize>,
    key: String,
}

impl SurfaceSendKeyParams {
    fn target(&self) -> SurfaceTarget {
        SurfaceTarget::new(self.pane_index, self.pane_id, self.surface_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct PaneReadParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    #[serde(default = "default_read_lines")]
    lines: usize,
}

impl Default for PaneReadParams {
    fn default() -> Self {
        Self {
            tab_index: None,
            pane_index: None,
            pane_id: None,
            lines: default_read_lines(),
        }
    }
}

impl PaneReadParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct PaneExecParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    command: String,
}

impl PaneExecParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct PaneSendKeysParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    keys: String,
}

impl PaneSendKeysParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct PaneCreateParams {
    tab_index: Option<usize>,
    location: PaneCreateLocation,
    command: Option<String>,
}

impl Default for PaneCreateParams {
    fn default() -> Self {
        Self {
            tab_index: None,
            location: PaneCreateLocation::Right,
            command: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct PaneWaitParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    timeout_secs: Option<u64>,
    pattern: Option<String>,
}

impl PaneWaitParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct TmuxCaptureParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    target: Option<String>,
    #[serde(default = "default_capture_lines")]
    lines: usize,
}

impl Default for TmuxCaptureParams {
    fn default() -> Self {
        Self {
            tab_index: None,
            pane_index: None,
            pane_id: None,
            target: None,
            lines: default_capture_lines(),
        }
    }
}

impl TmuxCaptureParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct TmuxSendKeysParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    target: String,
    literal_text: Option<String>,
    key_names: Vec<String>,
    append_enter: bool,
}

impl TmuxSendKeysParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct TmuxRunParams {
    tab_index: Option<usize>,
    pane_index: Option<usize>,
    pane_id: Option<usize>,
    target: Option<String>,
    location: TmuxExecLocation,
    command: String,
    window_name: Option<String>,
    cwd: Option<String>,
    detached: bool,
}

impl Default for TmuxRunParams {
    fn default() -> Self {
        Self {
            tab_index: None,
            pane_index: None,
            pane_id: None,
            target: None,
            location: TmuxExecLocation::NewWindow,
            command: String::new(),
            window_name: None,
            cwd: None,
            detached: false,
        }
    }
}

impl TmuxRunParams {
    fn target(&self) -> PaneTarget {
        PaneTarget::new(self.pane_index, self.pane_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct AgentAskParams {
    tab_index: Option<usize>,
    prompt: String,
    auto_approve_tools: bool,
    timeout_secs: Option<u64>,
}

fn decode_params<T>(params: Value) -> Result<T, ControlError>
where
    T: for<'de> Deserialize<'de> + Default,
{
    let value = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(value).map_err(|err| ControlError::invalid_params(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_command_round_trips_from_rpc() {
        let command = ControlCommand::PanesExec {
            tab_index: Some(2),
            target: PaneTarget::new(None, Some(7)),
            command: "cargo test".to_string(),
        };

        let parsed = ControlCommand::from_rpc(command.method_name(), command.params_json())
            .expect("round-trip parse");

        match parsed {
            ControlCommand::PanesExec {
                tab_index,
                target,
                command,
            } => {
                assert_eq!(tab_index, Some(2));
                assert_eq!(target.pane_id, Some(7));
                assert_eq!(command, "cargo test");
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn surface_command_round_trips_from_rpc() {
        let command = ControlCommand::SurfacesSplit {
            tab_index: Some(3),
            source: SurfaceTarget::new(None, Some(9), Some(12)),
            location: PaneCreateLocation::Down,
            title: Some("worker".to_string()),
            command: Some("codex".to_string()),
            owner: Some("subagent".to_string()),
            close_pane_when_last: true,
        };

        let parsed = ControlCommand::from_rpc(command.method_name(), command.params_json())
            .expect("round-trip parse");

        match parsed {
            ControlCommand::SurfacesSplit {
                tab_index,
                source,
                location,
                title,
                command,
                owner,
                close_pane_when_last,
            } => {
                assert_eq!(tab_index, Some(3));
                assert_eq!(source.pane_id, Some(9));
                assert_eq!(source.surface_id, Some(12));
                assert_eq!(location, PaneCreateLocation::Down);
                assert_eq!(title.as_deref(), Some("worker"));
                assert_eq!(command.as_deref(), Some("codex"));
                assert_eq!(owner.as_deref(), Some("subagent"));
                assert!(close_pane_when_last);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn surface_split_defaults_to_ephemeral_pane_close() {
        let parsed = ControlCommand::from_rpc("surfaces.split", json!({}))
            .expect("default surface split parse");

        match parsed {
            ControlCommand::SurfacesSplit {
                close_pane_when_last,
                ..
            } => assert!(close_pane_when_last),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn surface_wait_ready_accepts_timeout() {
        let parsed = ControlCommand::from_rpc(
            "surfaces.wait_ready",
            json!({
                "tab_index": 2,
                "surface_id": 7,
                "timeout_secs": 15,
            }),
        )
        .expect("surface wait-ready parse");

        match parsed {
            ControlCommand::SurfacesWaitReady {
                tab_index,
                target,
                timeout_secs,
            } => {
                assert_eq!(tab_index, Some(2));
                assert_eq!(target.surface_id, Some(7));
                assert_eq!(timeout_secs, Some(15));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn unknown_method_returns_json_rpc_error() {
        let err = ControlCommand::from_rpc("con.unknown", json!({})).expect_err("should fail");
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn control_methods_are_non_empty() {
        assert!(!control_methods().is_empty());
    }

    #[test]
    fn debug_build_uses_isolated_default_control_endpoint() {
        if cfg!(debug_assertions) {
            assert!(
                DEFAULT_SOCKET_PATH.contains("debug"),
                "debug builds should not share the production control endpoint"
            );
        } else {
            assert!(
                !DEFAULT_SOCKET_PATH.contains("debug"),
                "release builds should keep the stable production control endpoint"
            );
        }
    }
}
