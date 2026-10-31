use super::*;
use crate::native_agent::NativeAgentPhase;
use con_core::native_session::NativeAgentSession;

fn session_key(reference: &NativeAgentSession) -> String {
    serde_json::to_string(&reference.identity()).expect("native session identity is serializable")
}

#[cfg(target_os = "macos")]
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

impl ConWorkspace {
    fn release_native_claim(&mut self, reference: &NativeAgentSession, origin: uuid::Uuid) {
        let key = session_key(reference);
        if self.native_session_claims.get(&key) == Some(&origin) {
            self.native_session_claims.remove(&key);
        }
    }

    pub(super) fn native_session_hint_for_tab(&self, tab: usize) -> Option<String> {
        let hints: Vec<_> = self
            .tabs
            .get(tab)?
            .pane_tree
            .surface_infos(None)
            .into_iter()
            .filter_map(|surface| {
                let hint = surface.terminal.native_agent.borrow().summary()?;
                Some(
                    surface
                        .title
                        .filter(|title| !title.is_empty())
                        .map(|title| {
                            format!(
                                "{}
{hint}",
                                title.chars().take(80).collect::<String>()
                            )
                        })
                        .unwrap_or(hint),
                )
            })
            .collect();
        (!hints.is_empty()).then(|| {
            hints.join(
                "

",
            )
        })
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn on_native_session_requested(
        &mut self,
        source: &Entity<GhosttyView>,
        event: &crate::ghostty_view::GhosttyNativeSessionRequested,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let terminal = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane_tree.all_surface_terminals())
            .find(|terminal| terminal.entity_id() == source.entity_id())
            .cloned();
        let Some(terminal) = terminal else {
            return;
        };
        if event.0 {
            self.forget_native_session(&terminal, cx);
        } else {
            if terminal.native_agent.borrow().phase == NativeAgentPhase::Resuming {
                return;
            }
            if let Err(error) = self.request_native_session_resume(terminal.clone(), false, cx) {
                self.fail_native_resume(&terminal, error.message, cx);
            }
        }
    }

    fn native_terminal(&self, origin: uuid::Uuid) -> Option<(usize, TerminalPane)> {
        self.tabs.iter().enumerate().find_map(|(tab, state)| {
            state
                .pane_tree
                .all_surface_terminals()
                .into_iter()
                .find_map(|terminal| {
                    (terminal.native_agent.borrow().origin == origin)
                        .then(|| (tab, terminal.clone()))
                })
        })
    }

    // Keep the private RPC fields explicit at the workspace boundary.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_native_session_report(
        &mut self,
        origin: &str,
        sequence: u64,
        reference: Option<NativeAgentSession>,
        error: Option<String>,
        expected_session_id: Option<String>,
        started: bool,
        cx: &mut Context<Self>,
    ) -> ControlResult {
        let origin = origin
            .parse()
            .map_err(|_| ControlError::invalid_params("Invalid terminal context"))?;
        let (_, terminal) = self.native_terminal(origin).ok_or_else(|| {
            ControlError::invalid_params("The originating terminal no longer exists")
        })?;
        if started && (reference.is_some() || error.is_some()) {
            return Err(ControlError::invalid_params(
                "A start acknowledgement cannot replace native identity or report failure",
            ));
        }
        if let Some(reference) = &reference {
            reference.validate().map_err(ControlError::invalid_params)?;
        }
        if error.as_ref().is_some_and(|error| error.len() > 2048) {
            return Err(ControlError::invalid_params(
                "Native resume diagnostic is oversized",
            ));
        }
        {
            let mut state = terminal.native_agent.borrow_mut();
            if sequence <= state.report_sequence {
                return Err(ControlError::invalid_params(
                    "The native session report is stale",
                ));
            }
            if reference.is_none()
                && state
                    .reference
                    .as_ref()
                    .map(|saved| saved.session_id.as_str())
                    != expected_session_id.as_deref()
            {
                return Err(ControlError::invalid_params(
                    "The native helper belongs to a previous conversation",
                ));
            }
            state.report_sequence = sequence;
            if let Some(mut reference) = reference {
                if matches!(
                    state.phase,
                    NativeAgentPhase::Resuming
                        | NativeAgentPhase::Started
                        | NativeAgentPhase::NeedsInteraction
                ) && state
                    .reference
                    .as_ref()
                    .is_some_and(|saved| saved.identity() != reference.identity())
                {
                    state.phase = NativeAgentPhase::NeedsInteraction;
                    state.error = Some("The native agent reported a different conversation. The saved reference was kept; inspect the terminal.".into());
                    drop(state);
                    terminal.notify_native_agent(cx);
                    self.sync_sidebar(cx);
                    self.save_session(cx);
                    cx.notify();
                    return Err(ControlError::invalid_params(
                        "Native resume did not confirm the exact saved conversation",
                    ));
                }
                // Record confirmed native size, not guessed pane geometry.
                if let Some(size) = terminal
                    .surface_instance(cx)
                    .and_then(|t| t.upgrade())
                    .map(|t| t.size())
                    && size.width_px > 0
                    && size.height_px > 0
                {
                    reference.viewport_pixels = Some([size.width_px, size.height_px]);
                }
                if let Some(previous) = &state.reference
                    && previous.identity() != reference.identity()
                {
                    self.release_native_claim(previous, state.origin);
                }
                let key = session_key(&reference);
                self.native_session_claims.entry(key).or_insert(origin);
                state.reference = Some(reference);
                state.resume_submitted_at = None;
                state.phase = NativeAgentPhase::Observed;
                state.restored = false;
                state.error = None;
            } else if started {
                if state.phase == NativeAgentPhase::Resuming {
                    state.phase = NativeAgentPhase::Started;
                    state.resume_submitted_at = None;
                }
            } else {
                if let Some(reference) = &state.reference {
                    self.release_native_claim(reference, state.origin);
                }
                state.phase = if error.is_some() {
                    NativeAgentPhase::Failed
                } else {
                    NativeAgentPhase::Available
                };
                state.error = error;
            }
        }
        terminal.notify_native_agent(cx);
        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
        Ok(json!({"status":"recorded"}))
    }

    pub(super) fn forget_native_session(
        &mut self,
        terminal: &TerminalPane,
        cx: &mut Context<Self>,
    ) {
        let mut state = terminal.native_agent.borrow_mut();
        if let Some(reference) = &state.reference {
            self.release_native_claim(reference, state.origin);
        }
        state.reference = None;
        state.error = None;
        state.phase = NativeAgentPhase::Available;
        state.auto_attempted = true;
        drop(state);
        terminal.notify_native_agent(cx);
        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(super) fn request_native_session_resume(
        &mut self,
        terminal: TerminalPane,
        automatic: bool,
        cx: &mut Context<Self>,
    ) -> ControlResult {
        if !cfg!(target_os = "macos") {
            return Err(ControlError::invalid_params(
                "Native session resume currently supports macOS only",
            ));
        }
        let (origin, reference) = {
            let state = terminal.native_agent.borrow();
            if state.phase == NativeAgentPhase::Resuming {
                return Err(ControlError::invalid_params(
                    "This terminal is already resuming its native conversation",
                ));
            }
            (
                state.origin,
                state.reference.clone().ok_or_else(|| {
                    ControlError::invalid_params("No native conversation is linked to this surface")
                })?,
            )
        };
        reference.validate().map_err(ControlError::invalid_params)?;
        let key = session_key(&reference);
        if let Some(owner) = self.native_session_claims.get(&key).copied()
            && owner != origin
            && let Some((tab, peer)) = self.native_terminal(owner)
        {
            let pending = peer.native_agent.borrow().phase == NativeAgentPhase::Resuming;
            let (_, runtime) = self.observe_terminal_runtime_for_tab(tab, &peer, 20, cx);
            let shell = con_agent::control::PaneControlState::from_runtime(&runtime)
                .allows_visible_shell_exec();
            if pending || !shell {
                return Err(ControlError::invalid_params(
                    "This native conversation is already resuming or active in another surface",
                ));
            }
        }
        self.native_session_claims.insert(key, origin);
        {
            let mut state = terminal.native_agent.borrow_mut();
            state.phase = NativeAgentPhase::Resuming;
            state.error = None;
            state.auto_attempted = true;
        }
        terminal.notify_native_agent(cx);
        let handle = self.workspace_handle.clone();
        let window_handle = self.window_handle;
        let started = std::time::Instant::now();
        cx.spawn(async move |_, cx| {
            loop {
                let result = window_handle.update(cx, |_, window, cx| {
                    handle.update(cx, |workspace, cx| {
                        match workspace.launch_native_session(&terminal, &reference, automatic, window, cx) {
                            Ok(true) => true,
                            Ok(false) if started.elapsed() < Duration::from_secs(30) => false,
                            Ok(false) => {
                                workspace.fail_native_resume(&terminal, "The saved terminal did not reach a proven shell prompt; inspect it before retrying".into(), cx);
                                true
                            }
                            Err(error) => {
                                workspace.fail_native_resume(&terminal, error.message, cx);
                                true
                            }
                        }
                    }).unwrap_or(true)
                }).unwrap_or(true);
                if result { return; }
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
        }).detach();
        self.sync_sidebar(cx);
        cx.notify();
        Ok(json!({"status":"queued", "origin":origin.to_string()}))
    }

    fn fail_native_resume(
        &mut self,
        terminal: &TerminalPane,
        error: String,
        cx: &mut Context<Self>,
    ) {
        let mut state = terminal.native_agent.borrow_mut();
        if let Some(reference) = &state.reference {
            self.release_native_claim(reference, state.origin);
        }
        state.phase = NativeAgentPhase::Failed;
        state.error = Some(error);
        state.auto_attempted = true;
        drop(state);
        terminal.notify_native_agent(cx);
        self.sync_sidebar(cx);
        self.save_session(cx);
        cx.notify();
    }

    fn launch_native_session(
        &mut self,
        terminal: &TerminalPane,
        reference: &NativeAgentSession,
        automatic: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, ControlError> {
        if automatic && !self.config.appearance.resume_agent_sessions {
            self.release_native_claim(reference, terminal.native_agent.borrow().origin);
            terminal.native_agent.borrow_mut().phase = NativeAgentPhase::Available;
            terminal.notify_native_agent(cx);
            self.sync_sidebar(cx);
            cx.notify();
            return Ok(true);
        }
        if self.window_close_prepared {
            return Err(ControlError::invalid_params(
                "The workspace is closing; native resume was not started",
            ));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (terminal, reference, window, cx);
            return Err(ControlError::invalid_params(
                "Native resume is not supported on this platform",
            ));
        }
        #[cfg(target_os = "macos")]
        {
            if terminal.native_agent.borrow().reference.as_ref() != Some(reference) {
                return Err(ControlError::invalid_params(
                    "The saved native conversation changed before resume",
                ));
            }
            if !reference.cwd.is_dir() || !reference.config_dir.is_dir() {
                return Err(ControlError::invalid_params(
                    "The saved working directory or native store is unavailable; no new session was started",
                ));
            }
            terminal.prepare_native_resume(window, cx);
            let (tab_idx, _) = self
                .native_terminal(terminal.native_agent.borrow().origin)
                .ok_or_else(|| ControlError::invalid_params("The target terminal has closed"))?;
            let (_, runtime) = self.observe_terminal_runtime_for_tab(tab_idx, terminal, 20, cx);
            let control = con_agent::control::PaneControlState::from_runtime(&runtime);
            if runtime.remote_host.is_some() {
                return Err(ControlError::invalid_params(
                    "Native session resume is local-only; this terminal is connected to a remote host",
                ));
            }
            if !control.allows_visible_shell_exec() {
                if runtime.agent_cli.is_some() {
                    return Err(ControlError::invalid_params(
                        "Another agent is already active in this terminal",
                    ));
                }
                return Ok(false);
            }
            let executable =
                std::env::current_exe().map_err(|e| ControlError::internal(e.to_string()))?;
            let helper = executable.with_file_name("con-cli");
            if !helper.is_file() {
                return Err(ControlError::invalid_params(
                    "The matching companion con-cli is missing",
                ));
            }
            let origin = terminal.native_agent.borrow().origin.to_string();
            let mut args = vec![
                helper.to_string_lossy().into_owned(),
                "--socket".into(),
                con_core::control_socket_path()
                    .to_string_lossy()
                    .into_owned(),
                "native-session".into(),
                "resume".into(),
                "--origin".into(),
                origin,
                "--agent".into(),
                reference.agent.clone(),
                "--session-id".into(),
                reference.session_id.clone(),
                "--cwd".into(),
                reference.cwd.to_string_lossy().into_owned(),
                "--config-dir".into(),
                reference.config_dir.to_string_lossy().into_owned(),
            ];
            if let Some(path) = &reference.transcript_path {
                args.extend([
                    "--transcript-path".into(),
                    path.to_string_lossy().into_owned(),
                ]);
            }
            let command = args
                .iter()
                .map(|arg| quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
                + "\n";
            if !terminal.write_observed(command.as_bytes(), cx) {
                return Err(ControlError::internal(
                    "Native resume delivery was not observed; inspect the terminal before retrying",
                ));
            }
            terminal.native_agent.borrow_mut().resume_submitted_at =
                Some(std::time::Instant::now());
            self.save_session(cx);
            Ok(true)
        }
    }

    pub(super) fn refresh_native_session_resumes(&mut self, cx: &mut Context<Self>) {
        if !cfg!(target_os = "macos") {
            return;
        }
        let mut changed = false;
        for terminal in self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane_tree.all_surface_terminals())
        {
            let mut state = terminal.native_agent.borrow_mut();
            if state.phase == NativeAgentPhase::Resuming
                && state
                    .resume_submitted_at
                    .is_some_and(|sent| sent.elapsed() > Duration::from_secs(30))
            {
                state.phase = NativeAgentPhase::NeedsInteraction;
                state.error = Some("The native session link is not confirmed. Inspect the terminal; no second launch was attempted.".into());
                drop(state);
                terminal.notify_native_agent(cx);
                changed = true;
            }
        }
        if changed {
            self.sync_sidebar(cx);
            cx.notify();
        }
        if !self.config.appearance.resume_agent_sessions {
            return;
        }
        let candidates: Vec<_> = self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane_tree.all_surface_terminals())
            .filter(|terminal| {
                let state = terminal.native_agent.borrow();
                state.restored && state.reference.is_some() && !state.auto_attempted
            })
            .cloned()
            .collect();
        for terminal in candidates {
            // Terminal creation is window-aware. A readiness failure retains the
            // exact reference and offers an explicit retry instead of looping.
            if let Err(error) = self.request_native_session_resume(terminal.clone(), true, cx) {
                self.fail_native_resume(&terminal, error.message, cx);
            }
        }
    }
}
