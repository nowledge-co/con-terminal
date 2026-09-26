mod binding;
mod control;
mod destination;
mod fallback_observer;
mod kimi_delivery;
mod kimi_delivery_state;
mod kimi_screen;
mod launch_notice;
mod route;

use super::*;
use binding::running_source_agent;
use destination::{ExecuteHandoff, ExistingAgentTab, HandoffDestinationPanel};
use route::cancel_prepared_handoff;

#[derive(Clone, Copy)]
struct SourceAgentTab {
    tab_id: u64,
    terminal_id: EntityId,
    foreground_group: u64,
    agent: con_agent::handoff::AgentKind,
}

fn handoff_agent(name: &str) -> Option<con_agent::handoff::AgentKind> {
    name.parse().ok()
}

fn process_agent(name: &str) -> Option<con_agent::handoff::AgentKind> {
    if name == "Kimi Code" {
        return Some(con_agent::handoff::AgentKind::Kimi);
    }
    agent_from_process_name(name)
        .and_then(handoff_agent)
        .or_else(|| (name == "cursor-agent").then_some(con_agent::handoff::AgentKind::Cursor))
        .or_else(|| handoff_agent(name))
}

/// The launch helper must be a sibling of the running Con executable — the
/// contract executes `con-cli handoff run …` from exactly that layout, and
/// packaged installs plus `just build` both produce it. `EXE_SUFFIX` keeps
/// the lookup correct on Windows (`con-cli.exe`).
fn con_cli_path() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.with_file_name(format!("con-cli{}", std::env::consts::EXE_SUFFIX)))
}

impl ConWorkspace {
    /// The toolbar entry point is only offered while the experimental feature
    /// is on and the active tab has a live terminal running a known Agent CLI.
    #[cfg(target_os = "macos")]
    pub(super) fn handoff_button_visible(&self, cx: &App) -> bool {
        if !self.config.experimental.handoff {
            return false;
        }
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return false;
        };
        if tab.agent_cli.and_then(handoff_agent).is_none() {
            return false;
        }
        tab.pane_tree
            .try_focused_terminal()
            .is_some_and(|terminal| terminal.is_alive(cx))
    }

    /// Push the in-memory experimental gate into every terminal view so the
    /// right-click menu matches the toolbar without reading persisted config.
    #[cfg(target_os = "macos")]
    pub(super) fn sync_handoff_menu_entry(&self, cx: &mut App) {
        let enabled = self.config.experimental.handoff;
        for tab in &self.tabs {
            for terminal in tab.pane_tree.all_surface_terminals() {
                terminal.set_handoff_menu_entry_enabled(enabled, cx);
            }
        }
    }

    pub(super) fn open_agent_handoff(
        &mut self,
        _: &crate::HandoffAgent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cfg!(target_os = "macos") {
            return;
        }
        if !self.config.experimental.handoff {
            self.handoff_info(
                window,
                cx,
                "Experimental feature",
                "Enable Agent Handoff in Settings → Experimental.",
            );
            return;
        }
        if let Some(handle) = self.handoff_window {
            // The panel binds source and job state at open time; reopening
            // fresh is cheaper and safer than revalidating a stale panel.
            let _ = handle.update(cx, |_, window, _| window.remove_window());
            self.handoff_window = None;
        }
        self.refresh_agent_cli_detection(cx);
        let Some(source) = self.try_active_terminal().cloned() else {
            return;
        };
        if !source.is_alive(cx) {
            self.handoff_info(
                window,
                cx,
                "Agent unavailable",
                "Open Handoff from a running Agent Tab.",
            );
            return;
        }
        let (_, runtime) = self.observe_terminal_runtime_for_tab(self.active_tab, &source, 40, cx);
        if runtime.remote_host.is_some() {
            // Informational only: the answer is intentionally not awaited.
            std::mem::drop(window.prompt(
                PromptLevel::Info,
                "Local sessions only",
                Some("Agent Handoff currently supports local agent sessions."),
                &["OK"],
                cx,
            ));
            return;
        }
        let Some(cwd) = source.current_dir(cx).map(PathBuf::from) else {
            // Informational only: the answer is intentionally not awaited.
            std::mem::drop(window.prompt(
                PromptLevel::Info,
                "Working directory unavailable",
                Some("Open the project in a terminal with shell integration first."),
                &["OK"],
                cx,
            ));
            return;
        };
        let Some(group) = source.foreground_process_group_id(cx) else {
            self.handoff_info(
                window,
                cx,
                "Source status unavailable",
                "Cannot identify the running Agent in this Tab.",
            );
            return;
        };
        let Some(process) = crate::process_name::process_name(group) else {
            self.handoff_info(
                window,
                cx,
                "Source status unavailable",
                "Cannot identify the running Agent process.",
            );
            return;
        };
        let detected = self.tabs[self.active_tab].agent_cli.and_then(handoff_agent);
        let agent = running_source_agent(detected, &process, group, &source.content_lines(200, cx));
        let Some(agent) = agent else {
            self.handoff_info(
                window,
                cx,
                "Running Agent required",
                "Open Handoff while the source Agent is running in this Tab.",
            );
            return;
        };
        let canonical_cwd = cwd.canonicalize().unwrap_or(cwd.clone());
        let existing = self.available_handoff_tabs(self.active_tab, &canonical_cwd, cx);
        let source_binding = SourceAgentTab {
            tab_id: self.tabs[self.active_tab].summary_id,
            terminal_id: source.entity_id(),
            foreground_group: group,
            agent,
        };
        let workspace = cx.weak_entity();
        let main_window = window.window_handle();
        let runtime = self.harness.runtime_handle();
        // First-frame estimate only: header, source card, footer and one
        // destination row per existing Agent Tab. The panel re-fits itself
        // to the measured body content on the first prepaint.
        let height = (356.0 + 42.0 * existing.len() as f32).min(560.0);
        let bounds = WindowBounds::centered(size(px(480.0), px(height)), cx);
        let result = cx.open_window(
            WindowOptions {
                window_bounds: Some(bounds),
                titlebar: Some(crate::settings_panel::floating_titlebar_options(
                    "Agent Handoff".into(),
                )),
                window_background: WindowBackgroundAppearance::Opaque,
                ..Default::default()
            },
            move |window, cx| {
                let dialog = window.window_handle();
                let panel = cx.new(|cx| {
                    let mut panel = HandoffDestinationPanel::new(
                        cwd,
                        runtime.clone(),
                        agent,
                        group,
                        source.clone(),
                        existing,
                        cx,
                    );
                    // Closing the window (or app teardown) releases undelivered
                    // Prepared jobs; persisted launch/delivery intents survive.
                    cx.on_release(|panel: &mut HandoffDestinationPanel, _cx| {
                        panel.release_undelivered()
                    })
                    .detach();
                    panel.load(window, cx);
                    panel
                });
                panel.read(cx).focus_handle(cx).focus(window, cx);
                let panel_for_result = panel.clone();
                let panel_for_route = panel.clone();
                cx.subscribe(&panel, move |_, event: &ExecuteHandoff, cx| {
                    let event = event.clone();
                    let job_id = event.job.id.clone();
                    // route_handoff re-validates identity synchronously, then
                    // runs the service/snapshot work on the Tokio runtime and
                    // finishes (closing the dialog) back on the GPUI thread.
                    let launched = main_window.update(cx, |_, _window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            workspace.route_handoff(
                                source_binding,
                                event,
                                &panel_for_route,
                                &dialog,
                                cx,
                            )
                        })
                    });
                    if !matches!(launched, Ok(Ok(Ok(())))) {
                        let error = format!("Handoff needs review: {launched:?}");
                        panel_for_result.update(cx, |panel, cx| panel.report_error(error, cx));
                        cancel_prepared_handoff(&runtime, job_id);
                    }
                })
                .detach();
                cx.new(|cx| gpui_component::Root::new(panel, window, cx).bg(cx.theme().background))
            },
        );
        match result {
            Ok(handle) => self.handoff_window = Some(handle.into()),
            Err(error) => log::error!("Cannot open Agent Handoff: {error}"),
        }
    }

    fn handoff_info(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        title: &str,
        message: &str,
    ) {
        // Informational only: the answer is intentionally not awaited.
        std::mem::drop(window.prompt(PromptLevel::Info, title, Some(message), &["OK"], cx));
    }

    fn available_handoff_tabs(
        &self,
        source_index: usize,
        cwd: &Path,
        cx: &App,
    ) -> Vec<ExistingAgentTab> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                if index == source_index {
                    return None;
                }
                let terminal = tab.pane_tree.try_focused_terminal()?;
                if !terminal.is_alive(cx) {
                    return None;
                }
                let group = terminal.foreground_process_group_id(cx)?;
                let process = crate::process_name::process_name(group)?;
                if process_name_is_shell(&process) {
                    return None;
                }
                let detected = tab.agent_cli.and_then(handoff_agent);
                let agent = running_source_agent(
                    detected,
                    &process,
                    group,
                    &terminal.content_lines(200, cx),
                )?;
                let target_cwd = PathBuf::from(terminal.current_dir(cx)?)
                    .canonicalize()
                    .ok()?;
                if target_cwd != cwd {
                    return None;
                }
                Some(ExistingAgentTab {
                    tab_id: tab.summary_id,
                    terminal_id: terminal.entity_id(),
                    foreground_group: group,
                    agent,
                    label: tab.user_label.clone().unwrap_or_else(|| tab.title.clone()),
                    position: index + 1,
                })
            })
            .collect()
    }

    fn start_handoff_surface(
        &mut self,
        tab_id: u64,
        source_id: EntityId,
        job: &con_core::handoff::HandoffJob,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let tab_idx = self
            .tabs
            .iter()
            .position(|tab| tab.summary_id == tab_id)
            .ok_or_else(|| anyhow::anyhow!("Source tab was closed"))?;
        let _pane_id = self.tabs[tab_idx]
            .pane_tree
            .surface_terminals()
            .into_iter()
            .find(|(_, _, terminal)| terminal.entity_id() == source_id)
            .map(|(pane_id, _, _)| pane_id)
            .ok_or_else(|| anyhow::anyhow!("Source surface was replaced or closed"))?;
        self.activate_tab(tab_idx, window, cx);
        self.start_handoff_tab(job, window, cx)
    }

    fn start_handoff_tab(
        &mut self,
        job: &con_core::handoff::HandoffJob,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let cli = con_cli_path()?;
        anyhow::ensure!(
            cli.is_file(),
            "con-cli is missing beside the Con executable ({}); build it with `cargo build -p con-cli` or reinstall Con",
            cli.display()
        );
        let quote = |text: &str| format!("'{}'", text.replace('\'', "'\"'\"'"));
        let command = format!(
            "{} handoff run {} --revision {}",
            quote(&cli.to_string_lossy()),
            quote(&job.id),
            job.revision
        );
        self.new_tab(&NewTab, window, cx);
        let tab_idx = self.active_tab;
        self.tabs[tab_idx].user_label = Some(format!("{} · Handoff", job.target.agent.label()));
        let terminal = self.tabs[tab_idx]
            .pane_tree
            .try_focused_terminal()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("New Tab has no terminal"))?;
        terminal.ensure_surface(window, cx);
        terminal.write(format!("{command}\n").as_bytes(), cx);
        self.record_runtime_event_for_terminal(
            tab_idx,
            &terminal,
            con_agent::context::PaneRuntimeEvent::PaneCreated {
                startup_command: Some(command),
            },
        );
        self.save_session(cx);
        self.sync_sidebar(cx);
        cx.notify();
        cx.activate(true);
        window.activate_window();
        window.refresh();
        self.observe_handoff_launch(job.id.clone(), self.tabs[tab_idx].summary_id, terminal, cx);
        let pending = job.clone();
        self.harness.spawn_detached(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let _ = tokio::task::spawn_blocking(move || {
                con_core::handoff::HandoffService::new()?
                    .expire_pending_launch(&pending.id, pending.revision)
            })
            .await;
        });
        Ok(())
    }
}
