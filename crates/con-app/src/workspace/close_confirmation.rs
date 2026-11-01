//! One confirmation for a user close operation, addressed by stable identities.
use super::*;
use gpui_component::{
    FocusTrapElement,
    button::{Button, ButtonVariants},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CloseTarget {
    Tabs(Vec<u64>),
    Pane { tab: u64, pane: usize },
    Surface { tab: u64, surface: usize },
}

impl CloseTarget {
    fn contains(&self, tab: u64, pane: usize, surface: usize) -> bool {
        match self {
            Self::Tabs(tabs) => tabs.contains(&tab),
            Self::Pane {
                tab: target,
                pane: target_pane,
            } => *target == tab && *target_pane == pane,
            Self::Surface {
                tab: target,
                surface: target_surface,
            } => *target == tab && *target_surface == surface,
        }
    }

    fn title(&self) -> String {
        match self {
            Self::Tabs(tabs) if tabs.len() > 1 => format!("Close {} tabs?", tabs.len()),
            Self::Tabs(_) => "Close this tab?".into(),
            Self::Pane { .. } => "Close this pane?".into(),
            Self::Surface { .. } => "Close this terminal?".into(),
        }
    }

    fn action(&self) -> &'static str {
        match self {
            Self::Tabs(tabs) if tabs.len() > 1 => "Close Tabs",
            Self::Tabs(_) => "Close Tab",
            Self::Pane { .. } => "Close Pane",
            Self::Surface { .. } => "Close Terminal",
        }
    }
}

struct AgentCloseEntry {
    label: String,
    icon: &'static str,
    context: String,
}

pub(super) struct AgentCloseConfirmation {
    target: CloseTarget,
    agents: Vec<AgentCloseEntry>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
}

impl ConWorkspace {
    fn close_warning_entries(&self, target: &CloseTarget, cx: &App) -> Vec<AgentCloseEntry> {
        let mut agents = Vec::new();
        for (index, tab) in self.tabs.iter().enumerate() {
            for source in tab.pane_tree.surface_infos(None) {
                if !target.contains(tab.summary_id, source.pane_id, source.surface_id) {
                    continue;
                }
                let Some(agent) = self.agent_for_close_warning(&source.terminal, cx) else {
                    continue;
                };
                let label = con_core::native_session::native_agent_adapter(agent)
                    .map_or_else(|| agent.to_string(), |adapter| adapter.label.to_string());
                let directory = cwd_subtitle(source.terminal.current_dir(cx).as_deref());
                let context = source
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .or(directory)
                    .unwrap_or_else(|| tab.user_label.as_ref().unwrap_or(&tab.title).clone());
                agents.push(AgentCloseEntry {
                    label,
                    icon: crate::assets::agent_cli_icon(Some(agent))
                        .unwrap_or("phosphor/robot.svg"),
                    context,
                });
            }
            let includes_builtin = match target {
                CloseTarget::Tabs(tabs) => tabs.contains(&tab.summary_id),
                CloseTarget::Pane { tab: id, pane } => {
                    *id == tab.summary_id
                        && tab.pane_tree.pane_count() == 1
                        && tab.pane_tree.editor_view_for_pane(*pane).is_none()
                }
                CloseTarget::Surface { .. } => false,
            };
            let activity = if index == self.active_tab {
                self.agent_panel.read(cx).state().activity()
            } else {
                tab.panel_state.activity()
            };
            if includes_builtin && activity > con_core::terminal_status::Activity::Idle {
                agents.push(AgentCloseEntry {
                    label: "con agent".into(),
                    icon: "phosphor/robot.svg",
                    context: tab.user_label.as_ref().unwrap_or(&tab.title).clone(),
                });
            }
        }
        agents
    }

    pub(super) fn close_requires_agent_confirmation(&self, target: &CloseTarget, cx: &App) -> bool {
        !self.close_warning_entries(target, cx).is_empty()
    }

    /// True means the operation is held, not that a tab has been removed.
    pub(super) fn request_agent_close_confirmation(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let agents = self.close_warning_entries(&target, cx);
        if agents.is_empty() {
            return false;
        }
        if self.close_confirmation.is_some() {
            return true;
        }
        let previous_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        for terminal in self.tabs[self.active_tab].pane_tree.all_surface_terminals() {
            terminal.set_focus_state(false, cx);
        }
        self.close_confirmation = Some(AgentCloseConfirmation {
            target,
            agents,
            focus,
            previous_focus,
        });
        cx.notify();
        true
    }

    fn dismiss_agent_close_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(confirmation) = self.close_confirmation.take() {
            // This dialog restores its exact prior focus; the legacy palette
            // fallback in render must not replace it with terminal focus.
            self.modal_was_open = false;
            if let Some(focus) = confirmation.previous_focus {
                focus.focus(window, cx);
            }
            self.sync_active_terminal_focus_states(cx);
            cx.notify();
        }
    }

    fn confirm_agent_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirmation) = self.close_confirmation.take() else {
            return;
        };
        self.modal_was_open = false;
        if let Some(focus) = confirmation.previous_focus {
            focus.focus(window, cx);
        }
        self.execute_confirmed_close(confirmation.target, window, cx);
        cx.notify();
    }

    pub(super) fn execute_confirmed_close(
        &mut self,
        target: CloseTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            CloseTarget::Tabs(ids) => {
                // Resolve on approval, not on request: tabs may have moved.
                let indices: Vec<_> = self
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, tab)| ids.contains(&tab.summary_id).then_some(index))
                    .collect();
                for index in indices.into_iter().rev() {
                    self.close_tab_by_index_unchecked(index, window, cx);
                }
            }
            CloseTarget::Pane { tab, pane } => {
                if let Some(index) = self.tab_index_for_summary_id(tab) {
                    if !self.tabs[index].pane_tree.contains_pane(pane) {
                        return;
                    }
                    if !self.remove_pane_in_tab(index, pane, window, cx, true) {
                        self.close_tab_by_index_unchecked(index, window, cx);
                    }
                }
            }
            CloseTarget::Surface { tab, surface } => {
                if let Some(index) = self.tab_index_for_summary_id(tab) {
                    self.close_surface_in_tab_unchecked(index, surface, window, cx);
                }
            }
        }
    }

    pub(super) fn request_close_tabs(
        &mut self,
        ids: Vec<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_confirmation.is_some() {
            return;
        }
        let target = CloseTarget::Tabs(ids);
        if !self.request_agent_close_confirmation(target.clone(), window, cx) {
            self.execute_confirmed_close(target, window, cx);
        }
    }

    pub(super) fn render_agent_close_confirmation(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let confirmation = self.close_confirmation.as_ref()?;
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let title = confirmation.target.title();
        let focus = confirmation.focus.clone();
        let rows = confirmation
            .agents
            .iter()
            .enumerate()
            .map(|(row, agent)| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .child(
                        svg()
                            .path(agent.icon)
                            .size(px(22.0))
                            .flex_shrink_0()
                            .text_color(foreground),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(foreground)
                                    .child(agent.label.clone()),
                            )
                            .child(
                                div()
                                    .id(("agent-close-context", row))
                                    .text_xs()
                                    .text_color(muted)
                                    .truncate()
                                    .child(agent.context.clone())
                                    .tooltip({
                                        let text = agent.context.clone();
                                        move |_, cx| cx.new(|_| Tooltip::new(text.clone())).into()
                                    }),
                            ),
                    )
            })
            .collect::<Vec<_>>();
        let panel =
            div()
                .id("agent-close-confirmation")
                .debug_selector(|| "agent-close-confirmation".into())
                .role(gpui::Role::Dialog)
                .aria_label(title.clone())
                .w(px(420.0))
                .max_w((window.viewport_size().width - px(32.0)).max(px(0.0)))
                .max_h((window.viewport_size().height - px(32.0)).max(px(0.0)))
                .rounded(px(14.0))
                .bg(cx.theme().background.opacity(1.0))
                .p_6()
                .flex()
                .flex_col()
                .gap_4()
                .font_family(cx.theme().font_family.clone())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child(title),
                )
                .child(div().text_sm().text_color(muted).child(
                    "These agent sessions are still open. Closing may interrupt ongoing work.",
                ))
                .child(
                    div()
                        .id("agent-close-list")
                        .min_h_0()
                        .max_h(px(220.0))
                        .overflow_y_scroll()
                        .px_3()
                        .py_1()
                        .rounded(px(9.0))
                        .bg(foreground.opacity(0.04))
                        .children(rows),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .pt_1()
                        .child(
                            Button::new("agent-close-keep-open")
                                .label("Keep Open")
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.dismiss_agent_close_confirmation(window, cx)
                                })),
                        )
                        .child(
                            Button::new("agent-close-confirm")
                                .label(confirmation.target.action())
                                .danger()
                                .text_color(foreground)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.confirm_agent_close(window, cx)
                                })),
                        ),
                );
        let overlay = div()
            .id("agent-close-backdrop")
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
                    this.dismiss_agent_close_confirmation(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .child(panel);
        Some(
            overlay
                .focus_trap("agent-close-focus-trap", &confirmation.focus)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::CloseTarget;
    #[test]
    fn targets_do_not_alias_surfaces_or_panes_in_another_tab() {
        let target = CloseTarget::Surface { tab: 9, surface: 2 };
        assert!(target.contains(9, 0, 2));
        assert!(!target.contains(10, 0, 2));
        assert!(!target.contains(9, 0, 3));
        let pane = CloseTarget::Pane { tab: 9, pane: 5 };
        assert!(pane.contains(9, 5, 2));
        assert!(pane.contains(9, 5, 3));
        assert!(!pane.contains(9, 6, 2));
    }
    #[test]
    fn tab_batch_targets_keep_identity_after_reordering_and_exclude_new_tabs() {
        let target = CloseTarget::Tabs(vec![8, 3]);
        let reordered = [3, 12, 8, 2];
        let closing: Vec<_> = reordered
            .iter()
            .enumerate()
            .filter_map(|(i, id)| target.contains(*id, 0, 0).then_some(i))
            .collect();
        assert_eq!(closing, vec![0, 2]);
        assert!(!target.contains(12, 0, 0));
        assert!(!target.contains(2, 0, 0));
    }
}
