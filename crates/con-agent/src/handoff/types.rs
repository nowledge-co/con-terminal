use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::AgentKind;

pub const MAX_EXPORT_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_CONTEXT_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSession {
    /// Local preflight failure; visible in discovery, never a recent suggestion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_warning: Option<String>,
    #[serde(default)]
    pub agent: AgentKind,
    pub id: String,
    pub store_identity: String,
    pub title: String,
    pub cwd: PathBuf,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryRecord {
    pub turn_id: String,
    pub item_id: String,
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryExport {
    pub source: SourceSession,
    #[serde(alias = "codex_version")]
    pub agent_version: String,
    pub last_turn_id: String,
    pub records: Vec<HistoryRecord>,
    pub omissions: Vec<String>,
    pub digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetCapabilities {
    #[serde(default = "AgentKind::default_target")]
    pub agent: AgentKind,
    pub executable: PathBuf,
    pub version: String,
    /// Native argv delivery only, not an ACK. Kimi stays false: Con uses
    /// screen-gated PTY injection without changing the serialized capability.
    pub automatic_delivery: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentAvailability {
    pub agent: AgentKind,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub source_supported: bool,
    pub target_supported: bool,
    pub diagnostic: Option<String>,
}
