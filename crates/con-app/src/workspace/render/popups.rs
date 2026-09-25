use super::super::*;

impl ConWorkspace {
    pub(super) fn render_skill_popup(
        &self,
        terminal_content_left: f32,
        terminal_content_width: f32,
        elevated_ui_surface_opacity: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme();
        let popup_available_width = (terminal_content_width - 48.0).max(240.0);
        let popup_width = px((terminal_content_width * 0.34)
            .clamp(320.0, 480.0)
            .min(popup_available_width));
        let popup_bottom = self.input_bar.read(cx).skill_popup_offset(cx);
        let skills = self
            .input_bar
            .read(cx)
            .filtered_skills(cx)
            .into_iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect::<Vec<_>>();
        let sel = self.input_bar.read(cx).skill_selection();
        let sel = sel.min(skills.len().saturating_sub(1));

        let mut popup = div()
            .absolute()
            .bottom(popup_bottom)
            .left(px(terminal_content_left + 24.0))
            .w(popup_width)
            .max_h(px(320.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .bg(theme.background.opacity(elevated_ui_surface_opacity))
            .border_1()
            .border_color(theme.muted.opacity(0.16))
            .py(px(6.0))
            .overflow_hidden()
            .font_family(theme.font_family.clone());

        for (i, (name, desc)) in skills.iter().enumerate() {
            let is_sel = i == sel;
            let name_clone = name.clone();
            popup = popup.child(
                div()
                    .id(SharedString::from(format!("skill-{name}")))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .mx(px(6.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .bg(if is_sel {
                        theme.primary.opacity(0.10)
                    } else {
                        theme.transparent
                    })
                    .hover(|s| s.bg(theme.primary.opacity(0.08)))
                    .on_mouse_down(MouseButton::Left, {
                        let input_bar = self.input_bar.clone();
                        cx.listener(move |_this, _, window, cx| {
                            input_bar.update(cx, |bar, cx| {
                                bar.complete_skill(&name_clone, window, cx);
                            });
                        })
                    })
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if is_sel {
                                theme.primary
                            } else {
                                theme.foreground
                            })
                            .child(format!("/{name}")),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(theme.muted_foreground.opacity(0.68))
                            .child(desc.clone()),
                    ),
            );
        }
        Some(popup.into_any_element())
    }

    pub(super) fn render_path_popup(
        &self,
        terminal_content_left: f32,
        terminal_content_width: f32,
        elevated_ui_surface_opacity: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme();
        let popup_available_width = (terminal_content_width - 48.0).max(240.0);
        let popup_width = px((terminal_content_width * 0.32)
            .clamp(320.0, 440.0)
            .min(popup_available_width));
        let popup_bottom = self.input_bar.read(cx).skill_popup_offset(cx);
        let candidates = self.input_bar.read(cx).path_completion_candidates();
        let sel = self
            .input_bar
            .read(cx)
            .path_completion_selection()
            .min(candidates.len().saturating_sub(1));

        let mut popup = div()
            .absolute()
            .bottom(popup_bottom)
            .left(px(terminal_content_left + 24.0))
            .w(popup_width)
            .max_h(px(280.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .bg(theme.background.opacity(elevated_ui_surface_opacity))
            .border_1()
            .border_color(theme.muted.opacity(0.16))
            .py(px(6.0))
            .overflow_hidden()
            .font_family(theme.mono_font_family.clone());

        for (i, candidate) in candidates.iter().enumerate() {
            let is_sel = i == sel;
            let candidate_ix = i;
            popup = popup.child(
                div()
                    .id(SharedString::from(format!("path-candidate-{i}")))
                    .mx(px(6.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .bg(if is_sel {
                        theme.primary.opacity(0.10)
                    } else {
                        theme.transparent
                    })
                    .hover(|s| s.bg(theme.primary.opacity(0.08)))
                    .on_mouse_down(MouseButton::Left, {
                        let input_bar = self.input_bar.clone();
                        cx.listener(move |_this, _, window, cx| {
                            input_bar.update(cx, |bar, cx| {
                                let _ = bar.accept_path_completion_candidate_at(
                                    candidate_ix,
                                    window,
                                    cx,
                                );
                            });
                        })
                    })
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_family(theme.mono_font_family.clone())
                            .text_color(if is_sel {
                                theme.primary
                            } else {
                                theme.foreground
                            })
                            .child(candidate.clone()),
                    ),
            );
        }
        Some(popup.into_any_element())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_pane_scope_picker(
        &self,
        terminal_content_left: f32,
        terminal_content_width: f32,
        input_bar_progress: f32,
        ui_surface_opacity: f32,
        elevated_ui_surface_opacity: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.pane_scope_picker_open
            && self.input_bar_visible
            && self.input_bar.read(cx).mode() != InputMode::Agent
        {
            let panes = self.input_bar.read(cx).pane_infos();
            if panes.len() > 1 {
                let focused_id = self.input_bar.read(cx).focused_pane_id();
                let selected_ids: HashSet<usize> = self
                    .input_bar
                    .read(cx)
                    .scope_selected_ids()
                    .into_iter()
                    .collect();
                let is_broadcast = self.input_bar.read(cx).is_broadcast_scope();
                let is_focused = self.input_bar.read(cx).is_focused_scope();
                let layout = self.active_pane_layout(cx);
                let pane_map: HashMap<usize, PaneInfo> =
                    panes.iter().cloned().map(|pane| (pane.id, pane)).collect();
                let display_indices: HashMap<usize, usize> = panes
                    .iter()
                    .enumerate()
                    .map(|(ix, pane)| (pane.id, ix))
                    .collect();
                let popup_available_width = (terminal_content_width - 48.0).max(300.0);
                let popup_width = px((terminal_content_width * 0.38)
                    .clamp(380.0, 540.0)
                    .min(popup_available_width));
                let popup_bottom = px(62.0 + (43.0 * input_bar_progress.max(0.01)));
                let preview_content = self.render_scope_node(
                    &layout,
                    &pane_map,
                    &display_indices,
                    &selected_ids,
                    focused_id,
                    cx,
                );
                let theme = cx.theme();
                let popup_surface = if theme.is_dark() {
                    theme
                        .background
                        .opacity(elevated_ui_surface_opacity.max(0.96))
                } else {
                    theme.title_bar.opacity(0.99)
                };
                let preview_surface = if theme.is_dark() {
                    theme.title_bar.opacity(ui_surface_opacity * 0.98)
                } else {
                    theme.foreground.opacity(0.038)
                };
                let segmented_surface = if theme.is_dark() {
                    theme.title_bar.opacity(ui_surface_opacity * 0.96)
                } else {
                    theme.foreground.opacity(0.035)
                };
                let scope_frame_inset = px(3.0);
                let scope_frame_radius = px(9.0);
                let local_keycap = |label: String| {
                    let wide = label.chars().count() > 1;
                    div()
                        .h(px(17.0))
                        .min_w(if wide { px(28.0) } else { px(17.0) })
                        .px(px(if wide { 6.0 } else { 0.0 }))
                        .rounded(px(4.5))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme.foreground.opacity(0.048))
                        .text_size(px(9.5))
                        .line_height(px(11.0))
                        .font_family(theme.mono_font_family.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.foreground.opacity(0.66))
                        .child(label)
                };

                let presets = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(14.0))
                    .child(
                        div().flex().items_center().gap(px(6.0)).child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_family(theme.mono_font_family.clone())
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.foreground.opacity(0.86))
                                        .child("Pane scope"),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.0))
                                        .line_height(px(13.0))
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground.opacity(0.52))
                                        .child("Choose where input is sent"),
                                ),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .px(px(6.0))
                            .py(px(4.0))
                            .rounded(px(8.0))
                            .bg(theme.foreground.opacity(0.026))
                            .child(
                                div()
                                    .text_size(px(9.5))
                                    .line_height(px(11.0))
                                    .font_family(theme.mono_font_family.clone())
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.muted_foreground.opacity(0.56))
                                    .child("Toggle"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(3.0))
                                    .child(local_keycap("1-9".to_string()))
                                    .child(local_keycap("A".to_string()))
                                    .child(local_keycap("F".to_string())),
                            ),
                    );

                let preset_segment =
                    |id: &'static str, icon: &'static str, label: &'static str, active: bool| {
                        div().flex_1().child(
                            div()
                                .id(SharedString::from(id))
                                .h(px(23.0))
                                .w_full()
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(6.0))
                                .bg(if active {
                                    theme.primary.opacity(0.11)
                                } else {
                                    theme.transparent
                                })
                                .hover(|s| {
                                    s.bg(if active {
                                        theme.primary.opacity(0.14)
                                    } else {
                                        theme.foreground.opacity(0.045)
                                    })
                                })
                                .child(svg().path(icon).size(mono_icon_px(theme, 12.0)).text_color(
                                    if active {
                                        theme.primary.opacity(0.88)
                                    } else {
                                        theme.muted_foreground.opacity(0.48)
                                    },
                                ))
                                .child(
                                    div()
                                        .text_size(px(10.8))
                                        .line_height(px(13.0))
                                        .font_family(theme.mono_font_family.clone())
                                        .font_weight(if active {
                                            FontWeight::MEDIUM
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(if active {
                                            theme.primary.opacity(0.94)
                                        } else {
                                            theme.muted_foreground.opacity(0.68)
                                        })
                                        .child(label),
                                ),
                        )
                    };

                let presets_row = div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w_full()
                            .h(px(31.0))
                            .p(scope_frame_inset)
                            .rounded(scope_frame_radius)
                            .bg(segmented_surface)
                            .flex()
                            .items_center()
                            .gap(px(2.0))
                            .child(
                                preset_segment(
                                    "scope-all",
                                    "phosphor/squares-four.svg",
                                    "All panes",
                                    is_broadcast,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(
                                        |this: &mut ConWorkspace,
                                         _: &MouseDownEvent,
                                         window,
                                         cx| {
                                            this.set_scope_broadcast(window, cx);
                                        },
                                    ),
                                ),
                            )
                            .child(
                                preset_segment(
                                    "scope-focused",
                                    "phosphor/target.svg",
                                    "Focused",
                                    is_focused,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(
                                        |this: &mut ConWorkspace,
                                         _: &MouseDownEvent,
                                         window,
                                         cx| {
                                            this.set_scope_focused(window, cx);
                                        },
                                    ),
                                ),
                            ),
                    )
                    .child(div().flex_1());

                let preview = div()
                    .h(px(214.0))
                    .w_full()
                    .rounded(scope_frame_radius)
                    .p(scope_frame_inset)
                    .bg(preview_surface)
                    .child(preview_content);

                return Some(
                    div()
                        .absolute()
                        .left(px(terminal_content_left + 20.0))
                        .bottom(popup_bottom)
                        .w(popup_width)
                        .rounded(px(12.0))
                        .bg(popup_surface)
                        .p(px(11.0))
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(presets)
                        .child(presets_row)
                        .child(preview)
                        .into_any_element(),
                );
            }
        }

        None
    }

    pub(super) fn render_inline_skill_popup(
        &self,
        elevated_ui_surface_opacity: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = cx.theme();
        let popup_bottom = self.agent_panel.read(cx).inline_skill_popup_offset(cx);
        let effective_agent_panel_width = self
            .agent_panel_width
            .min(max_agent_panel_width(window.bounds().size.width.as_f32()));
        let panel_left = window.bounds().size.width.as_f32() - effective_agent_panel_width;
        let popup_left = px((panel_left + 8.0).max(24.0));
        let popup_width = px((effective_agent_panel_width - 24.0).clamp(180.0, 320.0));
        let skills = self
            .agent_panel
            .read(cx)
            .filtered_inline_skills(cx)
            .into_iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect::<Vec<_>>();
        let sel = self.agent_panel.read(cx).inline_skill_selection();
        let sel = sel.min(skills.len().saturating_sub(1));

        let mut popup = div()
            .absolute()
            .bottom(popup_bottom)
            .left(popup_left)
            .w(popup_width)
            .max_h(px(280.0))
            .flex()
            .flex_col()
            .rounded(px(10.0))
            .bg(theme.background.opacity(elevated_ui_surface_opacity))
            .border_1()
            .border_color(theme.muted.opacity(0.16))
            .py(px(6.0))
            .overflow_hidden()
            .font_family(theme.font_family.clone());

        for (i, (name, desc)) in skills.iter().enumerate() {
            let is_sel = i == sel;
            let name_clone = name.clone();
            popup = popup.child(
                div()
                    .id(SharedString::from(format!("inline-skill-{name}")))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .mx(px(6.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .bg(if is_sel {
                        theme.primary.opacity(0.10)
                    } else {
                        theme.transparent
                    })
                    .hover(|s| s.bg(theme.primary.opacity(0.08)))
                    .on_mouse_down(MouseButton::Left, {
                        let agent_panel = self.agent_panel.clone();
                        cx.listener(move |_this, _, window, cx| {
                            agent_panel.update(cx, |panel, cx| {
                                panel.complete_inline_skill(&name_clone, window, cx);
                            });
                        })
                    })
                    .child(
                        div()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if is_sel {
                                theme.primary
                            } else {
                                theme.foreground
                            })
                            .child(format!("/{name}")),
                    )
                    .child(
                        div()
                            .text_size(px(11.0))
                            .line_height(px(16.0))
                            .text_color(theme.muted_foreground.opacity(0.68))
                            .child(desc.clone()),
                    ),
            );
        }
        Some(popup.into_any_element())
    }
}
