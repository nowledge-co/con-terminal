use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

use con_agent::handoff::{
    AgentAvailability, AgentKind, SourceSession, candidate_models, probe_target,
};
use con_core::handoff::{HandoffJob, validate_target_model};
use gpui::*;
use gpui_component::input::{InputEvent, InputState};
use tokio::runtime::Runtime;

mod binding_check;
mod fallback_refresh;
mod lifecycle;
mod prepare;
mod presence;
mod source_selection;
mod view;

use presence::{TargetPresence, classify_target_presence};

pub(super) use super::cancel_prepared_handoff;

/// Brand logo for an Agent, used by destination rows and the "+" picker.
pub(super) fn agent_icon(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Unknown => "phosphor/question.svg",
        AgentKind::Codex => "agents/codex.svg",
        AgentKind::Cursor => "agents/cursor.svg",
        AgentKind::Kimi => "agents/kimi.svg",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ExistingAgentTab {
    pub tab_id: u64,
    pub terminal_id: EntityId,
    pub foreground_group: u64,
    pub agent: AgentKind,
    pub label: String,
    pub position: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Destination {
    Existing(ExistingAgentTab),
    NewTab(AgentKind),
}

#[derive(Clone)]
pub(super) struct ExecuteHandoff {
    pub job: HandoffJob,
    pub destination: Destination,
    /// Live-bound source session at send time; `None` when the session was
    /// picked manually. `route_handoff` re-binds this evidence against the
    /// live foreground Agent before any state transaction.
    pub live_source_binding: Option<String>,
    pub source_session_confirmed: bool,
}

pub(super) struct HandoffDestinationPanel {
    focus_handle: FocusHandle,
    cwd: PathBuf,
    runtime: Arc<Runtime>,
    source_agent: AgentKind,
    source_foreground_group: u64,
    source_terminal: crate::terminal_pane::TerminalPane,
    live_source_id: Option<String>,
    live_source_requires_confirmation: bool,
    /// True when the source TUI has not created a session yet (Kimi before
    /// its first message) and no live binding exists: the panel shows a
    /// "no session yet" note instead of offering stale history as the
    /// handoff source.
    source_not_started: bool,
    /// Explicit user pick among multiple candidate sessions (no live binding).
    selected_session_id: Option<String>,
    picker_open: bool,
    sessions: Vec<SourceSession>,
    agents: Vec<AgentAvailability>,
    existing: Vec<ExistingAgentTab>,
    selected_destination: Option<Destination>,
    /// Exact model ID typed for a new-tab launch; created lazily on the
    /// first new-tab selection. Empty means "default (don't specify)".
    model_input: Option<Entity<InputState>>,
    /// Candidate model IDs probed for the selected new-tab agent. Empty
    /// when the agent has no side-effect-free listing or the probe failed;
    /// exact-ID entry stays available either way.
    model_candidates: Vec<String>,
    /// The agent `model_candidates` were probed for.
    model_candidates_for: Option<AgentKind>,
    active_job: Option<HandoffJob>,
    instruction_available: Option<String>,
    copied_instructions: std::collections::HashSet<String>,
    /// Cached records for selecting the latest non-terminal job of this Tab.
    jobs: Vec<HandoffJob>,
    /// Job ID of the most recently sent handoff; lets `report_error` refresh
    /// the job (and surface its card when it is non-terminal) after the
    /// router reports a failure, without the router passing it back.
    sent_job_id: Option<String>,
    /// Request ID (= job ID) of an in-flight prepare; lets teardown cancel a
    /// job whose delivery never started.
    preparing_request_id: Option<String>,
    /// Set when the panel is released; an in-flight prepare cancels its job
    /// instead of leaving an unused Prepared job behind.
    closed: Arc<AtomicBool>,
    /// Progressive disclosure bound for the session candidate list.
    visible_candidate_limit: usize,
    busy: bool,
    loading_agents: bool,
    loading_sessions: bool,
    single_session_confirmed: bool,
    error: Option<String>,
    binding_warning: Option<String>,
}

impl EventEmitter<ExecuteHandoff> for HandoffDestinationPanel {}

impl HandoffDestinationPanel {
    pub fn new(
        cwd: PathBuf,
        runtime: Arc<Runtime>,
        source_agent: AgentKind,
        source_foreground_group: u64,
        source_terminal: crate::terminal_pane::TerminalPane,
        existing: Vec<ExistingAgentTab>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            cwd,
            runtime,
            source_agent,
            source_foreground_group,
            source_terminal,
            live_source_id: None,
            live_source_requires_confirmation: false,
            source_not_started: false,
            selected_session_id: None,
            picker_open: false,
            sessions: Vec::new(),
            agents: Vec::new(),
            existing,
            selected_destination: None,
            model_input: None,
            model_candidates: Vec::new(),
            model_candidates_for: None,
            active_job: None,
            instruction_available: None,
            copied_instructions: Default::default(),
            jobs: Vec::new(),
            sent_job_id: None,
            preparing_request_id: None,
            closed: Arc::new(AtomicBool::new(false)),
            visible_candidate_limit: view::CANDIDATE_PAGE,
            busy: false,
            loading_agents: false,
            loading_sessions: false,
            single_session_confirmed: false,
            error: None,
            binding_warning: None,
        }
    }

    fn selected_source(&self) -> Option<&SourceSession> {
        source_selection::selected_source(
            &self.sessions,
            self.live_source_id.as_deref(),
            self.selected_session_id.as_deref(),
            self.source_not_started,
        )
    }

    fn source_confirmed(&self) -> bool {
        if self.selected_source().is_none() {
            return false;
        }
        if self.live_source_id.is_some() {
            return !self.live_source_requires_confirmation || self.single_session_confirmed;
        }
        // An explicit pick among multiple candidates is itself the confirmation.
        if self.selected_session_id.is_some() {
            return true;
        }
        self.single_session_confirmed
    }

    fn target_agents(&self) -> Vec<AgentKind> {
        self.agents
            .iter()
            .filter(|agent| agent.target_supported)
            .map(|agent| agent.agent)
            .collect()
    }

    /// A listed job belongs in the main flow only when it provably originates
    /// from this Tab: same source Agent, and its source session is the one
    /// bound to the live foreground process (or the session the user
    /// explicitly picked when no live binding exists). With no session
    /// binding at all, no job is presented as current.
    fn related_to_current_tab(&self, job: &HandoffJob) -> bool {
        if job.request.source_agent != self.source_agent {
            return false;
        }
        let bound = self
            .live_source_id
            .as_ref()
            .or(self.selected_session_id.as_ref());
        bound.is_some_and(|id| *id == job.request.source_session_id)
    }

    fn promote_related_job(&mut self, cx: &mut Context<Self>) {
        self.active_job = self
            .jobs
            .iter()
            .filter(|job| !job.state.is_terminal() && self.related_to_current_tab(job))
            .max_by_key(|job| (job.created_at, job.updated_at))
            .cloned();
        self.refresh_active_job(cx);
        cx.notify();
    }

    /// Reconcile only the displayed job; this never controls new sends.
    fn refresh_active_job(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.active_job.clone() else {
            return;
        };
        if job.state == con_core::handoff::HandoffState::LaunchPending {
            self.arm_launch_expiry(&job, cx);
        }
        if self.target_presence(&job) != TargetPresence::Absent {
            return;
        }
        let task = self.runtime.spawn_blocking(move || {
            con_core::handoff::HandoffService::new()?.cancel_absent_target(&job.id, job.revision)
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(job)) = task.await {
                let _ = this.update(cx, |panel, cx| {
                    panel.absorb_job_update(job);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn tracked_job(&self, id: &str) -> Option<&HandoffJob> {
        self.active_job.as_ref().filter(|job| job.id == id)
    }

    fn absorb_job_update(&mut self, job: HandoffJob) {
        if let Some(cached) = self.jobs.iter_mut().find(|cached| cached.id == job.id) {
            *cached = job.clone();
        }
        if self.tracked_job(&job.id).is_some() {
            self.active_job = (!job.state.is_terminal()).then_some(job);
        }
    }

    /// Live process stamp for a recorded pid: start time, and the Agent that
    /// process is right now. A dead pid yields `(None, None)`.
    fn observe_target(&self, pid: u32) -> (Option<u64>, Option<con_agent::handoff::AgentKind>) {
        let Some(start) = con_agent::handoff::process_start_secs(pid as i32) else {
            return (None, None);
        };
        let agent = crate::process_name::process_name(pid as u64)
            .and_then(|name| super::running_source_agent(None, &name, pid as u64, &[]));
        (Some(start), agent)
    }

    fn target_presence(&self, job: &HandoffJob) -> TargetPresence {
        let (observed_start, observed_agent) = job
            .target_pid
            .map(|pid| self.observe_target(pid))
            .unwrap_or((None, None));
        let live_agents: Vec<_> = self.existing.iter().map(|tab| tab.agent).collect();
        classify_target_presence(
            job.state,
            job.target.agent,
            job.target_pid,
            job.target_process_start,
            observed_start,
            observed_agent,
            &live_agents,
        )
    }

    /// The window re-fits itself to the measured content on every prepaint,
    /// so opening or closing the picker only flips state here.
    fn set_picker_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.picker_open == open {
            return;
        }
        self.picker_open = open;
        cx.notify();
    }

    /// Kick off a background candidate-model probe for a newly picked
    /// target agent. The probe (`con_agent::handoff::candidate_models`)
    /// never errors — a missing list command, failure, or timeout yields
    /// empty candidates — so it must not block the panel or surface an
    /// error; exact-ID entry stays available regardless. Results arriving
    /// after a destination switch are dropped.
    fn load_model_candidates(&mut self, agent: AgentKind, cx: &mut Context<Self>) {
        self.model_candidates.clear();
        self.model_candidates_for = None;
        let cwd = self.cwd.clone();
        let task = self.runtime.spawn(async move {
            let capabilities = probe_target(agent).await.ok()?;
            let models = candidate_models(&capabilities, &cwd).await;
            (!models.is_empty()).then_some(models)
        });
        cx.spawn(async move |this, cx| {
            let models = task.await.ok().flatten().unwrap_or_default();
            let _ = this.update(cx, |panel, cx| {
                if panel.selected_destination == Some(Destination::NewTab(agent)) {
                    panel.model_candidates = models;
                    panel.model_candidates_for = Some(agent);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Lazily create the model input on the first new-tab selection so the
    /// panel opens without it. Typing re-renders the panel (validation hint,
    /// prepare gating) through the change subscription.
    fn ensure_model_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model_input.is_some() {
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Default (don't specify)"));
        cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        self.model_input = Some(input);
    }

    /// Switching the target Agent (or moving off the new-tab path) resets
    /// the model to "default (don't specify)" and drops probed candidates.
    fn clear_model_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(input) = &self.model_input {
            input.update(cx, |state, cx| state.set_value("", window, cx));
        }
        self.model_candidates.clear();
        self.model_candidates_for = None;
    }

    /// The trimmed model ID as currently typed; "" when unset.
    fn model_value(&self, cx: &App) -> String {
        self.model_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_string())
            .unwrap_or_default()
    }

    /// Live validation of the typed model; empty (the default) is valid.
    fn model_error(&self, cx: &App) -> Option<String> {
        let value = self.model_value(cx);
        if value.is_empty() {
            return None;
        }
        validate_target_model(&value)
            .err()
            .map(|error| error.to_string())
    }

    /// The model to write into `PrepareRequest`: only the new-tab path may
    /// carry one, and only when the user explicitly typed an ID.
    fn requested_model(&self, destination: &Destination, cx: &App) -> Option<String> {
        if !matches!(destination, Destination::NewTab(_)) {
            return None;
        }
        let value = self.model_value(cx);
        (!value.is_empty()).then_some(value)
    }
}

impl Focusable for HandoffDestinationPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
