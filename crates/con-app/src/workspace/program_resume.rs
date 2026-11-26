use super::*;
use crate::dialog_style::DialogButtonStyle as _;
use con_core::program_resume_store::ProgramResumeStore;
use con_terminal::program_resume::{LiveResume, ResumeCommand, ResumeOwner, ResumeReview};
use gpui_component::{
    Disableable, FocusTrapElement,
    button::Button,
    scroll::{Scrollbar, ScrollbarMode},
};
use uuid::Uuid;

pub(super) struct PendingReview {
    terminal: TerminalPane,
    record: Uuid,
    command: ResumeCommand,
    review: ResumeReview,
    focus: FocusHandle,
    scroll: gpui::ScrollHandle,
    submitting: bool,
    message: Option<&'static str>,
}

#[derive(PartialEq, Eq)]
struct ReadyPrompt {
    context: Uuid,
    input: u64,
    foreground_group: Option<u64>,
    output: u64,
}

type VerifiedReports = anyhow::Result<Vec<con_ghostty::VerifiedResumeReport>>;

struct ResumeRecordUpdate {
    record: Option<Uuid>,
    writes: Vec<ProgramResumeWrite>,
}

fn reduce_program_resume_reports(
    live: &mut LiveResume,
    previous: Option<Uuid>,
    reports: VerifiedReports,
    exited: Option<ResumeOwner>,
    now: u64,
) -> Option<ResumeRecordUpdate> {
    let mut changed = exited.is_some_and(|owner| live.owner_exited(owner));
    match reports {
        Ok(reports) => {
            for (owner, report, directory) in reports {
                changed |= live.apply(owner, report, directory, now);
            }
        }
        Err(_) => {
            // An overflow could have discarded a clear. Invalidate only
            // this incarnation's live candidate, never a restored one.
            changed |= live.candidate().is_some();
            live.invalidate_candidate();
        }
    }
    if !changed {
        return None;
    }

    let mut writes = Vec::new();
    if let Some(id) = previous {
        writes.push(ProgramResumeWrite::Remove(id));
    }
    let record = live.candidate().map(|(_, command)| {
        let id = Uuid::new_v4();
        writes.push(ProgramResumeWrite::Save(id, command.clone()));
        id
    });
    Some(ResumeRecordUpdate { record, writes })
}

impl ConWorkspace {
    /// Poll alongside native process observations, never from render. Hold
    /// one read per terminal incarnation, even while its pane is transferred.
    pub(super) fn refresh_program_resumes(&self, cx: &mut Context<Self>) {
        if self.window_close_prepared || self.is_quick_terminal {
            return;
        }
        let mut pending = Vec::new();
        let mut observations = Vec::new();
        let mut discarded = Vec::new();
        for terminal in self
            .tabs
            .iter()
            .flat_map(|tab| tab.pane_tree.all_surface_terminals())
        {
            let Some(context) = terminal.program_resume_context(cx) else {
                continue;
            };
            let mut tracking = terminal.program_resume_tracking.borrow_mut();
            if tracking.scan_pending {
                continue;
            }
            if tracking.context != Some(context) {
                if tracking.live.candidate().is_some()
                    && let Some(id) = terminal.take_saved_program_resume()
                {
                    discarded.push(ProgramResumeWrite::Remove(id));
                }
                tracking.context = Some(context);
                tracking.live = Default::default();
            }
            let reader = terminal.take_program_resume_reader(cx);
            let owner = tracking.live.candidate().map(|(owner, _)| *owner);
            if reader.is_none() && owner.is_none() {
                continue;
            }
            tracking.scan_pending = true;
            pending.push((terminal.clone(), context));
            observations.push((reader, owner));
        }
        self.save_program_resume_updates(discarded, cx);
        if pending.is_empty() {
            return;
        }

        cx.spawn(async move |_, cx| {
            let verified = cx
                .background_executor()
                .spawn(async move {
                    observations
                        .into_iter()
                        .map(|(reader, owner)| {
                            let reports = reader.map_or_else(|| Ok(Vec::new()), |read| read());
                            let exited = owner.filter(|owner| {
                                con_ghostty::process::process_incarnation_exited(
                                    owner.pid.get(),
                                    owner.started_at,
                                )
                            });
                            (reports, exited)
                        })
                        .collect::<Vec<_>>()
                })
                .await;

            let _ = cx.update(|cx| {
                let workspaces = cx.global::<WorkspaceContinuity>().workspaces.clone();
                for ((terminal, context), (reports, exited)) in pending.into_iter().zip(verified) {
                    terminal.program_resume_tracking.borrow_mut().scan_pending = false;
                    if terminal.program_resume_context(cx) != Some(context) {
                        continue;
                    }
                    let owner =
                        workspaces
                            .iter()
                            .filter_map(WeakEntity::upgrade)
                            .find(|workspace| {
                                let workspace = workspace.read(cx);
                                !workspace.window_close_prepared
                                    && !workspace.is_quick_terminal
                                    && workspace.tabs.iter().any(|tab| {
                                        tab.pane_tree.pane_id_for_terminal(&terminal).is_some()
                                    })
                            });
                    if let Some(owner) = owner {
                        owner.update(cx, |workspace, cx| {
                            workspace.apply_program_resume_reports(&terminal, reports, exited, cx);
                        });
                    }
                }
            });
        })
        .detach();
    }

    fn apply_program_resume_reports(
        &self,
        terminal: &TerminalPane,
        reports: VerifiedReports,
        exited: Option<ResumeOwner>,
        cx: &mut Context<Self>,
    ) {
        let Ok(elapsed) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
            return;
        };
        let Some(update) = reduce_program_resume_reports(
            &mut terminal.program_resume_tracking.borrow_mut().live,
            terminal.saved_program_resume(),
            reports,
            exited,
            elapsed.as_secs(),
        ) else {
            return;
        };
        terminal.set_saved_program_resume(update.record);
        let writes = update.writes;
        self.save_program_resume_updates(writes, cx);
    }

    pub(super) fn review_saved_program(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.program_resume_review.is_some() || self.close_confirmation.is_some() {
            return;
        }
        let Some(terminal) = self.try_active_terminal().cloned() else {
            return;
        };
        let Some(record) = terminal.recoverable_program_resume() else {
            return;
        };

        cx.spawn_in(window, async move |workspace, window| {
            let loaded = window
                .background_executor()
                .spawn(async move { ProgramResumeStore::default().load(record) })
                .await;
            let _ = window.update(|window, cx| {
                let _ =
                    workspace.update(cx, |this, cx| {
                        if !this.active_program_resume_matches(&terminal, record)
                            || this.program_resume_review.is_some()
                            || this.close_confirmation.is_some()
                        {
                            return;
                        }
                        if matches!(loaded, Ok(None)) {
                            terminal.take_saved_program_resume();
                            this.save_session(cx);
                        }
                        let Some((command, review)) = loaded.ok().flatten().and_then(|command| {
                            command.review().ok().map(|review| (command, review))
                        }) else {
                            std::mem::drop(window.prompt(
                                PromptLevel::Info,
                                "Saved program unavailable",
                                Some("The terminal cannot read a valid recovery record."),
                                &["OK"],
                                cx,
                            ));
                            return;
                        };
                        let focus = cx.focus_handle();
                        focus.focus(window, cx);
                        this.program_resume_review = Some(PendingReview {
                            terminal,
                            record,
                            command,
                            review,
                            focus,
                            scroll: gpui::ScrollHandle::new(),
                            submitting: false,
                            message: None,
                        });
                        cx.notify();
                    });
            });
        })
        .detach();
    }

    fn active_program_resume_matches(&self, terminal: &TerminalPane, record: Uuid) -> bool {
        self.try_active_terminal().is_some_and(|active| {
            active.entity_id() == terminal.entity_id()
                && active.recoverable_program_resume() == Some(record)
        })
    }

    fn dismiss_program_resume_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .program_resume_review
            .as_ref()
            .is_some_and(|review| review.submitting)
        {
            return;
        }
        if self.program_resume_review.take().is_some() {
            self.modal_was_open = false;
            if let Some(terminal) = self.try_active_terminal() {
                terminal.focus(window, cx);
            }
            cx.notify();
        }
    }

    fn confirm_program_resume(
        &mut self,
        start_at_prompt: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.program_resume_review.as_ref() else {
            return;
        };
        if pending.submitting {
            return;
        }
        let terminal = pending.terminal.clone();
        let record = pending.record;
        if !self.active_program_resume_matches(&terminal, record) {
            return;
        }
        let Some(terminal_context) = terminal.program_resume_context(cx) else {
            self.set_program_resume_message("Open a local shell in this terminal first.", cx);
            return;
        };
        let prompt = if start_at_prompt {
            let Some(prompt) = self.local_resume_prompt(&terminal, record, cx) else {
                self.set_program_resume_message("The shell prompt changed. Try again.", cx);
                return;
            };
            Some(prompt)
        } else {
            None
        };
        let window = window.window_handle();
        let Some(instance) = terminal.surface_instance(cx) else {
            return;
        };
        let dispatch = std::env::current_exe()
            .ok()
            .map(|path| {
                path.with_file_name(if cfg!(target_os = "windows") {
                    "con-cli.exe"
                } else {
                    "con-cli"
                })
            })
            .filter(|path| path.is_file())
            .and_then(|path| helper_invocation_prefix(&terminal, &path, cx));
        let Some(dispatch) = dispatch else {
            self.set_program_resume_message(
                "Recovery needs the companion con-cli and a supported local shell.",
                cx,
            );
            return;
        };

        let command = pending.command.clone();
        if let Some(pending) = self.program_resume_review.as_mut() {
            pending.submitting = true;
            pending.message = None;
        }
        cx.notify();
        cx.spawn(async move |workspace, cx| {
            let approval = cx
                .background_executor()
                .spawn(async move {
                    ProgramResumeStore::default().approve(record, &command, terminal_context)
                })
                .await;
            let Ok(ticket) = approval else {
                let _ = workspace.update(cx, |this, cx| {
                    if this
                        .program_resume_review
                        .as_ref()
                        .is_some_and(|review| review.record == record)
                    {
                        this.set_program_resume_message(
                            "The recovery record is no longer available.",
                            cx,
                        );
                    }
                });
                return;
            };
            let mut sent = false;
            let _ = window.update(cx, |_, window, cx| {
                let _ = workspace.update(cx, |this, cx| {
                    let current = terminal.surface_instance(cx);
                    let unchanged = this.active_program_resume_matches(&terminal, record)
                        && terminal.program_resume_context(cx) == Some(terminal_context)
                        && current.is_some_and(|current| current.ptr_eq(&instance))
                        && this.program_resume_review.as_ref().is_some_and(|review| {
                            review.record == record
                                && review.terminal.entity_id() == terminal.entity_id()
                        });
                    if !unchanged {
                        this.set_program_resume_message(
                            "The terminal changed before recovery could start.",
                            cx,
                        );
                        return;
                    }
                    let line = format!("{dispatch} +program-resume {ticket}");
                    if let Some(prompt) = &prompt {
                        if this.local_resume_prompt(&terminal, record, cx).as_ref() != Some(prompt)
                            || !terminal.write_observed(format!("{line}\n").as_bytes(), cx)
                        {
                            this.set_program_resume_message(
                                "The shell prompt changed. Try again.",
                                cx,
                            );
                            return;
                        }
                        this.program_resume_review = None;
                        cx.notify();
                    } else {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(line));
                        use gpui_component::{WindowExt, notification::Notification};
                        this.program_resume_review = None;
                        terminal.focus(window, cx);
                        cx.notify();
                        window.push_notification(
                            Notification::new().title("Resume command copied").message(
                                "Paste it at this terminal’s local shell prompt within one minute.",
                            ),
                            cx,
                        );
                    }
                    sent = true;
                });
            });
            if !sent {
                cx.background_executor()
                    .spawn(async move {
                        let _ = ProgramResumeStore::default().remove(ticket);
                    })
                    .await;
            }
        })
        .detach();
    }

    fn set_program_resume_message(&mut self, message: &'static str, cx: &mut Context<Self>) {
        if let Some(review) = self.program_resume_review.as_mut() {
            review.submitting = false;
            review.message = Some(message);
            cx.notify();
        }
    }

    #[cfg(target_os = "macos")]
    fn local_resume_prompt(
        &self,
        terminal: &TerminalPane,
        record: Uuid,
        cx: &App,
    ) -> Option<ReadyPrompt> {
        if self.window_close_prepared
            || !self.active_program_resume_matches(terminal, record)
            || !terminal.is_alive(cx)
            || terminal.is_busy(cx)
        {
            return None;
        }
        let context = terminal.program_resume_context(cx)?;
        let prompt = terminal.prompt_state(cx);
        let foreground_group = terminal.foreground_process_group_id(cx);
        foreground_group?;
        let (observation, runtime) =
            self.observe_terminal_runtime_for_tab(self.active_tab, terminal, 12, cx);
        (observation.input_generation == observation.last_command_finished_input_generation
            && runtime.remote_host.is_none()
            && runtime.tmux_session.is_none()
            && con_agent::control::PaneControlState::from_runtime(&runtime)
                .allows_visible_shell_exec())
        .then_some(ReadyPrompt {
            context,
            input: observation.input_generation,
            foreground_group,
            output: prompt.output_generation,
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn local_resume_prompt(&self, _: &TerminalPane, _: Uuid, _: &App) -> Option<ReadyPrompt> {
        None
    }

    pub(super) fn render_program_resume_review(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let pending = self.program_resume_review.as_ref()?;
        let focus = pending.focus.clone();
        let theme = cx.theme();
        let foreground = theme.foreground;
        let muted = theme.muted_foreground;
        let mono = theme.mono_font_family.clone();
        let title = "Resume this program?";
        let entry = |label: &'static str, value: String| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_xs().text_color(muted).child(label))
                .child(
                    div()
                        .font_family(mono.clone())
                        .text_sm()
                        .whitespace_normal()
                        .child(value),
                )
        };
        let body = div()
            .id("program-resume-details")
            .min_h_0()
            .max_h(px(290.0))
            .overflow_y_scroll()
            .track_scroll(&pending.scroll)
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(entry("Program", pending.review.executable.clone()))
            .child(entry("Working directory", pending.review.directory.clone()))
            .when(!pending.review.arguments.is_empty(), |body| {
                body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_xs().text_color(muted).child("Arguments"))
                        .children(pending.review.arguments.iter().cloned().map(|argument| {
                            div()
                                .font_family(mono.clone())
                                .text_sm()
                                .whitespace_normal()
                                .child(argument)
                        })),
                )
            });

        let starts_at_prompt = self
            .local_resume_prompt(&pending.terminal, pending.record, cx)
            .is_some();
        let panel = div()
            .id("program-resume-review")
            .role(gpui::Role::Dialog)
            .aria_label(title)
            .w(px(460.0))
            .max_w((window.viewport_size().width - px(32.0)).max(px(0.0)))
            .max_h((window.viewport_size().height - px(32.0)).max(px(0.0)))
            .rounded(px(14.0))
            .bg(theme.background.opacity(1.0))
            .font_family(theme.font_family.clone())
            .text_color(foreground)
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .child(
                div()
                    .text_size(px(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div().text_sm().text_color(muted).child(if starts_at_prompt {
                    "A program saved this command for recovery in this terminal."
                } else {
                    "Copy the one-time command and run it at this terminal’s local shell prompt."
                }),
            )
            .child(
                div()
                    .relative()
                    .min_h_0()
                    .max_h(px(290.0))
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .bg(foreground.opacity(0.045))
                    .child(body)
                    .child(Scrollbar::vertical(&pending.scroll).mode(ScrollbarMode::Always)),
            )
            .when_some(pending.message, |panel, message| {
                panel.child(div().text_sm().text_color(muted).child(message))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("program-resume-cancel")
                            .label("Cancel")
                            .dialog_secondary(cx)
                            .disabled(pending.submitting)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dismiss_program_resume_review(window, cx);
                            })),
                    )
                    .child(
                        Button::new("program-resume-confirm")
                            .label(if starts_at_prompt { "Resume" } else { "Copy Command" })
                            .dialog_primary(cx)
                            .disabled(pending.submitting)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.confirm_program_resume(starts_at_prompt, window, cx);
                            })),
                    ),
            );

        Some(
            div()
                .id("program-resume-backdrop")
                .absolute()
                .inset_0()
                .size_full()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::black().opacity(0.18))
                .track_focus(&focus)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape"
                        || (event.keystroke.key == "enter" && focus.is_focused(window))
                    {
                        this.dismiss_program_resume_review(window, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }))
                .child(panel)
                .focus_trap("program-resume-focus-trap", &pending.focus)
                .into_any_element(),
        )
    }
}

/// The only interpolated values are the installed companion path and the
/// approval UUID. Report argv and cwd are always loaded by the companion.
fn helper_invocation_prefix(terminal: &TerminalPane, path: &Path, cx: &App) -> Option<String> {
    let path = path.to_str()?;
    if path.chars().any(char::is_control) {
        return None;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (terminal, cx);
        Some(format!("command '{}'", path.replace('\'', "'\\''")))
    }
    #[cfg(target_os = "windows")]
    {
        let shell = terminal.root_identity(cx)?.executable;
        let name = shell.file_stem()?.to_str()?.to_ascii_lowercase();
        match name.as_str() {
            "pwsh" | "powershell" => Some(format!("& '{}'", path.replace('\'', "''"))),
            // cmd expands percent variables even inside a double quote.
            "cmd" if !path.contains(['%', '!', '"']) => Some(format!("\"{path}\"")),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use con_terminal::program_resume::{ReportAction, ResumeReport};

    fn owner(pid: u32, started_at: u64) -> ResumeOwner {
        ResumeOwner {
            pid: pid.try_into().unwrap(),
            started_at,
        }
    }

    fn report(owner: ResumeOwner, time: u64, action: ReportAction) -> VerifiedReports {
        Ok(vec![(
            owner,
            ResumeReport {
                pid: owner.pid,
                time,
                action,
            },
            None,
        )])
    }

    #[::core::prelude::v1::test]
    fn record_replacement_and_removal_follow_the_verified_owner() {
        let mut live = LiveResume::default();
        let inherited = Some(Uuid::new_v4());
        let process = owner(23, 1000);
        let other_process = owner(23, 2000);
        let command = ResumeCommand {
            argv: vec!["editor".into(), "workspace".into()],
            cwd: Some(std::env::temp_dir()),
        };

        let accepted = reduce_program_resume_reports(
            &mut live,
            inherited,
            report(process, 100, ReportAction::Set(command.clone())),
            None,
            100,
        )
        .unwrap();
        assert!(matches!(accepted.writes.as_slice(),
            [ProgramResumeWrite::Remove(old), ProgramResumeWrite::Save(new, value)]
                if Some(*old) == inherited && Some(*new) == accepted.record && *value == command));

        let saved = accepted.record;
        assert!(
            reduce_program_resume_reports(
                &mut live,
                saved,
                report(process, 101, ReportAction::Set(command)),
                None,
                101,
            )
            .is_none()
        );
        assert!(
            reduce_program_resume_reports(
                &mut live,
                saved,
                report(other_process, 102, ReportAction::Clear),
                Some(other_process),
                102,
            )
            .is_none()
        );

        let removal =
            reduce_program_resume_reports(&mut live, saved, Ok(Vec::new()), Some(process), 103)
                .unwrap();
        assert!(removal.record.is_none());
        assert!(matches!(removal.writes.as_slice(),
            [ProgramResumeWrite::Remove(id)] if Some(*id) == saved));
    }

    #[::core::prelude::v1::test]
    fn overflow_cannot_clear_a_record_from_a_previous_incarnation() {
        let mut live = LiveResume::default();
        let saved = Some(Uuid::new_v4());
        assert!(
            reduce_program_resume_reports(
                &mut live,
                saved,
                Err(anyhow::anyhow!("overflow")),
                None,
                1,
            )
            .is_none()
        );
    }
}
