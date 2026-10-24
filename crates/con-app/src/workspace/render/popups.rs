use super::super::*;
use gpui_component::scroll::ScrollableElement;

use super::super::chrome_actions::PANE_SCOPE_GAP;

fn pane_scope_frame(theme: &gpui_component::Theme) -> gpui::Div {
    div()
        .w_full()
        .min_w_0()
        .p(px(3.0))
        .rounded(px(9.0))
        .bg(theme.foreground.opacity(0.025))
}

#[cfg(test)]
mod tests {
    use super::{PANE_SCOPE_GAP, pane_scope_frame, pane_scope_popup_geometry};
    use gpui::px;

    #[test]
    fn pane_targets_follow_control_and_multiline_height() {
        let (left, width, bottom) = pane_scope_popup_geometry(230.0, 800.0, 44.0, 1.0, 1.0);
        assert_eq!((left, width, bottom), (px(274.0), px(320.0), px(54.0)));
        assert_eq!(
            pane_scope_popup_geometry(230.0, 800.0, 84.0, 1.0, 1.0).2,
            px(94.0)
        );
    }

    #[test]
    fn pane_targets_fit_terminal_gutters_at_scaled_and_narrow_widths() {
        for scale in [1.0, 1.5] {
            for terminal_left in [0.0, 230.0] {
                for terminal_width in [240.0, 420.0, 1100.0] {
                    let (left, width, bottom) = pane_scope_popup_geometry(
                        terminal_left,
                        terminal_width,
                        84.0 * scale,
                        0.5,
                        scale,
                    );
                    assert!(left >= px(terminal_left + 12.0 * scale));
                    assert!(left + width <= px(terminal_left + terminal_width - 12.0 * scale));
                    assert_eq!(bottom, px(52.0 * scale));
                }
            }
        }
    }

    #[gpui::test]
    fn preset_and_equal_split_frames_share_edges_and_midline(cx: &mut gpui::TestAppContext) {
        use gpui::{
            Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div,
            px, size,
        };
        use gpui_component::ActiveTheme;

        struct Frames;
        impl Render for Frames {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let theme = cx.theme();
                let segments = pane_scope_frame(theme)
                    .debug_selector(|| "presets".into())
                    .h(px(34.0))
                    .flex()
                    .gap(px(PANE_SCOPE_GAP))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .debug_selector(|| "preset-left".into()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .debug_selector(|| "preset-right".into()),
                    );
                let map = pane_scope_frame(theme)
                    .debug_selector(|| "map".into())
                    .h(px(128.0))
                    .child(
                        div()
                            .flex()
                            .size_full()
                            .gap(px(PANE_SCOPE_GAP))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .debug_selector(|| "pane-left".into()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .debug_selector(|| "pane-right".into()),
                            ),
                    );
                div()
                    .w_full()
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(segments)
                    .child(map)
            }
        }
        cx.update(gpui_component::init);
        let (_, view) = cx.add_window_view(|_, _| Frames);
        for width in [240.0, 320.0, 440.0] {
            view.simulate_resize(size(px(width), px(250.0)));
            let presets = view.debug_bounds("presets").unwrap();
            let map = view.debug_bounds("map").unwrap();
            assert_eq!(presets.left(), map.left());
            assert_eq!(presets.right(), map.right());
            for (preset, pane) in [("preset-left", "pane-left"), ("preset-right", "pane-right")] {
                let preset = view.debug_bounds(preset).unwrap();
                let pane = view.debug_bounds(pane).unwrap();
                assert_eq!(preset.left(), pane.left());
                assert_eq!(preset.right(), pane.right());
            }
        }
    }
}

fn pane_scope_popup_geometry(
    terminal_left: f32,
    terminal_width: f32,
    input_height: f32,
    progress: f32,
    scale: f32,
) -> (Pixels, Pixels, Pixels) {
    let inset = 12.0 * scale;
    let width = (terminal_width * 0.38)
        .clamp(320.0 * scale, 440.0 * scale)
        .min((terminal_width - 2.0 * inset).max(0.0));
    let anchor = (crate::input_chrome::BAR_INSET
        + crate::input_chrome::CONTROL_SIZE
        + crate::input_chrome::CONTROL_GAP)
        * scale;
    let left = (terminal_left + anchor)
        .min(terminal_left + terminal_width - width - inset)
        .max(terminal_left + inset);
    let bottom = input_height * progress.clamp(0.0, 1.0) + 10.0 * scale;
    (px(left), px(width), px(bottom))
}

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
        #[cfg(target_os = "macos")]
        let popup_bottom = popup_bottom
            + px((frame::CANVAS_INSET + frame::INPUT_BOTTOM_INSET) * self.canvas_depth_progress);
        let skills = self
            .input_bar
            .read(cx)
            .filtered_skills(cx)
            .into_iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect::<Vec<_>>();
        let sel = self.input_bar.read(cx).skill_selection();
        let sel = sel.min(skills.len().saturating_sub(1));

        let scroll = self.input_bar.read(cx).skill_popup_scroll_handle();
        let mut popup = div()
            .id("skill-popup")
            .role(Role::ListBox)
            .debug_selector(|| "skill-popup".into())
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
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .occlude()
            .font_family(theme.font_family.clone());

        for (i, (name, desc)) in skills.iter().enumerate() {
            let is_sel = i == sel;
            let name_clone = name.clone();
            popup = popup.child(
                div()
                    .id(SharedString::from(format!("skill-{name}")))
                    .role(Role::ListBoxOption)
                    .aria_selected(is_sel)
                    .aria_label(format!("/{name}"))
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
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
        Some(popup.vertical_scrollbar(&scroll).into_any_element())
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
        #[cfg(target_os = "macos")]
        let popup_bottom = popup_bottom
            + px((frame::CANVAS_INSET + frame::INPUT_BOTTOM_INSET) * self.canvas_depth_progress);
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
        _ui_surface_opacity: f32,
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
                let (popup_left, popup_width, popup_bottom) = pane_scope_popup_geometry(
                    terminal_content_left,
                    terminal_content_width,
                    self.input_bar.read(cx).rendered_height(cx).as_f32() + {
                        #[cfg(target_os = "macos")]
                        {
                            (frame::CANVAS_INSET + frame::INPUT_BOTTOM_INSET)
                                * self.canvas_depth_progress
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            0.0
                        }
                    },
                    input_bar_progress,
                    crate::ui_scale::mono_density_scale(cx.theme()),
                );
                let preview_content = self.render_scope_node(
                    &layout,
                    &pane_map,
                    &display_indices,
                    &selected_ids,
                    focused_id,
                    cx,
                );
                let theme = cx.theme();
                let popup_surface = theme
                    .background
                    .blend(theme.foreground.opacity(0.035))
                    .opacity(elevated_ui_surface_opacity.max(0.99));
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
                        .text_color(theme.muted_foreground)
                        .child(label)
                };

                let presets = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .font_family(theme.font_family.clone())
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Send to"),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .line_height(px(13.0))
                            .font_family(theme.font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(format!("{} of {} panes", selected_ids.len(), panes.len())),
                    );

                let preset_segment =
                    |id: &'static str, icon: &'static str, label: &'static str, active: bool| {
                        div().flex_1().min_w_0().child(
                            div()
                                .id(SharedString::from(id))
                                .h(px(28.0))
                                .w_full()
                                .rounded(px(6.0))
                                .cursor_pointer()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(6.0))
                                .bg(if active {
                                    theme.background.blend(theme.foreground.opacity(0.015))
                                } else {
                                    theme.transparent
                                })
                                .hover(|s| {
                                    s.bg(if active {
                                        theme.background.blend(theme.foreground.opacity(0.035))
                                    } else {
                                        theme.foreground.opacity(0.045)
                                    })
                                })
                                .child(svg().path(icon).size(mono_icon_px(theme, 12.0)).text_color(
                                    if active {
                                        theme.primary
                                    } else {
                                        theme.muted_foreground
                                    },
                                ))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .line_height(px(13.0))
                                        .font_family(theme.font_family.clone())
                                        .font_weight(if active {
                                            FontWeight::MEDIUM
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(if active {
                                            theme.foreground
                                        } else {
                                            theme.muted_foreground
                                        })
                                        .child(label),
                                )
                                .child(local_keycap(
                                    if id == "scope-all" { "A" } else { "F" }.to_string(),
                                )),
                        )
                    };

                let presets_row = pane_scope_frame(theme)
                    .debug_selector(|| "pane-scope-presets".into())
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .gap(px(PANE_SCOPE_GAP))
                    .child(
                        preset_segment(
                            "scope-all",
                            "phosphor/broadcast-duotone.svg",
                            "All panes",
                            is_broadcast,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(
                                |this: &mut ConWorkspace, _: &MouseDownEvent, window, cx| {
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
                                |this: &mut ConWorkspace, _: &MouseDownEvent, window, cx| {
                                    this.set_scope_focused(window, cx);
                                },
                            ),
                        ),
                    );

                let preview = pane_scope_frame(theme)
                    .debug_selector(|| "pane-scope-map".into())
                    .h(px(if panes.len() <= 3 {
                        128.0
                    } else if panes.len() <= 6 {
                        184.0
                    } else {
                        224.0
                    }))
                    .child(preview_content);

                return Some(
                    div()
                        .debug_selector(|| "pane-scope-popup".into())
                        .absolute()
                        .left(popup_left)
                        .bottom(popup_bottom)
                        .w(popup_width)
                        .rounded(px(12.0))
                        .bg(popup_surface)
                        .p(px(12.0))
                        .font_family(theme.font_family.clone())
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
