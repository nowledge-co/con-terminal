use gpui_component::{Sizable, input::Input};

use super::*;

impl HandoffDestinationPanel {
    pub(super) fn render_source_card(
        &self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut source_card = card(palette);
        if let Some(warning) = &self.binding_warning {
            source_card = source_card.child(muted_note(warning.clone(), palette));
        }
        if let Some(agent) = self
            .agents
            .iter()
            .find(|agent| agent.agent == self.source_agent)
            && agent.target_supported
            && !agent.source_supported
        {
            source_card = source_card.child(muted_note(
                "Source: export unavailable for this version".into(),
                palette,
            ));
        }
        if self.source_not_started {
            source_card = source_card.child(muted_note(
                "TUI has no session ID — select one below".into(),
                palette,
            ));
        }
        if self.loading_sessions {
            source_card = source_card.child(muted_note("Detecting session…".into(), palette));
        } else if let Some(source) = self.selected_source().cloned() {
            source_card = source_card.child(
                card_row()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .min_w_0()
                            .child(
                                svg()
                                    .path(agent_icon(self.source_agent))
                                    .size_4()
                                    .flex_none()
                                    .text_color(palette.foreground),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_ellipsis()
                                    .child(source.title),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .pl(px(24.0))
                            .text_size(px(11.5))
                            .font_family(palette.mono_font_family.clone())
                            .text_color(palette.muted_foreground)
                            .text_ellipsis()
                            .child(source.id),
                    ),
            );
            let needs_current_confirmation = (self.live_source_id.is_some()
                && self.live_source_requires_confirmation)
                || (self.live_source_id.is_none() && self.selected_session_id.is_none());
            if needs_current_confirmation {
                source_card = source_card.child(Self::check_row(
                    "handoff-current-session",
                    "This is the current session",
                    self.single_session_confirmed,
                    !self.busy,
                    palette,
                    |this, checked, cx| {
                        this.single_session_confirmed = checked;
                        cx.notify();
                    },
                    cx,
                ));
            }
        } else if !self.sessions.is_empty() {
            // No live binding and several candidates: the contract requires
            // an explicit user pick of the native session ID.
            source_card =
                source_card.child(muted_note("Select the current session".into(), palette));
            let mut list = div()
                .id("handoff-session-candidates")
                .flex()
                .flex_col()
                .max_h(ROW_HEIGHT * 4.0)
                .overflow_y_scroll();
            let candidates: Vec<_> = self
                .sessions
                .iter()
                .take(self.visible_candidate_limit)
                .map(|session| {
                    (
                        session.id.clone(),
                        session.title.clone(),
                        session.export_warning.clone(),
                    )
                })
                .collect();
            for (id, title, warning) in candidates {
                let selected = self.selected_session_id.as_deref() == Some(id.as_str());
                list = list.child(Self::destination_row(
                    format!("handoff-session-{id}"),
                    agent_icon(self.source_agent),
                    title,
                    warning.clone().unwrap_or_else(|| compact_session_id(&id)),
                    !self.busy,
                    selected,
                    palette,
                    move |this, _, cx| {
                        if let Some(warning) = &warning {
                            this.error = Some(warning.clone());
                            cx.notify();
                            return;
                        }
                        this.error = None;
                        this.live_source_id = None;
                        this.selected_session_id = Some(id.clone());
                        log::info!("handoff: picked source session {id}");
                        // A listed job bound to the picked session belongs in
                        // the current-task card.
                        this.promote_related_job(cx);
                        cx.notify();
                    },
                    window,
                    cx,
                ));
            }
            source_card = source_card.child(list);
            let remaining = self
                .sessions
                .len()
                .saturating_sub(self.visible_candidate_limit);
            if remaining > 0 {
                let page = remaining.min(CANDIDATE_PAGE);
                source_card = source_card.child(Self::destination_row(
                    "handoff-sessions-show-more",
                    "phosphor/caret-down.svg",
                    format!("Show {page} more…"),
                    format!("{remaining} not shown"),
                    !self.busy,
                    false,
                    palette,
                    |this, _, cx| {
                        this.visible_candidate_limit += CANDIDATE_PAGE;
                        cx.notify();
                    },
                    window,
                    cx,
                ));
            }
        } else {
            source_card = source_card.child(muted_note(
                "No saved sessions in this directory".into(),
                palette,
            ));
        }
        source_card
    }

    pub(super) fn render_destination_card(
        &self,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let selectable = !self.busy && !self.loading_sessions;
        let source_available = selectable && self.selected_source().is_some();
        let mut destination_card = card(palette);
        for target in self.existing.clone() {
            let selected = self.selected_destination == Some(Destination::Existing(target.clone()));
            destination_card = destination_card.child(Self::destination_row(
                format!("handoff-tab-{}", target.tab_id),
                agent_icon(target.agent),
                format!("Tab {} · {}", target.position, target.agent.label()),
                target.label.clone(),
                source_available,
                selected,
                palette,
                move |this, window, cx| {
                    this.selected_destination = Some(Destination::Existing(target.clone()));
                    // Existing tabs cannot override the model; reset any ID
                    // typed for a previous new-tab pick and hide the control.
                    this.clear_model_input(window, cx);
                    log::info!("handoff: selected existing tab {}", target.tab_id);
                    cx.notify();
                },
                window,
                cx,
            ));
        }
        let picker_ready = !self.busy && !self.loading_agents;
        let new_tab_agent = match &self.selected_destination {
            Some(Destination::NewTab(agent)) => Some(*agent),
            _ => None,
        };
        destination_card = destination_card.child(Self::destination_row(
            "handoff-new-tab",
            new_tab_agent.map(agent_icon).unwrap_or("phosphor/plus.svg"),
            new_tab_agent
                .map(|agent| format!("New {} Tab", agent.label()))
                .unwrap_or_else(|| "New Agent Tab".into()),
            if self.loading_agents {
                "Loading…".into()
            } else if new_tab_agent.is_some() {
                "Change…".into()
            } else {
                String::new()
            },
            picker_ready,
            new_tab_agent.is_some(),
            palette,
            |this, _, cx| this.set_picker_open(!this.picker_open, cx),
            window,
            cx,
        ));
        if self.picker_open {
            // Inline agent rows instead of a Select popup: the modal is
            // too short for the dropdown to open below its input without
            // overlapping it, and rows match the card idiom.
            for agent in self.target_agents() {
                let selected = self.selected_destination == Some(Destination::NewTab(agent));
                destination_card = destination_card.child(Self::destination_row(
                    format!("handoff-pick-{}", agent.label()),
                    agent_icon(agent),
                    agent.label().to_string(),
                    "Target: available".into(),
                    !self.busy,
                    selected,
                    palette,
                    move |this, window, cx| {
                        // Switching the target Agent resets the typed model.
                        if new_tab_agent != Some(agent) {
                            this.clear_model_input(window, cx);
                        }
                        this.selected_destination = Some(Destination::NewTab(agent));
                        this.ensure_model_input(window, cx);
                        if let Some(input) = &this.model_input {
                            input.read(cx).focus_handle(cx).focus(window, cx);
                        }
                        this.load_model_candidates(agent, cx);
                        log::info!("handoff: selected new {:?} tab", agent);
                        this.set_picker_open(false, cx);
                        cx.notify();
                    },
                    window,
                    cx,
                ));
            }
        }
        if new_tab_agent.is_some()
            && let Some(input) = self.model_input.clone()
        {
            destination_card = destination_card.child(
                card_row()
                    .gap_2()
                    .child(row_icon_tinted(
                        "phosphor/sliders.svg",
                        palette.muted_foreground,
                    ))
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Requested model"),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&input)
                                .appearance(false)
                                .focus_bordered(false)
                                .small()
                                .cleanable(true)
                                .disabled(self.busy)
                                .w_full(),
                        ),
                    ),
            );
            destination_card = destination_card.child(
                card_row()
                    .text_size(px(11.5))
                    .text_color(palette.muted_foreground)
                    .child("Requested only — the target Agent decides the actual model"),
            );
            if let Some(error) = self.model_error(cx) {
                destination_card = destination_card.child(
                    card_row()
                        .text_size(px(11.5))
                        .text_color(palette.danger)
                        .child(error),
                );
            }
            if !self.model_candidates.is_empty() {
                // Suggestion list: capped and scrollable so a long probe
                // result can never stretch the window; clicking a row
                // fills the exact-input control above. Typing in that
                // control filters the list (case-insensitive substring)
                // so long probe results stay searchable.
                let query = self.model_value(cx);
                let filtered = filter_model_candidates(&self.model_candidates, &query);
                let mut list = div()
                    .id("handoff-model-candidates")
                    .flex()
                    .flex_col()
                    .mx(px(10.0))
                    .mb(px(6.0))
                    .rounded(px(8.0))
                    .bg(palette.background.opacity(0.6))
                    .max_h(ROW_HEIGHT * 3.0)
                    .overflow_y_scroll();
                if filtered.is_empty() {
                    list = list.child(
                        div()
                            .flex()
                            .items_center()
                            .w_full()
                            .px(px(8.0))
                            .py(px(6.0))
                            .text_size(px(11.5))
                            .text_color(palette.muted_foreground.opacity(0.7))
                            .child("No matching models"),
                    );
                }
                for model in filtered {
                    let model = model.clone();
                    let active = query == model;
                    let mut row = div()
                        .id(format!("handoff-model-{model}"))
                        .flex()
                        .items_center()
                        .w_full()
                        .rounded(px(6.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .text_size(px(11.5))
                        .font_family(palette.mono_font_family.clone())
                        .text_color(if active {
                            palette.primary
                        } else {
                            palette.muted_foreground
                        })
                        .child(model.clone());
                    if self.busy {
                        row = row.opacity(0.45);
                    } else {
                        row = row
                            .cursor_pointer()
                            .hover(|style| style.bg(palette.muted.opacity(0.08)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(input) = &this.model_input {
                                    input.update(cx, |state, cx| {
                                        state.set_value(model.clone(), window, cx)
                                    });
                                }
                                cx.notify();
                            }));
                    }
                    list = list.child(row);
                }
                destination_card = destination_card.child(list);
            }
        }
        destination_card
    }
}

fn compact_session_id(id: &str) -> String {
    let prefix: String = id.chars().take(12).collect();
    if id.chars().count() > 12 {
        format!("ID {prefix}…")
    } else {
        format!("ID {prefix}")
    }
}

/// Case-insensitive substring filter for the probed model IDs. An empty or
/// whitespace-only query keeps the full list, so the control doubles as a
/// plain suggestion list until the user types a search term; the rows keep
/// their probe order.
fn filter_model_candidates<'a>(candidates: &'a [String], query: &str) -> Vec<&'a String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return candidates.iter().collect();
    }
    candidates
        .iter()
        .filter(|model| model.to_lowercase().contains(&query))
        .collect()
}

#[cfg(test)]
mod tests {
    // Explicit import on purpose: `use super::*` would pull gpui's `test`
    // attribute macro in through the view → destination → `gpui::*` glob
    // chain, making `#[test]` expand into itself (recursion limit error).
    use super::filter_model_candidates;

    fn sample_models() -> Vec<String> {
        [
            "auto",
            "gpt-5.3-codex-low",
            "gpt-5.3-codex-low-fast",
            "gpt-5.3-codex",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    #[test]
    fn filter_model_candidates_matches_substring_case_insensitively() {
        let models = sample_models();
        let hits: Vec<&str> = filter_model_candidates(&models, "CODEX")
            .into_iter()
            .map(|model| model.as_str())
            .collect();
        assert_eq!(
            hits,
            vec![
                "gpt-5.3-codex-low",
                "gpt-5.3-codex-low-fast",
                "gpt-5.3-codex"
            ]
        );
    }

    #[test]
    fn filter_model_candidates_keeps_full_list_for_blank_query() {
        let models = sample_models();
        assert_eq!(filter_model_candidates(&models, "").len(), models.len());
        assert_eq!(filter_model_candidates(&models, "   ").len(), models.len());
        assert!(filter_model_candidates(&models, "zzz").is_empty());
    }
}
