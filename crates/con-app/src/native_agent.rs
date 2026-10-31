//! Per-terminal native conversation identity, independent of presentation status.
use con_core::native_session::NativeAgentSession;
use std::{cell::RefCell, rc::Rc};

pub(crate) type SharedNativeAgent = Rc<RefCell<NativeAgentState>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NativeAgentPhase {
    Available,
    Observed,
    Resuming,
    Started,
    Failed,
    NeedsInteraction,
}

pub(crate) struct NativeAgentState {
    pub origin: uuid::Uuid,
    pub reference: Option<NativeAgentSession>,
    pub phase: NativeAgentPhase,
    pub error: Option<String>,
    pub auto_attempted: bool,
    pub restored: bool,
    pub report_sequence: u64,
    pub resume_submitted_at: Option<std::time::Instant>,
}

impl Default for NativeAgentState {
    fn default() -> Self {
        Self {
            origin: uuid::Uuid::new_v4(),
            reference: None,
            phase: NativeAgentPhase::Available,
            error: None,
            auto_attempted: false,
            restored: false,
            report_sequence: 0,
            resume_submitted_at: None,
        }
    }
}

impl NativeAgentState {
    pub fn restore(&mut self, reference: NativeAgentSession) {
        self.reference = Some(reference);
        self.restored = true;
        self.phase = NativeAgentPhase::Available;
        if let Some(error) = self
            .reference
            .as_ref()
            .and_then(|reference| reference.validate().err())
        {
            self.phase = NativeAgentPhase::Failed;
            self.error = Some(error);
            self.auto_attempted = true;
        }
    }
    pub fn summary(&self) -> Option<String> {
        let reference = self.reference.as_ref()?;
        let state = match self.phase {
            NativeAgentPhase::Available => "Saved conversation · Resume available",
            NativeAgentPhase::Observed => "Native conversation linked",
            NativeAgentPhase::Resuming => "Resuming conversation…",
            NativeAgentPhase::Started => "Native CLI started · Waiting for session confirmation",
            NativeAgentPhase::Failed => "Could not resume conversation",
            NativeAgentPhase::NeedsInteraction => "Resume needs attention",
        };
        Some(format!(
            "{}\n{}{}",
            reference.label(),
            state,
            self.error
                .as_ref()
                .map(|e| format!("\n{e}"))
                .unwrap_or_default()
        ))
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn launch_environment(state: Option<&SharedNativeAgent>) -> Vec<(String, String)> {
    state
        .map(|state| {
            vec![
                (
                    "CON_TERMINAL_CONTEXT".into(),
                    state.borrow().origin.to_string(),
                ),
                (
                    "CON_SOCKET_PATH".into(),
                    con_core::control_socket_path()
                        .to_string_lossy()
                        .into_owned(),
                ),
            ]
        })
        .unwrap_or_default()
}
