use std::path::PathBuf;

use anyhow::{Result, ensure};
use con_agent::handoff::{AgentKind, HistoryExport, TargetCapabilities};
use serde::{Deserialize, Serialize};

/// Maximum accepted byte length of a requested target model identifier.
pub const MAX_TARGET_MODEL_LEN: usize = 128;

/// Launch-helper protocol revision understood by this build. Bump when the
/// visible `con-cli handoff run` helper gains a new job capability; helpers
/// must refuse jobs that require a newer protocol instead of silently
/// dropping the capability (currently: target model override = 2).
pub const LAUNCH_HELPER_PROTOCOL: u32 = 2;

/// Reject model identifiers that are empty, option-like, overlong, or carry
/// control characters. The value travels as a standalone argv argument (or a
/// dedicated environment variable for the target child process), never
/// interpolated into a shell command, so a leading `-` — which a CLI could
/// parse as its own flag — is the dangerous shape to keep out.
pub fn validate_target_model(model: &str) -> Result<()> {
    ensure!(
        !model.trim().is_empty(),
        "Target model cannot be empty; leave it unset to keep the target's own model"
    );
    ensure!(
        model.len() <= MAX_TARGET_MODEL_LEN,
        "Target model is limited to {MAX_TARGET_MODEL_LEN} bytes"
    );
    ensure!(
        !model.chars().any(char::is_control),
        "Target model cannot contain control characters"
    );
    ensure!(
        !model.starts_with('-'),
        "Target model cannot look like a command-line option"
    );
    Ok(())
}

/// Validate the target and model before persisting a job.
pub fn validate_target_model_for_agent(agent: AgentKind, model: &str) -> Result<()> {
    ensure!(agent != AgentKind::Unknown, "Unsupported target Agent");
    validate_target_model(model)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrepareRequest {
    #[serde(default)]
    pub source_agent: AgentKind,
    #[serde(default = "AgentKind::default_target")]
    pub target_agent: AgentKind,
    pub request_id: String,
    pub cwd: PathBuf,
    pub source_session_id: String,
    #[serde(default)]
    pub goal: String,
    /// Exact model identifier requested for this handoff's native launch.
    /// `None` — the default, including jobs written before this field
    /// existed — keeps the target's own model configuration untouched.
    /// Only the launch-a-new-tab path may set this: delivery to an
    /// already-running tab must keep it `None`, because a live process
    /// cannot change models. Non-empty values must pass
    /// `validate_target_model` before a job is persisted, and the launch
    /// helper passes the value as its own argv argument — never as part of
    /// a shell command line. The value participates in request-idempotency
    /// comparison, so retrying a request ID with a different model is a
    /// conflict, not a replay.
    #[serde(default)]
    pub target_model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub cwd: PathBuf,
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub git_common_dir: PathBuf,
    pub head: String,
    pub index_digest: String,
    pub worktree_digest: String,
    pub status: String,
    pub untracked: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffBundle {
    pub schema_version: u32,
    pub handoff_id: String,
    pub created_at: u64,
    pub history: HistoryExport,
    pub workspace: WorkspaceSnapshot,
    pub goal: String,
    pub context: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HandoffState {
    Prepared,
    LaunchPending,
    StartingTarget,
    Delivering,
    /// Kept for backward-compatible deserialization of jobs written before
    /// the send-is-confirmation change (2026-09-24): those still complete via
    /// `ConfirmReceived`. New flows go from Delivering /
    /// AwaitingManualDelivery straight to Active.
    AwaitingConfirmation,
    AwaitingManualDelivery,
    Active,
    NeedsInteraction,
    Cancelled,
    Failed,
}

impl HandoffState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Cancelled | Self::Failed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Prepared => "Ready to hand off",
            Self::LaunchPending => "Opening target…",
            Self::StartingTarget => "Creating target session…",
            Self::Delivering => "Delivery outcome unknown — check the target",
            Self::AwaitingConfirmation => "Delivery attempted — confirm receipt",
            Self::AwaitingManualDelivery => "Automatic delivery unavailable — paste instruction",
            Self::Active => "Sent — check target",
            Self::NeedsInteraction => "Finish handoff manually",
            Self::Cancelled => "Handoff cancelled",
            Self::Failed => "Handoff failed before launch",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HandoffJob {
    pub id: String,
    pub revision: u64,
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    pub request: PrepareRequest,
    pub state: HandoffState,
    pub target: TargetCapabilities,
    /// Con tab selected for delivery to an already-running native Agent.
    /// None for the original launch-a-new-target flow and older records.
    #[serde(default)]
    pub existing_target_tab_id: Option<u64>,
    pub target_session_id: Option<String>,
    pub target_pid: Option<u32>,
    /// Unix seconds of `target_pid`'s start time. Together with the pid this
    /// identifies the process that received the handoff. A reused pid has a
    /// different start time and is not this target. Absent on older records.
    #[serde(default)]
    pub target_process_start: Option<u64>,
    pub receipt: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub fallback: Option<super::HandoffFallbackGuide>,
}

/// Explicit outcomes; no boolean approval or implicit process interruption.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffResponse {
    ConfirmReceived,
    ConfirmSent,
    ConfirmStopped,
    Abandon,
}

impl HandoffJob {
    /// Explicitly relinquish a handoff without claiming the target stopped.
    /// A recorded Kimi submit keeps the stop-confirmation path.
    pub fn can_abandon(&self) -> bool {
        self.receipt.as_deref() != Some("pty_injection_submit_observed")
            && (matches!(
                self.state,
                HandoffState::Prepared | HandoffState::NeedsInteraction
            ) || (self.state == HandoffState::Delivering
                && self.target.agent == AgentKind::Kimi
                && self.existing_target_tab_id.is_none()))
    }
}
