//! C-path guidance is evidence, never a delivery receipt.
use super::{HandoffJob, HandoffState};
use con_agent::handoff::AgentKind;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackKind {
    NotReady,
    Trust,
    PastePending,
    SubmitUncertain,
    LaunchTimeout,
    ArgvBootstrap,
    TargetDead,
}

impl FallbackKind {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotReady => "Kimi still starting",
            Self::Trust => "Approve folder trust in Kimi first",
            Self::PastePending => "Paste the instruction in Kimi",
            Self::SubmitUncertain => "Could not verify send — check Kimi",
            Self::LaunchTimeout => "Target did not start in time",
            Self::ArgvBootstrap => "Target failed to start — see target Tab",
            Self::TargetDead => "Target process ended",
        }
    }

    pub fn from_kimi_failure(key: &str) -> Self {
        match key {
            "trust" => Self::Trust,
            "not-ready" => Self::NotReady,
            "paste_pending" | "clipboard_failed" => Self::PastePending,
            _ => Self::SubmitUncertain,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackEvidence {
    #[default]
    Idle,
    PasteDetected,
    Submitted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffFallbackGuide {
    pub kind: FallbackKind,
    pub clipboard_copied: bool,
    #[serde(default)]
    pub evidence: FallbackEvidence,
}

impl HandoffJob {
    pub(crate) fn set_fallback(&mut self, kind: FallbackKind, clipboard_copied: bool) {
        self.error = Some(kind.message().into());
        self.fallback = Some(HandoffFallbackGuide {
            kind,
            clipboard_copied,
            evidence: FallbackEvidence::Idle,
        });
    }

    pub fn fallback_confirmation_ready(&self) -> bool {
        self.state == HandoffState::NeedsInteraction
            && self.fallback.as_ref().is_some_and(|guide| {
                !matches!(
                    guide.kind,
                    FallbackKind::TargetDead | FallbackKind::LaunchTimeout
                ) && self.target.agent == AgentKind::Kimi
                    && guide.evidence != FallbackEvidence::Idle
            })
    }
}
