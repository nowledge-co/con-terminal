use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Codex,
    Cursor,
    Kimi,
    /// Retained records from removed or newer adapters; never selectable.
    #[serde(other)]
    Unknown,
}

impl AgentKind {
    pub const ALL: [Self; 3] = [Self::Codex, Self::Cursor, Self::Kimi];

    pub fn default_target() -> Self {
        Self::Cursor
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Kimi => "kimi",
            Self::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Kimi => "Kimi",
            Self::Unknown => "Unsupported Agent",
        }
    }

    pub fn executable_name(self) -> &'static str {
        if self == Self::Cursor {
            "cursor-agent"
        } else {
            self.as_str()
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for AgentKind {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value.to_ascii_lowercase())
            .ok_or_else(|| format!("Unknown agent: {value}"))
    }
}

#[cfg(test)]
mod tests {
    use super::AgentKind;
    use crate::handoff::{
        TargetCapabilities, active_session_binding, candidate_models, create_target,
        discover_sessions, export_session, probe_target, target_args,
    };

    #[test]
    fn only_three_agents_are_selectable() {
        assert_eq!(
            AgentKind::ALL,
            [AgentKind::Codex, AgentKind::Cursor, AgentKind::Kimi]
        );
        assert!("dim".parse::<AgentKind>().is_err());
    }

    #[tokio::test]
    async fn unknown_agents_are_readable_but_cannot_be_used() {
        let agent: AgentKind = serde_json::from_str("\"opencode\"").unwrap();
        assert_eq!(agent, AgentKind::Unknown);
        assert!("unknown".parse::<AgentKind>().is_err());
        assert!(!AgentKind::ALL.contains(&agent));
        let cwd = std::env::temp_dir();
        let target = TargetCapabilities {
            agent,
            executable: cwd.join("must-not-run"),
            version: "old".into(),
            automatic_delivery: true,
        };
        assert!(probe_target(agent).await.is_err());
        assert!(create_target(&target, &cwd).await.is_err());
        assert!(target_args(&target, &cwd, None, None).is_err());
        assert!(discover_sessions(agent, &cwd).await.is_err());
        assert!(export_session(agent, &cwd, "old").await.is_err());
        assert!(active_session_binding(agent, 0, &[], &cwd).await.is_err());
        assert!(candidate_models(&target, &cwd).await.is_empty());
    }
}
