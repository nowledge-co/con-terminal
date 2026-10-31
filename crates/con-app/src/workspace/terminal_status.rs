use std::collections::{HashMap, HashSet};
use std::sync::Weak;
use std::time::{Duration, Instant};

use con_core::program_status::{self, Incoming};
use con_core::terminal_status::{AgentIdentity, IdentityScope, SurfaceStatus};
use con_ghostty::process::ProcessInfo;
use con_ghostty::{GhosttyTerminal, ProgramStatusEvent};

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Query {
    #[cfg(target_os = "linux")]
    Host,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    Foreground(u32),
    #[cfg(target_os = "windows")]
    Descendants(con_ghostty::process::ProcessIdentity),
    Unavailable,
}

impl Query {
    fn for_terminal(terminal: &TerminalPane, cx: &App) -> Self {
        #[cfg(target_os = "linux")]
        if terminal
            .surface_instance(cx)
            .and_then(|instance| instance.upgrade())
            .is_some_and(|terminal| terminal.uses_host_process_namespace())
        {
            return Self::Host;
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        return terminal
            .foreground_process_group_id(cx)
            .and_then(|pid| u32::try_from(pid).ok())
            .map(Self::Foreground)
            .unwrap_or(Self::Unavailable);
        #[cfg(target_os = "windows")]
        return terminal
            .root_identity(cx)
            .map(Self::Descendants)
            .unwrap_or(Self::Unavailable);
    }

    fn collect_batch(queries: &[Self]) -> Vec<Vec<ProcessInfo>> {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let groups: Vec<_> = queries
                .iter()
                .map(|query| match query {
                    Self::Foreground(pgid) => *pgid,
                    #[cfg(target_os = "linux")]
                    Self::Host => 0,
                    Self::Unavailable => 0,
                })
                .collect();
            con_ghostty::process::group_members_batch(&groups)
        }
        #[cfg(target_os = "windows")]
        {
            let roots: Vec<_> = queries
                .iter()
                .filter_map(|query| match query {
                    Self::Descendants(root) => Some(root.clone()),
                    Self::Unavailable => None,
                })
                .collect();
            let mut results = con_ghostty::process::descendants_batch(&roots).into_iter();
            queries
                .iter()
                .map(|query| match query {
                    Self::Descendants(_) => results.next().unwrap_or_default(),
                    Self::Unavailable => Vec::new(),
                })
                .collect()
        }
    }
}

struct Surface {
    incarnation: uuid::Uuid,
    instance: Weak<GhosttyTerminal>,
    query: Query,
    processes: Vec<ProcessInfo>,
    identity_process: Option<con_ghostty::process::ProcessIdentity>,
    /// Interpreter process whose screen may name a script-based agent. A
    /// screen identity lives exactly as long as this process does.
    screen_host: Option<con_ghostty::process::ProcessIdentity>,
    /// Visible lines last seen before the current program started. Output
    /// left by earlier commands is not evidence about the next program.
    shell_screen: HashMap<String, usize>,
    shell_screen_revision: Option<u64>,
    revision: u64,
    detection: AgentCliDetectionState,
    last_scan: Option<Instant>,
    /// Program-status records live in `status` and follow this terminal
    /// entity across pane and tab moves. Replacing or closing the entity
    /// drops them.
    status: SurfaceStatus,
    #[cfg(target_os = "linux")]
    host_request: Option<(u64, Instant)>,
    #[cfg(target_os = "linux")]
    host_observed: Option<(Option<u32>, Instant)>,
}

impl Surface {
    fn accept_processes(
        &mut self,
        instance: &Weak<GhosttyTerminal>,
        query: &Query,
        revision: u64,
        processes: Vec<ProcessInfo>,
    ) {
        if !self.instance.ptr_eq(instance) || self.query != *query || self.revision != revision {
            return;
        }
        self.replace_processes(processes);
    }

    fn replace_processes(&mut self, processes: Vec<ProcessInfo>) {
        // Unrelated child PID churn must not reopen the screen-scan budget.
        if native_agent(&self.processes, None) != native_agent(&processes, None) {
            self.detection = AgentCliDetectionState::default();
        }
        self.processes = processes;
    }

    /// Process and title evidence outrank screen text, which may be chat,
    /// code or logs. Screen text only names an otherwise unidentified
    /// interpreter host, using lines it painted itself, and the first match
    /// holds for that host's lifetime.
    fn resolve_identity(
        &mut self,
        observation: AgentCliObservation,
        now: Instant,
        mut read_screen: impl FnMut() -> Vec<String>,
    ) -> Option<(&'static str, IdentityScope)> {
        let direct = native_agent(&self.processes, self.identity_process.as_ref());
        let identity_process = direct.map(|(_, identity)| identity.clone());
        let direct = direct.map(|(agent, _)| agent);
        if self.identity_process != identity_process {
            self.identity_process = identity_process;
            self.revision += 1;
        }
        let screen_host = screen_host(&self.processes, self.screen_host.as_ref()).cloned();
        if self.screen_host != screen_host {
            self.screen_host = screen_host;
            self.revision += 1;
            self.detection = AgentCliDetectionState::default();
        }
        let observation_changed = self.detection.observe(observation);
        if observation_changed {
            self.last_scan = None;
        }
        // Windows collects a process tree, not a foreground group. A tree
        // containing only shells is still safe negative evidence; refresh its
        // baseline too, rather than reusing output from a previous agent.
        let shell_foreground = !self.processes.is_empty()
            && self
                .processes
                .iter()
                .all(|process| process_name_is_shell(&process.identity.name));
        // Typing at the prompt refreshes the snapshot. A new job is also
        // captured before its process list arrives, which covers output
        // written to the PTY without keyboard input.
        let new_job = self.shell_screen_revision != Some(self.revision);
        if (shell_foreground && observation_changed)
            || ((shell_foreground || self.processes.is_empty()) && new_job)
        {
            self.shell_screen.clear();
            for line in read_screen() {
                *self.shell_screen.entry(line).or_default() += 1;
            }
            self.shell_screen_revision = Some(self.revision);
        }
        let scope = if cfg!(target_os = "windows") {
            IdentityScope::ProcessTree
        } else {
            IdentityScope::ForegroundJob
        };
        let confirmed_screen = self
            .status
            .identity()
            .filter(|identity| {
                identity.scope == IdentityScope::Screen && identity.generation == self.revision
            })
            .map(|identity| identity.agent);
        let detected = if shell_foreground {
            None
        } else if let Some(agent) = direct {
            Some((agent, scope))
        } else if let Some(agent) = observation.title_agent {
            Some((agent, IdentityScope::Title))
        } else if self.screen_host.is_none() {
            None
        } else if let Some(agent) = confirmed_screen {
            Some((agent, IdentityScope::Screen))
        } else if self
            .last_scan
            .is_none_or(|last| now.duration_since(last) >= self.detection.scan_interval())
            && self.detection.take_screen_scan_attempt()
        {
            self.last_scan = Some(now);
            log::trace!(
                target: "con::activity",
                "identity_screen_scan surface={}",
                observation.terminal_id
            );
            let mut earlier = self.shell_screen.clone();
            let lines: Vec<_> = read_screen()
                .into_iter()
                .filter(|line| match earlier.get_mut(line) {
                    Some(count) if *count > 0 => {
                        *count -= 1;
                        false
                    }
                    _ => true,
                })
                .collect();
            con_agent::context::classify_screen_agent_cli(None, &lines)
                .or_else(|| agent_from_screen_text(&lines))
                .map(|agent| (agent, IdentityScope::Screen))
        } else {
            None
        };
        if shell_foreground || detected.is_some() {
            self.detection.finish();
        }
        detected
    }
}

fn screen_host<'a>(
    processes: &'a [ProcessInfo],
    previous: Option<&con_ghostty::process::ProcessIdentity>,
) -> Option<&'a con_ghostty::process::ProcessIdentity> {
    let hosts: Vec<_> = processes
        .iter()
        .map(|process| &process.identity)
        .filter(|identity| {
            process_name_is_interpreter(&identity.name)
                || identity
                    .executable
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(process_name_is_interpreter)
        })
        .collect();
    hosts
        .iter()
        .find(|host| Some(**host) == previous)
        .or_else(|| hosts.first())
        .copied()
}

fn native_agent<'a>(
    processes: &'a [ProcessInfo],
    previous: Option<&con_ghostty::process::ProcessIdentity>,
) -> Option<(&'static str, &'a con_ghostty::process::ProcessIdentity)> {
    let mut candidates = processes.iter().filter_map(|process| {
        process
            .identity
            .executable
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(agent_from_process_name)
            .or_else(|| agent_from_process_name(&process.identity.name))
            .or_else(|| {
                // Claude's native installer resolves `claude` to a versioned
                // executable; macOS reports the version as its process name.
                let path = &process.identity.executable;
                let version = path.file_name()?.to_str()?;
                let mut parts = version.split('.');
                (path.parent()?.file_name()? == "versions"
                    && path.parent()?.parent()?.file_name()? == "claude"
                    && parts.clone().count() == 3
                    && parts.all(|part| {
                        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
                    }))
                .then_some("claude")
            })
            .map(|agent| (agent, &process.identity))
    });
    let mut selected = candidates.next()?;
    for candidate in candidates {
        if candidate.0 != selected.0 {
            return None;
        }
        if Some(candidate.1) == previous {
            selected = candidate;
        }
    }
    Some(selected)
}

#[derive(Default)]
pub(super) struct TerminalPresentation {
    surfaces: HashMap<u64, Surface>,
    pub(super) tabs: HashMap<u64, Option<con_core::terminal_status::Status>>,
    program_details: HashMap<u64, Option<crate::sidebar::ProgramStatusDetail>>,
    pub(super) pane_details: HashMap<u64, Vec<crate::sidebar::PaneHoverDetail>>,
    in_flight: bool,
    last_query: Option<Instant>,
    sequence: u64,
    presented_focus: Option<(u64, usize, Option<u64>)>,
}

impl ConWorkspace {
    pub(super) fn explicit_program_observation(
        &self,
        terminal: &TerminalPane,
    ) -> Option<con_core::program_observation::ProgramObservation> {
        let surface = self
            .terminal_presentation
            .surfaces
            .get(&terminal.entity_id().as_u64())?;
        Some(con_core::program_observation::ProgramObservation::new(
            surface.incarnation,
            surface.status.program_snapshot(),
        ))
    }

    pub(super) fn observe_terminal_title(
        &mut self,
        entity: &Entity<GhosttyView>,
        cx: &mut Context<Self>,
    ) {
        let Some(surface) = self
            .terminal_presentation
            .surfaces
            .get_mut(&entity.entity_id().as_u64())
        else {
            // The collector initializes new surfaces on its next pass.
            return;
        };
        surface
            .status
            .observe_title(entity.read(cx).terminal_title.raw(), Instant::now());
        self.refresh_cached_tab_presentation(cx);
    }

    /// Called by the event pump, never by an animation/render callback.
    pub(super) fn refresh_terminal_presentation(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let mut live = HashSet::new();
        let state = &mut self.terminal_presentation;
        for tab in &self.tabs {
            for terminal in tab.pane_tree.all_surface_terminals() {
                let id = terminal.entity_id().as_u64();
                let Some(instance) = terminal.surface_instance(cx) else {
                    continue;
                };
                if state
                    .surfaces
                    .get(&id)
                    .is_some_and(|surface| !surface.instance.ptr_eq(&instance))
                {
                    state.surfaces.remove(&id);
                }
                if !terminal.is_alive(cx) {
                    if let Some(surface) = state.surfaces.get_mut(&id) {
                        let events = surface
                            .instance
                            .upgrade()
                            .map(|terminal| terminal.take_program_events())
                            .unwrap_or_default();
                        apply_program_events(&mut surface.status, events, now);
                        live.insert(id);
                    }
                    continue;
                }
                live.insert(id);
                let query = Query::for_terminal(terminal, cx);
                let surface = state.surfaces.entry(id).or_insert_with(|| Surface {
                    incarnation: uuid::Uuid::new_v4(),
                    instance,
                    query: query.clone(),
                    processes: Vec::new(),
                    identity_process: None,
                    screen_host: None,
                    shell_screen: HashMap::new(),
                    shell_screen_revision: None,
                    revision: 0,
                    detection: AgentCliDetectionState::default(),
                    last_scan: None,
                    status: SurfaceStatus::new(id),
                    #[cfg(target_os = "linux")]
                    host_request: None,
                    #[cfg(target_os = "linux")]
                    host_observed: None,
                });
                if surface.query != query {
                    surface.query = query;
                    surface.processes.clear();
                    surface.revision += 1;
                    surface.detection = AgentCliDetectionState::default();
                    // Invalidate the old job immediately, before its replacement
                    // query finishes. The screen can still contain its banner.
                    surface.status.observe_identity(id, state.sequence, None);
                }
                #[cfg(target_os = "linux")]
                if surface.query == Query::Host {
                    // Host PIDs never enter local /proc queries. Accept only the
                    // outstanding sequence on this exact surface incarnation.
                    let timeout = Duration::from_secs(2);
                    if let Some((sequence, sent)) = surface.host_request {
                        if now.duration_since(sent) >= timeout {
                            surface.host_request = None;
                        } else if let Some(metadata) = surface
                            .instance
                            .upgrade()
                            .and_then(|terminal| terminal.cached_process_metadata())
                            .filter(|metadata| metadata.sequence == sequence)
                        {
                            if surface.host_observed.map(|(group, _)| group)
                                != Some(metadata.process_group_id)
                            {
                                surface.revision += 1;
                                surface.status.observe_identity(id, state.sequence, None);
                            }
                            surface.host_observed = Some((metadata.process_group_id, now));
                            surface.host_request = None;
                            surface.replace_processes(metadata.processes);
                        }
                    }
                    if surface
                        .host_observed
                        .is_some_and(|(_, observed)| now.duration_since(observed) >= timeout)
                    {
                        surface.host_observed = None;
                        surface.processes.clear();
                        surface.revision += 1;
                        surface.status.observe_identity(id, state.sequence, None);
                    }
                }
                let title = terminal.cached_title(cx);
                let title_agent = agent_from_osc_title(title.as_deref());
                let observation = AgentCliObservation {
                    terminal_id: id,
                    foreground_process_group_id: match surface.query {
                        #[cfg(any(target_os = "macos", target_os = "linux"))]
                        Query::Foreground(pgid) => Some(u64::from(pgid)),
                        _ => None,
                    },
                    title_agent,
                    input_generation: terminal.input_generation(cx),
                };
                let detected =
                    surface.resolve_identity(observation, now, || terminal.content_lines(200, cx));
                // A presentation sequence is local to each observation pass;
                // background results separately validate query + incarnation.
                let identity = detected.map(|(agent, scope)| AgentIdentity {
                    agent,
                    scope,
                    generation: surface.revision,
                });
                surface
                    .status
                    .observe_identity(id, state.sequence + 1, identity);
                let events = surface
                    .instance
                    .upgrade()
                    .map(|terminal| terminal.take_program_events())
                    .unwrap_or_default();
                apply_program_events(&mut surface.status, events, now);
                surface.status.observe_title(title.as_deref(), now);
                surface
                    .status
                    .observe_progress(terminal.progress(cx).map(|progress| {
                        use con_core::terminal_status::{Activity, Progress};
                        let (activity, percent) = match progress {
                            TerminalProgress::Running(percent) => (Activity::Busy, percent),
                            TerminalProgress::Indeterminate => (Activity::Busy, None),
                            TerminalProgress::Error(percent) => (Activity::Error, percent),
                            TerminalProgress::Paused(percent) => (Activity::Paused, percent),
                        };
                        Progress { activity, percent }
                    }));
            }
        }
        state.sequence += 2;
        state.surfaces.retain(|id, _| live.contains(id));
        self.refresh_cached_tab_presentation(cx);
    }

    /// Reaggregate retained facts after focus/layout events, without terminal
    /// reads, screen scans or process queries in GPUI's render path.
    pub(super) fn refresh_cached_tab_presentation(&mut self, cx: &mut Context<Self>) {
        let now = Instant::now();
        let state = &mut self.terminal_presentation;
        let focus = self.tabs.get(self.active_tab).map(|tab| {
            (
                tab.summary_id,
                tab.pane_tree.focused_pane_id(),
                tab.pane_tree
                    .focused_terminal_entity_id()
                    .map(|id| id.as_u64()),
            )
        });
        let mut changed = state.presented_focus != focus;
        state.presented_focus = focus;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let terminals = tab.pane_tree.all_surface_terminals();
            for terminal in &terminals {
                let icon = state.surfaces.get(&terminal.entity_id().as_u64()).map_or(
                    crate::terminal_identity::TERMINAL,
                    |surface| {
                        surface
                            .status
                            .identity()
                            .and_then(|identity| agent_cli_icon(Some(identity.agent)))
                            .unwrap_or_else(|| {
                                crate::terminal_identity::from_processes(
                                    surface
                                        .processes
                                        .iter()
                                        .map(|process| process.identity.name.as_str()),
                                )
                            })
                    },
                );
                changed |= terminal.set_presentation_icon(icon);
            }
            let pane_details = if tab.pane_tree.pane_count() > 1 {
                collect_pane_hover_details(&tab.pane_tree, &state.surfaces, now, cx)
            } else {
                Vec::new()
            };
            changed |=
                retain_pane_hover_details(&mut state.pane_details, tab.summary_id, pane_details);
            let focused = tab
                .pane_tree
                .focused_terminal_entity_id()
                .map(|id| id.as_u64());
            let mut statuses: Vec<_> = terminals
                .iter()
                .filter_map(|terminal| state.surfaces.get(&terminal.entity_id().as_u64()))
                .filter_map(|surface| surface.status.status(now))
                .collect();
            let focused_agent = focused
                .and_then(|id| state.surfaces.get(&id))
                .and_then(|surface| surface.status.identity())
                .map(|identity| identity.agent);
            if tab.agent_cli != focused_agent {
                tab.agent_cli = focused_agent;
                changed = true;
            }
            let activity = if index == self.active_tab {
                self.agent_panel.read(cx).state().activity()
            } else {
                tab.panel_state.activity()
            };
            if activity > con_core::terminal_status::Activity::Idle {
                statuses.push(con_core::terminal_status::Status {
                    // GPUI entity IDs are nonzero. The built-in session is not
                    // a terminal surface and never wins the focused-pane tie.
                    surface_id: 0,
                    activity,
                    evidence: con_core::terminal_status::Evidence::BuiltinAgent,
                    percent: None,
                });
            }
            let status = con_core::terminal_status::aggregate(statuses, focused);
            let detail = status
                .filter(|status| {
                    status.evidence == con_core::terminal_status::Evidence::ProgramStatus
                })
                .and_then(|status| {
                    let detail = state
                        .surfaces
                        .get(&status.surface_id)?
                        .status
                        .program_detail_parts()?;
                    let sources = tab.pane_tree.surface_infos(None);
                    let source = sources
                        .into_iter()
                        .find(|source| source.terminal.entity_id().as_u64() == status.surface_id)?;
                    Some(crate::sidebar::ProgramStatusDetail {
                        content: detail,

                        source_path: super::tab_presentation::cwd_subtitle(
                            source.terminal.current_dir(cx).as_deref(),
                        ),
                    })
                });
            changed |= retain_program_detail(&mut state.program_details, tab.summary_id, detail);
            if state.tabs.get(&tab.summary_id) != Some(&status) {
                state.tabs.insert(tab.summary_id, status);
                changed = true;
            }
        }
        state
            .tabs
            .retain(|id, _| self.tabs.iter().any(|tab| tab.summary_id == *id));
        state
            .program_details
            .retain(|id, _| self.tabs.iter().any(|tab| tab.summary_id == *id));
        state
            .pane_details
            .retain(|id, _| self.tabs.iter().any(|tab| tab.summary_id == *id));
        if changed {
            self.sync_sidebar(cx);
            cx.notify();
        }
    }

    /// The winning program-status record, when that record owns the tab
    /// indicator. Heuristic activity does not borrow this text.
    pub(super) fn program_status_detail(&self, summary_id: u64) -> Option<String> {
        self.program_status_context(summary_id)
            .map(|detail| detail.content.to_text())
    }

    pub(super) fn program_status_context(
        &self,
        summary_id: u64,
    ) -> Option<crate::sidebar::ProgramStatusDetail> {
        self.terminal_presentation
            .program_details
            .get(&summary_id)
            .cloned()
            .flatten()
    }

    pub(super) fn refresh_agent_cli_detection(&mut self, cx: &mut Context<Self>) {
        self.refresh_terminal_presentation(cx);
        let state = &mut self.terminal_presentation;
        let now = Instant::now();
        if state.in_flight
            || state
                .last_query
                .is_some_and(|last| now.duration_since(last) < Duration::from_millis(300))
        {
            return;
        }
        #[cfg(target_os = "linux")]
        for surface in state.surfaces.values_mut() {
            if surface.query == Query::Host
                && surface.host_request.is_none()
                && surface
                    .instance
                    .upgrade()
                    .is_some_and(|terminal| terminal.request_process_metadata(state.sequence))
            {
                surface.host_request = Some((state.sequence, now));
            }
        }
        let requests: Vec<_> = state
            .surfaces
            .iter()
            .map(|(id, surface)| {
                (
                    *id,
                    surface.instance.clone(),
                    surface.query.clone(),
                    surface.revision,
                )
            })
            .collect();
        if requests.is_empty() {
            return;
        }
        state.in_flight = true;
        state.last_query = Some(now);
        log::trace!(target: "con::activity", "process_batch surfaces={}", requests.len());
        let queries: Vec<_> = requests
            .iter()
            .map(|(_, _, query, _)| query.clone())
            .collect();
        let task = cx
            .background_executor()
            .spawn(async move { Query::collect_batch(&queries) });
        cx.spawn(async move |this, cx| {
            let results = task.await;
            let _ = this.update(cx, |workspace, cx| {
                let state = &mut workspace.terminal_presentation;
                state.in_flight = false;
                for ((id, instance, query, revision), processes) in
                    requests.into_iter().zip(results)
                {
                    let Some(surface) = state.surfaces.get_mut(&id) else {
                        continue;
                    };
                    #[cfg(target_os = "linux")]
                    if query == Query::Host {
                        continue;
                    }
                    surface.accept_processes(&instance, &query, revision, processes);
                }
                workspace.refresh_terminal_presentation(cx);
            });
        })
        .detach();
    }
}

fn apply_program_events(status: &mut SurfaceStatus, events: Vec<ProgramStatusEvent>, now: Instant) {
    for event in events {
        match event {
            ProgramStatusEvent::Snapshot(program) => status.replace_program_status(program),
            ProgramStatusEvent::Report {
                state,
                kind,
                progress,
                id,
                app,
                title,
                message,
            } => {
                if state == 5 {
                    let _ = status.clear_program_status(&id, now);
                    continue;
                }
                let Some(state) = program_state(state) else {
                    continue;
                };
                let _ = status.observe_program_status(
                    Incoming {
                        state,
                        id: &id,
                        kind: program_kind(kind),
                        progress: (progress >= 0).then_some(progress as u8),
                        app: nonempty(&app),
                        title: nonempty(&title),
                        message: nonempty(&message),
                    },
                    now,
                );
            }
            ProgramStatusEvent::Prompt => {
                status.program_status_prompt(now);
            }
            ProgramStatusEvent::ProcessExit => {
                status.program_status_process_exit(now);
            }
            ProgramStatusEvent::FullReset => {
                status.program_status_full_reset(now);
            }
            ProgramStatusEvent::Key => {
                status.acknowledge_program_status(now);
            }
        }
    }
}

fn program_state(state: i32) -> Option<program_status::State> {
    Some(match state {
        0 => program_status::State::Idle,
        1 => program_status::State::Working,
        2 => program_status::State::Done,
        3 => program_status::State::Blocked,
        4 => program_status::State::Error,
        _ => return None,
    })
}

fn program_kind(kind: i32) -> Option<program_status::BlockedKind> {
    Some(match kind {
        1 => program_status::BlockedKind::Permission,
        2 => program_status::BlockedKind::Question,
        3 => program_status::BlockedKind::Auth,
        _ => return None,
    })
}

fn nonempty(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}

fn collect_pane_hover_details(
    tree: &crate::pane_tree::PaneTree,
    surfaces: &HashMap<u64, Surface>,
    now: Instant,
    cx: &App,
) -> Vec<crate::sidebar::PaneHoverDetail> {
    let sources = tree.surface_infos(None);
    tree.pane_ids()
        .into_iter()
        .filter_map(|pane_id| {
            let pane_sources: Vec<_> = sources
                .iter()
                .filter(|source| source.pane_id == pane_id)
                .collect();
            let active = pane_sources.iter().find(|source| source.is_active).copied();
            if let Some(active) = active {
                let status = con_core::terminal_status::aggregate(
                    pane_sources.iter().filter_map(|source| {
                        surfaces
                            .get(&source.terminal.entity_id().as_u64())
                            .and_then(|surface| surface.status.status(now))
                    }),
                    Some(active.terminal.entity_id().as_u64()),
                );
                let source = status
                    .and_then(|status| {
                        pane_sources
                            .iter()
                            .find(|source| {
                                source.terminal.entity_id().as_u64() == status.surface_id
                            })
                            .copied()
                    })
                    .unwrap_or(active);
                let retained = surfaces.get(&source.terminal.entity_id().as_u64());
                let detail = status
                    .filter(|status| {
                        status.evidence == con_core::terminal_status::Evidence::ProgramStatus
                    })
                    .and_then(|_| retained?.status.program_detail_parts());
                let application = detail
                    .as_ref()
                    .and_then(|detail| detail.application.as_deref())
                    .or_else(|| retained?.status.identity().map(|identity| identity.agent));
                let icon = crate::assets::agent_cli_icon(application)
                    .unwrap_or_else(|| source.terminal.presentation_icon());
                let directory = super::tab_presentation::cwd_subtitle(
                    source.terminal.current_dir(cx).as_deref(),
                );
                let title = detail
                    .as_ref()
                    .and_then(|detail| detail.context.clone())
                    .or_else(|| application.map(str::to_owned))
                    .or_else(|| icon.strip_prefix("program:").map(str::to_owned))
                    .unwrap_or_else(|| {
                        directory
                            .as_deref()
                            .map(|dir| {
                                dir.trim_end_matches(['/', '\\'])
                                    .rsplit(['/', '\\'])
                                    .next()
                                    .filter(|name| !name.is_empty())
                                    .unwrap_or(dir)
                                    .to_string()
                            })
                            .unwrap_or_else(|| "Terminal".into())
                    });
                let status_label = detail.map(|detail| detail.status).unwrap_or_else(|| {
                    status.map_or("Terminal".into(), |status| format!("{:?}", status.activity))
                });
                Some(crate::sidebar::PaneHoverDetail {
                    id: pane_id as u64,
                    icon,
                    title,
                    directory,
                    status,
                    status_label,
                })
            } else {
                let editor = tree.editor_view_for_pane(pane_id)?;
                let path = editor
                    .read(cx)
                    .active_path()
                    .map(std::path::Path::to_path_buf);
                let title = path
                    .as_deref()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Editor".into());
                let directory = path
                    .as_deref()
                    .and_then(|path| path.parent())
                    .and_then(|path| super::tab_presentation::cwd_subtitle(path.to_str()));
                Some(crate::sidebar::PaneHoverDetail {
                    id: pane_id as u64,
                    icon: "phosphor/file-code.svg",
                    title,
                    directory,
                    status: None,
                    status_label: "Editor".into(),
                })
            }
        })
        .collect()
}

fn retain_pane_hover_details(
    details: &mut HashMap<u64, Vec<crate::sidebar::PaneHoverDetail>>,
    tab: u64,
    next: Vec<crate::sidebar::PaneHoverDetail>,
) -> bool {
    if details.get(&tab) == Some(&next) {
        return false;
    }
    details.insert(tab, next);
    true
}

fn retain_program_detail(
    details: &mut HashMap<u64, Option<crate::sidebar::ProgramStatusDetail>>,
    tab: u64,
    detail: Option<crate::sidebar::ProgramStatusDetail>,
) -> bool {
    if details.get(&tab) == Some(&detail) {
        return false;
    }
    details.insert(tab, detail);
    true
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Instant;

    use con_ghostty::ProgramStatusEvent;

    #[test]
    fn pane_details_update_when_the_tabs_aggregate_activity_does_not() {
        use con_core::terminal_status::{Activity, Evidence, Status, aggregate};
        let row = |id| crate::sidebar::PaneHoverDetail {
            id,
            icon: "agents/codex.svg",
            title: format!("task-{id}"),
            directory: Some(format!("/project/{id}")),
            status: Some(Status {
                surface_id: id,
                activity: Activity::Busy,
                evidence: Evidence::ProgramStatus,
                percent: None,
            }),
            status_label: "Working".into(),
        };
        let mut rows = vec![row(1), row(2)];
        let before = aggregate(rows.iter().filter_map(|row| row.status), Some(1))
            .unwrap()
            .activity;
        let mut retained = HashMap::new();
        assert!(super::retain_pane_hover_details(
            &mut retained,
            7,
            rows.clone()
        ));
        rows[1].status.as_mut().unwrap().activity = Activity::Done;
        rows[1].status_label = "Completed".into();
        assert_eq!(
            before,
            aggregate(rows.iter().filter_map(|row| row.status), Some(1))
                .unwrap()
                .activity
        );
        assert!(super::retain_pane_hover_details(
            &mut retained,
            7,
            rows.clone()
        ));
        assert_eq!(retained[&7][1].status.unwrap().activity, Activity::Done);
        assert!(!super::retain_pane_hover_details(&mut retained, 7, rows));
    }

    #[test]
    fn program_detail_changes_invalidate_without_activity_changes() {
        let detail = |context: &str, other_directory| crate::sidebar::ProgramStatusDetail {
            content: con_core::program_status::ProgramDetail {
                application: Some("cargo".into()),
                scope: None,
                status: "Working".into(),
                context: Some(context.into()),
            },

            source_path: Some(
                if other_directory {
                    "/project/other"
                } else {
                    "/project"
                }
                .into(),
            ),
        };
        let mut details = std::collections::HashMap::new();
        assert!(super::retain_program_detail(
            &mut details,
            7,
            Some(detail("building", false))
        ));
        assert!(!super::retain_program_detail(
            &mut details,
            7,
            Some(detail("building", false))
        ));
        assert!(super::retain_program_detail(
            &mut details,
            7,
            Some(detail("testing", false))
        ));
        assert_eq!(
            details[&7].as_ref().unwrap().content.context.as_deref(),
            Some("testing")
        );
        assert!(super::retain_program_detail(
            &mut details,
            7,
            Some(detail("testing", true))
        ));
        assert!(super::retain_program_detail(&mut details, 7, None));
        assert!(!super::retain_program_detail(&mut details, 7, None));
    }
    use super::{
        AgentCliDetectionState, AgentIdentity, IdentityScope, ProcessInfo, Query, Surface,
        SurfaceStatus, Weak, apply_program_events, native_agent,
    };

    fn process(pid: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            identity: con_ghostty::process::ProcessIdentity {
                pid,
                started_at: 100,
                executable: name.into(),
                name: name.into(),
            },
            parent_pid: 1,
            process_group_id: Some(10),
        }
    }

    #[test]
    fn pipelines_require_consensus_and_preserve_the_selected_process() {
        let previous = process(8, "claude");
        let mut group = vec![process(2, "claude"), process(5, "tee"), previous.clone()];
        assert_eq!(
            native_agent(&group, Some(&previous.identity)),
            Some(("claude", &previous.identity))
        );
        group.push(process(9, "codex"));
        assert_eq!(native_agent(&group, Some(&previous.identity)), None);
        assert_eq!(native_agent(&[process(4, "node")], None), None);
    }

    #[test]
    fn native_claude_versions_are_recognized_without_matching_arbitrary_versions() {
        for (path, expected) in [
            (
                "/home/me/.local/share/claude/versions/2.1.283",
                Some("claude"),
            ),
            ("/opt/other/versions/2.1.283", None),
            ("/opt/claude/versions/not-a-version", None),
            ("/opt/claude/versions/2..283", None),
        ] {
            let mut candidate = process(4, "2.1.283");
            candidate.identity.executable = path.into();
            assert_eq!(
                native_agent(&[candidate], None).map(|(agent, _)| agent),
                expected
            );
        }
    }

    #[test]
    fn process_completion_rejects_aba_revision_but_accepts_current_work() {
        let instance = Weak::new();
        let mut surface = Surface {
            incarnation: uuid::Uuid::new_v4(),
            instance: instance.clone(),
            query: Query::Unavailable,
            processes: vec![process(9, "codex")],
            identity_process: None,
            screen_host: None,
            shell_screen: HashMap::new(),
            shell_screen_revision: None,
            revision: 3,
            detection: AgentCliDetectionState::default(),
            last_scan: None,
            status: SurfaceStatus::new(7),
            #[cfg(target_os = "linux")]
            host_request: None,
            #[cfg(target_os = "linux")]
            host_observed: None,
        };
        // Query and surface pointer match again, but this work predates A→B→A.
        surface.accept_processes(
            &instance,
            &Query::Unavailable,
            1,
            vec![process(8, "claude")],
        );
        assert_eq!(surface.processes, vec![process(9, "codex")]);
        surface.accept_processes(
            &instance,
            &Query::Unavailable,
            3,
            vec![process(8, "claude")],
        );
        assert_eq!(surface.processes, vec![process(8, "claude")]);
        surface.detection.finish();
        surface.replace_processes(vec![process(8, "claude"), process(20, "rustc")]);
        assert!(surface.detection.is_exhausted());
        surface.replace_processes(vec![process(8, "claude"), process(21, "rustc")]);
        assert!(surface.detection.is_exhausted());
        surface.replace_processes(vec![process(22, "codex")]);
        assert!(surface.detection.observe(super::AgentCliObservation {
            terminal_id: 7,
            foreground_process_group_id: None,
            title_agent: None,
            input_generation: 0,
        }));
        assert!(!surface.detection.is_exhausted());
    }

    fn surface(processes: Vec<ProcessInfo>) -> Surface {
        Surface {
            incarnation: uuid::Uuid::new_v4(),
            instance: Weak::new(),
            query: Query::Unavailable,
            processes,
            identity_process: None,
            screen_host: None,
            shell_screen: HashMap::new(),
            shell_screen_revision: None,
            revision: 0,
            detection: AgentCliDetectionState::default(),
            last_scan: None,
            status: SurfaceStatus::new(7),
            #[cfg(target_os = "linux")]
            host_request: None,
            #[cfg(target_os = "linux")]
            host_observed: None,
        }
    }

    /// One observation pass. `step` is the input generation: a new value
    /// reopens the screen-scan window as typing or chat output would.
    fn pass(
        surface: &mut Surface,
        step: u64,
        title_agent: Option<&'static str>,
        screen: &[&str],
    ) -> Option<(&'static str, IdentityScope)> {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let detected = surface.resolve_identity(
            super::AgentCliObservation {
                terminal_id: 7,
                foreground_process_group_id: None,
                title_agent,
                input_generation: step,
            },
            Instant::now(),
            || screen.iter().map(|line| line.to_string()).collect(),
        );
        surface.status.observe_identity(
            7,
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            detected.map(|(agent, scope)| AgentIdentity {
                agent,
                scope,
                generation: surface.revision,
            }),
        );
        detected
    }

    fn agent(detected: Option<(&'static str, IdentityScope)>) -> Option<&'static str> {
        detected.map(|(agent, _)| agent)
    }

    #[test]
    fn brand_text_in_chat_does_not_replace_a_confirmed_screen_identity() {
        let mut surface = surface(vec![process(4, "node")]);
        assert_eq!(
            pass(&mut surface, 1, None, &["Gemini CLI v0.9"]),
            Some(("gemini", IdentityScope::Screen))
        );
        // Chat, code and tool logs later mention other products.
        for (step, mention) in [(2, "Claude Code"), (3, "OpenAI Codex"), (4, "Kimi Code")] {
            assert_eq!(
                pass(
                    &mut surface,
                    step,
                    None,
                    &[&format!("compare with {mention}")]
                ),
                Some(("gemini", IdentityScope::Screen))
            );
        }
        // Screen-text misses do not drop the confirmed identity either.
        assert_eq!(
            pass(&mut surface, 5, None, &[]),
            Some(("gemini", IdentityScope::Screen))
        );
    }

    #[test]
    fn brand_text_cannot_name_unknown_or_non_interpreter_processes() {
        // Process evidence unavailable: the screen still shows an old banner.
        let mut unavailable = surface(Vec::new());
        assert_eq!(pass(&mut unavailable, 1, None, &["Claude Code"]), None);
        for name in ["vim", "less", "git", "cargo"] {
            let mut viewer = surface(vec![process(4, name)]);
            assert_eq!(pass(&mut viewer, 1, None, &["Claude Code"]), None, "{name}");
        }
        let mut native = surface(vec![process(4, "codex")]);
        assert_eq!(
            agent(pass(&mut native, 1, None, &["Claude Code"])),
            Some("codex")
        );
        let mut titled = surface(vec![process(4, "node")]);
        assert_eq!(
            pass(&mut titled, 1, Some("qwen"), &["Claude Code"]),
            Some(("qwen", IdentityScope::Title))
        );
        for name in ["python3", "python3.12", "Python", "node.exe", "bun"] {
            assert!(super::process_name_is_interpreter(name), "{name}");
        }
        for name in ["pythonista", "nodemon", "zsh"] {
            assert!(!super::process_name_is_interpreter(name), "{name}");
        }
    }

    #[test]
    fn native_agents_do_not_need_screen_text_fallback() {
        for name in ["qoder", "hermes", "cline", "qwen", "kilo"] {
            let mut surface = surface(vec![process(4, name)]);
            assert_eq!(
                agent(pass(&mut surface, 1, None, &["OpenAI Codex"])),
                Some(name),
                "{name}"
            );
        }
    }

    #[test]
    fn shell_tree_captures_residue_between_interpreter_runs() {
        let mut surface = surface(vec![process(3, "pwsh.exe")]);
        assert_eq!(pass(&mut surface, 1, None, &[]), None);
        surface.replace_processes(vec![process(3, "pwsh.exe"), process(4, "node.exe")]);
        assert_eq!(
            agent(pass(&mut surface, 2, None, &["Gemini CLI"])),
            Some("gemini")
        );
        surface.replace_processes(vec![process(3, "pwsh.exe")]);
        assert_eq!(pass(&mut surface, 3, Some("qwen"), &["Gemini CLI"]), None);
        surface.replace_processes(vec![process(3, "pwsh.exe"), process(5, "node.exe")]);
        assert_eq!(pass(&mut surface, 4, None, &["Gemini CLI"]), None);
    }

    #[test]
    fn confirmed_identity_does_not_read_screen_again() {
        let mut surface = surface(vec![process(4, "node")]);
        assert_eq!(
            agent(pass(&mut surface, 1, None, &["Gemini CLI"])),
            Some("gemini")
        );
        assert_eq!(
            surface.resolve_identity(
                super::AgentCliObservation {
                    terminal_id: 7,
                    foreground_process_group_id: None,
                    title_agent: None,
                    input_generation: 2,
                },
                Instant::now(),
                || panic!("a confirmed host must not rescan terminal content"),
            ),
            Some(("gemini", IdentityScope::Screen))
        );
    }

    #[test]
    fn switching_programs_reidentifies_and_returning_to_shell_clears() {
        let mut surface = surface(vec![process(4, "node")]);
        assert_eq!(
            agent(pass(&mut surface, 1, None, &["Gemini CLI"])),
            Some("gemini")
        );
        // Unrelated interpreter children keep the original host.
        surface.replace_processes(vec![process(4, "node"), process(5, "node")]);
        assert_eq!(
            agent(pass(&mut surface, 2, None, &["Claude Code"])),
            Some("gemini")
        );
        // A different interpreter process is a new program.
        surface.replace_processes(vec![process(9, "node")]);
        assert_eq!(
            agent(pass(&mut surface, 3, None, &["Cline needs permission"])),
            Some("cline")
        );
        // A native agent replaces the screen identity.
        surface.replace_processes(vec![process(10, "claude")]);
        assert_eq!(
            agent(pass(&mut surface, 4, None, &["Cline needs permission"])),
            Some("claude")
        );
        // Back at the shell, with the old banner still visible.
        surface.replace_processes(vec![process(11, "zsh")]);
        assert_eq!(pass(&mut surface, 5, None, &["Claude Code"]), None);
        assert_eq!(surface.status.identity(), None);
        // A query in flight after a job change has no processes yet.
        surface.replace_processes(vec![process(12, "node")]);
        assert!(pass(&mut surface, 6, None, &["Gemini CLI"]).is_some());
        surface.processes.clear();
        assert_eq!(pass(&mut surface, 7, None, &["Gemini CLI"]), None);
    }

    #[test]
    fn output_left_at_the_shell_does_not_name_the_next_program() {
        let residue = "notes: compare OpenAI Codex and Claude Code and Kimi Code";
        let mut surface = surface(vec![process(3, "zsh")]);
        assert_eq!(pass(&mut surface, 1, None, &[residue, "$ gemini"]), None);
        // Gemini's banner is ASCII art: no brand text of its own.
        surface.replace_processes(vec![process(4, "node")]);
        let banner = [residue, "$ gemini", "> GEMINI", "Tips for getting started:"];
        assert_eq!(pass(&mut surface, 2, None, &banner), None);
        // Lines the program paints itself still identify it.
        let prompt = [residue, "$ gemini", "Cline needs permission to run"];
        assert_eq!(agent(pass(&mut surface, 3, None, &prompt)), Some("cline"));

        // Rerunning the same agent while its old banner is still visible.
        surface.replace_processes(vec![process(5, "zsh")]);
        let old = ["Gemini CLI v0.9", "$ gemini"];
        assert_eq!(pass(&mut surface, 4, None, &old), None);
        surface.replace_processes(vec![process(6, "node")]);
        let rerun = ["Gemini CLI v0.9", "$ gemini", "Gemini CLI v0.9"];
        assert_eq!(agent(pass(&mut surface, 5, None, &rerun)), Some("gemini"));
    }

    #[test]
    fn output_written_without_typing_is_captured_when_the_next_job_starts() {
        let residue = "notes: compare OpenAI Codex and Claude Code";
        let mut surface = surface(vec![process(3, "zsh")]);
        assert_eq!(pass(&mut surface, 1, None, &[]), None);
        // `echo` output and `gemini` arrive through the PTY, not the keyboard,
        // so the input generation stays the same. The foreground job changes.
        surface.processes.clear();
        surface.revision += 1;
        assert_eq!(pass(&mut surface, 1, None, &[residue, "$ gemini"]), None);
        surface.replace_processes(vec![process(4, "node")]);
        let banner = [residue, "$ gemini", "> GEMINI"];
        assert_eq!(pass(&mut surface, 1, None, &banner), None);
    }

    #[test]
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn batch_keeps_unavailable_slots_and_duplicate_jobs_in_order() {
        let process = con_ghostty::process::read_process(std::process::id()).unwrap();
        let pgid = process.process_group_id.unwrap();
        let results = Query::collect_batch(&[
            Query::Unavailable,
            Query::Foreground(pgid),
            Query::Foreground(pgid),
        ]);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_empty());
        assert!(results[1].contains(&process));
        assert!(results[2].contains(&process));
    }

    #[test]
    fn program_events_follow_the_protocol_lifetime() {
        let now = Instant::now();
        let mut status = SurfaceStatus::new(3);
        apply_program_events(
            &mut status,
            vec![ProgramStatusEvent::Report {
                state: 1,
                kind: 0,
                progress: 40,
                id: "build".into(),
                app: String::new(),
                title: String::new(),
                message: String::new(),
            }],
            now,
        );
        assert_eq!(
            status.status(now).map(|status| status.activity),
            Some(con_core::terminal_status::Activity::Busy)
        );
        apply_program_events(&mut status, vec![ProgramStatusEvent::Prompt], now);
        assert_eq!(status.status(now), None);
        apply_program_events(
            &mut status,
            vec![ProgramStatusEvent::Report {
                state: 2,
                kind: 0,
                progress: -1,
                id: String::new(),
                app: String::new(),
                title: String::new(),
                message: "ready".into(),
            }],
            now,
        );
        assert_eq!(
            status.status(now).map(|status| status.evidence),
            Some(con_core::terminal_status::Evidence::ProgramStatus)
        );
        apply_program_events(&mut status, vec![ProgramStatusEvent::Key], now);
        let acknowledged = status.status(now).expect("acknowledged root stays idle");
        assert_eq!(
            acknowledged.activity,
            con_core::terminal_status::Activity::Idle
        );
        assert_eq!(
            acknowledged.evidence,
            con_core::terminal_status::Evidence::ProgramStatus
        );
        apply_program_events(&mut status, vec![ProgramStatusEvent::FullReset], now);
        assert_eq!(status.program_records(), Vec::new());
    }

    #[test]
    fn program_events_cover_attention_then_completion() {
        let now = Instant::now();
        let mut status = SurfaceStatus::new(3);
        let report = |state, kind, progress, id: &str, message: &str| ProgramStatusEvent::Report {
            state,
            kind,
            progress,
            id: id.into(),
            app: "cargo".into(),
            title: String::new(),
            message: message.into(),
        };
        apply_program_events(&mut status, vec![report(1, 0, 10, "build", "")], now);
        assert_eq!(
            status.status(now).map(|status| status.activity),
            Some(con_core::terminal_status::Activity::Busy)
        );
        apply_program_events(&mut status, vec![report(3, 1, 40, "build", "Apply?")], now);
        assert_eq!(
            status.status(now).map(|status| status.activity),
            Some(con_core::terminal_status::Activity::NeedsInput)
        );
        assert!(
            status
                .program_detail()
                .unwrap()
                .contains("Waiting for permission")
        );
        apply_program_events(&mut status, vec![report(1, 0, 80, "build", "")], now);
        assert_eq!(
            status.status(now).map(|status| status.activity),
            Some(con_core::terminal_status::Activity::Busy)
        );
        apply_program_events(&mut status, vec![report(2, 0, -1, "build", "ready")], now);
        assert_eq!(
            status.status(now).map(|status| status.activity),
            Some(con_core::terminal_status::Activity::Done)
        );
        apply_program_events(&mut status, vec![ProgramStatusEvent::Key], now);
        assert_eq!(status.status(now), None);
        assert!(!status.program_records()[0].unseen);
        apply_program_events(
            &mut status,
            vec![report(4, 0, -1, "build/test", "failed")],
            now,
        );
        assert_eq!(
            status.status(now).map(|status| status.activity),
            Some(con_core::terminal_status::Activity::Error)
        );
        apply_program_events(&mut status, vec![ProgramStatusEvent::ProcessExit], now);
        assert_eq!(
            status.status(now).map(|status| status.activity),
            Some(con_core::terminal_status::Activity::Error)
        );
        assert!(!status.program_status_alternate_screen(true, now));
        assert_eq!(status.program_records().len(), 2);
    }

    #[test]
    fn program_status_burst_keeps_one_record_and_latest_progress() {
        const UPDATES: u32 = 8_000;
        let now = Instant::now();
        let mut surface = SurfaceStatus::new(1);
        let mut redraws = 0u32;
        for step in 0..UPDATES {
            if surface
                .observe_program_status(
                    con_core::program_status::Incoming {
                        state: con_core::program_status::State::Working,
                        id: "build",
                        kind: None,
                        progress: Some((step % 101) as u8),
                        app: Some("cargo"),
                        title: None,
                        message: None,
                    },
                    now,
                )
                .unwrap()
            {
                redraws += 1;
            }
            let status = surface.status(now).unwrap();
            assert_eq!(status.surface_id, 1);
            assert_eq!(status.activity, con_core::terminal_status::Activity::Busy);
            assert_eq!(status.percent, Some((step % 101) as u8));
        }
        assert_eq!(surface.program_records().len(), 1);
        assert_eq!(redraws, UPDATES);
        assert!(
            !surface
                .observe_program_status(
                    con_core::program_status::Incoming {
                        state: con_core::program_status::State::Working,
                        id: "build",
                        kind: None,
                        progress: Some(((UPDATES - 1) % 101) as u8),
                        app: Some("cargo"),
                        title: None,
                        message: None,
                    },
                    now,
                )
                .unwrap()
        );
        assert_eq!(surface.program_records().len(), 1);
    }
}
