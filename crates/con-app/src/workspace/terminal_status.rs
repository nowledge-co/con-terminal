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
    instance: Weak<GhosttyTerminal>,
    query: Query,
    processes: Vec<ProcessInfo>,
    identity_process: Option<con_ghostty::process::ProcessIdentity>,
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
    program_details: HashMap<u64, Option<String>>,
    in_flight: bool,
    last_query: Option<Instant>,
    sequence: u64,
    presented_focus: Option<(u64, usize, Option<u64>)>,
}

impl ConWorkspace {
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
                let query = Query::for_terminal(&terminal, cx);
                let surface = state.surfaces.entry(id).or_insert_with(|| Surface {
                    instance,
                    query: query.clone(),
                    processes: Vec::new(),
                    identity_process: None,
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
                let observation_changed = surface.detection.observe(observation);
                if observation_changed {
                    surface.last_scan = None;
                }
                let shell_foreground = !cfg!(target_os = "windows")
                    && !surface.processes.is_empty()
                    && surface
                        .processes
                        .iter()
                        .all(|process| process_name_is_shell(&process.identity.name));
                let direct = native_agent(&surface.processes, surface.identity_process.as_ref());
                let identity_process = direct.map(|(_, identity)| identity.clone());
                if surface.identity_process != identity_process {
                    surface.identity_process = identity_process;
                    surface.revision += 1;
                }
                let scope = if cfg!(target_os = "windows") {
                    IdentityScope::ProcessTree
                } else {
                    IdentityScope::ForegroundJob
                };
                let mut screen_hit = false;
                let detected = if shell_foreground {
                    None
                } else if let Some((agent, _)) = direct {
                    Some((agent, scope))
                } else if let Some(agent) = title_agent {
                    Some((agent, IdentityScope::Title))
                } else if (observation_changed || !surface.detection.is_exhausted())
                    && surface.last_scan.is_none_or(|last| {
                        now.duration_since(last) >= surface.detection.scan_interval()
                    })
                    && surface.detection.take_screen_scan_attempt()
                {
                    surface.last_scan = Some(now);
                    log::trace!(target: "con::activity", "identity_screen_scan surface={id}");
                    let lines = terminal.content_lines(200, cx);
                    let agent =
                        con_agent::context::classify_screen_agent_cli(title.as_deref(), &lines)
                            .or_else(|| agent_from_screen_text(&lines));
                    screen_hit = agent.is_some();
                    agent
                        .map(|agent| (agent, IdentityScope::Screen))
                        .or_else(|| {
                            (!surface.detection.is_exhausted())
                                .then(|| surface.status.identity())
                                .flatten()
                                .filter(|identity| identity.scope == IdentityScope::Screen)
                                .map(|identity| (identity.agent, identity.scope))
                        })
                } else {
                    surface
                        .status
                        .identity()
                        .map(|identity| (identity.agent, identity.scope))
                };
                if direct.is_some() || shell_foreground || title_agent.is_some() || screen_hit {
                    surface.detection.finish();
                }
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
            let focused = tab
                .pane_tree
                .focused_terminal_entity_id()
                .map(|id| id.as_u64());
            let mut statuses: Vec<_> = tab
                .pane_tree
                .all_surface_terminals()
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
                        .program_detail()?;
                    let source = tab
                        .pane_tree
                        .surface_infos(None)
                        .into_iter()
                        .find(|source| source.terminal.entity_id().as_u64() == status.surface_id)?;
                    Some(program_detail_with_source(
                        source.pane_index,
                        source.surface_index,
                        &detail,
                    ))
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
        if changed {
            self.sync_sidebar(cx);
            cx.notify();
        }
    }

    /// The winning program-status record, when that record owns the tab
    /// indicator. Heuristic activity does not borrow this text.
    pub(super) fn program_status_detail(&self, summary_id: u64) -> Option<String> {
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

// PaneSurfaceInfo already uses the one-based indices shown by the UI/CLI.
fn program_detail_with_source(pane_index: usize, surface_index: usize, detail: &str) -> String {
    format!("{detail}\nPane {pane_index} · Surface {surface_index}")
}

fn retain_program_detail(
    details: &mut HashMap<u64, Option<String>>,
    tab: u64,
    detail: Option<String>,
) -> bool {
    if details.get(&tab) == Some(&detail) {
        return false;
    }
    details.insert(tab, detail);
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn program_detail_preserves_one_based_surface_info_indices() {
        assert_eq!(
            super::program_detail_with_source(2, 1, "cargo · done"),
            "cargo · done\nPane 2 · Surface 1"
        );
    }
    use std::time::Instant;

    use con_ghostty::ProgramStatusEvent;

    #[test]
    fn program_detail_changes_invalidate_without_activity_changes() {
        let mut details = std::collections::HashMap::new();
        assert!(super::retain_program_detail(
            &mut details,
            7,
            Some("building".into())
        ));
        assert!(!super::retain_program_detail(
            &mut details,
            7,
            Some("building".into())
        ));
        assert!(super::retain_program_detail(
            &mut details,
            7,
            Some("testing".into())
        ));
        assert_eq!(details[&7].as_deref(), Some("testing"));
        assert!(super::retain_program_detail(&mut details, 7, None));
        assert!(!super::retain_program_detail(&mut details, 7, None));
    }
    use super::{
        AgentCliDetectionState, ProcessInfo, Query, Surface, SurfaceStatus, Weak,
        apply_program_events, native_agent,
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
            instance: instance.clone(),
            query: Query::Unavailable,
            processes: vec![process(9, "codex")],
            identity_process: None,
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
