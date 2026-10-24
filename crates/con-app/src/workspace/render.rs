mod popups;
mod top_bar;

use super::*;

fn workspace_focus_container(focus: &FocusHandle) -> Div {
    div()
        .track_focus(focus)
        // This handle provides shortcut ancestry and editor-only fallback, not
        // an input target for titlebar/divider clicks. Child focus runs first.
        .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}

/// A quiet grip in the surface gutter, with a wider hit target that does not
/// consume layout or paint a full-height stripe over either surface.
fn column_resize_handle(
    id: &'static str,
    center: f32,
    active: bool,
    foreground: Hsla,
    dark: bool,
) -> Stateful<Div> {
    let grip_color = foreground.opacity(if active { 0.34 } else { 0.0 });
    div()
        .id(id)
        .debug_selector(move || id.into())
        .group(id)
        .absolute()
        .top_0()
        .bottom_0()
        .left(px(center - 3.5))
        .w(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_col_resize()
        .occlude()
        .child(
            div()
                .debug_selector(|| "column-resize-grip".into())
                .w(px(2.0))
                .h(px(28.0))
                .max_h(relative(0.35))
                .rounded_full()
                .bg(grip_color)
                .group_hover(id, |style| {
                    style.bg(foreground.opacity(if active {
                        0.34
                    } else if dark {
                        0.24
                    } else {
                        0.18
                    }))
                }),
        )
}

#[cfg(target_os = "linux")]
const LINUX_WINDOW_CORNER_RADIUS: Pixels = px(14.0);

impl ConWorkspace {
    fn has_active_resize_drag(&self) -> bool {
        self.sidebar_drag.is_some()
            || self.agent_panel_drag.is_some()
            || self
                .tabs
                .get(self.active_tab)
                .is_some_and(|tab| tab.pane_tree.is_dragging())
    }

    fn finish_active_resize_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let mut should_save_session = false;

        if self.sidebar_drag.take().is_some() {
            changed = true;
            should_save_session = true;
        }

        if self.agent_panel_drag.take().is_some() {
            changed = true;
            should_save_session = true;
        }

        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            let pane_tree = &mut tab.pane_tree;
            if pane_tree.is_dragging() {
                pane_tree.end_drag();
                for terminal in pane_tree.all_terminals() {
                    terminal.notify(cx);
                }
                changed = true;
            }
        }

        if should_save_session {
            self.save_session(cx);
        }
        if changed {
            cx.notify();
        }

        changed
    }

    fn leading_panel_width_for_pane_resize(&self, cx: &mut Context<Self>) -> f32 {
        let progress = self.left_panel_motion.current();
        if progress <= 0.0 {
            return 0.0;
        }

        let sidebar = self.sidebar.read(cx);
        if self.vertical_tabs_enabled() {
            if self.sidebar_tools_open {
                sidebar.effective_panel_width() * progress
            } else {
                sidebar.rendered_width() * progress
            }
        } else {
            sidebar.effective_panel_width() * progress
        }
    }

    fn handle_active_resize_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        win: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.has_active_resize_drag() {
            return false;
        }

        if !resize_drag_should_continue(event.pressed_button) {
            self.finish_active_resize_drag(cx);
            return true;
        }

        if let Some((start_x, start_width)) = self.sidebar_drag {
            let win_w = win.bounds().size.width.as_f32();
            let agent_w = if self.agent_panel_open {
                self.agent_panel_width.min(max_agent_panel_width(win_w)) + 1.0
            } else {
                0.0
            };
            let max_width = max_sidebar_panel_width(win_w, agent_w);
            let delta = f32::from(event.position.x) - start_x;
            let new_width = (start_width + delta).clamp(PANEL_MIN_WIDTH, max_width);
            let current_width = self.sidebar.read(cx).panel_width();
            if (current_width - new_width).abs() > 0.5 {
                self.sidebar
                    .update(cx, |sidebar, cx| sidebar.set_panel_width(new_width, cx));
                cx.notify();
            }
            return true;
        }

        if let Some((start_x, start_width)) = self.agent_panel_drag {
            let delta = start_x - f32::from(event.position.x);
            let leading = self.leading_panel_width_for_pane_resize(cx);
            let max_width = max_agent_panel_width(win.bounds().size.width.as_f32() - leading);
            let new_width = (start_width + delta).clamp(AGENT_PANEL_MIN_WIDTH, max_width);
            if (self.agent_panel_width - new_width).abs() > 1.0 {
                self.agent_panel_width = new_width;
                cx.notify();
            }
            return true;
        }

        if self.active_tab >= self.tabs.len() {
            return true;
        }

        let top_bar_height = self.current_top_bar_height();
        let input_bar_height =
            self.input_bar.read(cx).rendered_height(cx).as_f32() * self.input_bar_motion.current();
        let win_w = f32::from(win.bounds().size.width);
        let win_h = f32::from(win.bounds().size.height);
        let effective_agent_panel_width = self.agent_panel_width.min(max_agent_panel_width(win_w));
        let agent_panel_drag_width = if self.agent_panel_open {
            effective_agent_panel_width + 7.0
        } else {
            0.0
        };
        let leading_panel_w = self.leading_panel_width_for_pane_resize(cx);
        #[cfg(target_os = "macos")]
        let rendered_bounds = self
            .pane_content_bounds
            .lock()
            .ok()
            .and_then(|bounds| *bounds);

        let pane_tree = &mut self.tabs[self.active_tab].pane_tree;
        if !pane_tree.is_dragging() {
            return false;
        }

        let (current_pos, total_size) = if let Some(dir) = pane_tree.dragging_direction() {
            #[cfg(target_os = "macos")]
            if let Some(bounds) = rendered_bounds {
                let (position, extent) = match dir {
                    SplitDirection::Horizontal => {
                        (event.position.x - bounds.left(), bounds.size.width)
                    }
                    SplitDirection::Vertical => {
                        (event.position.y - bounds.top(), bounds.size.height)
                    }
                };
                if pane_tree.update_drag(position.as_f32(), extent.as_f32()) {
                    cx.notify();
                }
                return true;
            }
            match dir {
                SplitDirection::Horizontal => (
                    f32::from(event.position.x) - leading_panel_w,
                    win_w - agent_panel_drag_width - leading_panel_w,
                ),
                SplitDirection::Vertical => (
                    f32::from(event.position.y),
                    win_h - top_bar_height - input_bar_height,
                ),
            }
        } else {
            return true;
        };

        if pane_tree.update_drag(current_pos, total_size) {
            cx.notify();
        }
        true
    }
}

impl Render for ConWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.flush_pending_create_pane_requests(window, cx);

        if !self.has_active_tab() {
            return div().size_full().into_any_element();
        }

        let active_terminal = self.try_active_terminal().cloned();

        // If a modal was dismissed internally (escape/backdrop), restore terminal focus
        let is_modal_open = self.is_modal_open(cx);
        let has_skill_popup = !self.input_bar.read(cx).filtered_skills(cx).is_empty();
        let has_path_popup = self.input_bar.read(cx).has_path_completion_candidates();
        let has_inline_skill_popup = self.agent_panel_open
            && !self.input_bar_visible
            && !self
                .agent_panel
                .read(cx)
                .filtered_inline_skills(cx)
                .is_empty();
        let needs_ghostty_hidden = false;

        // A palette action can hand focus to a newly opened inline editor.
        // Do not override that explicit transfer when dismissing the palette.
        if self.modal_was_open && !is_modal_open && self.surface_rename.is_none() {
            self.focus_terminal(window, cx);
        }
        // Manage ghostty NSView visibility separately — hide for modals AND skill popup
        if needs_ghostty_hidden && !self.ghostty_hidden {
            self.set_ghostty_views_visible(false, cx);
            self.ghostty_hidden = true;
        } else if !needs_ghostty_hidden && self.ghostty_hidden {
            self.set_ghostty_views_visible(true, cx);
            self.ghostty_hidden = false;
            let terminals: Vec<TerminalPane> = self.tabs[self.active_tab]
                .pane_tree
                .all_terminals()
                .into_iter()
                .cloned()
                .collect();
            cx.on_next_frame(window, move |_workspace, _window, cx| {
                for terminal in &terminals {
                    terminal.refresh_surface(cx);
                }
            });
        }
        self.modal_was_open = is_modal_open;

        // Keep pane focus in sync with which terminal has window focus
        let previous_focus = self.tabs[self.active_tab].pane_tree.focused_pane_id();
        self.tabs[self.active_tab].pane_tree.sync_focus(window, cx);
        self.chrome_preparation_dirty |=
            previous_focus != self.tabs[self.active_tab].pane_tree.focused_pane_id();
        if std::mem::take(&mut self.chrome_preparation_dirty) {
            log::trace!(target: "con::activity", "chrome_prepare");
            self.refresh_cached_tab_presentation(cx);
            self.reconcile_runtime_trackers_for_tab(self.active_tab);

            // Sync pane info and CWD to input bar
            let pane_tree = &self.tabs[self.active_tab].pane_tree;
            let focused_pane_id = pane_tree.focused_pane_id();
            let pane_infos: Vec<PaneInfo> = pane_tree
                .pane_terminals()
                .into_iter()
                .map(|(id, terminal)| {
                    let hostname = self
                        .cached_runtime_for_tab(self.active_tab, &terminal)
                        .and_then(|runtime| runtime.remote_host);
                    let title = terminal.title(cx);
                    let current_dir = terminal.current_dir(cx);
                    let name = pane_display_name(&hostname, &title, &current_dir, id);
                    let is_busy = terminal.is_busy(cx);
                    let is_alive = terminal.is_alive(cx);
                    PaneInfo {
                        id,
                        name,
                        hostname,
                        is_busy,
                        is_alive,
                    }
                })
                .collect();

            let cwd = active_terminal.as_ref().and_then(|t| t.current_dir(cx));
            if self.file_tree_view.read(cx).root().is_none() {
                self.sync_file_tree_from_active_focus(cx);
            }
            let display_cwd = cwd
                .map(|cwd| match dirs::home_dir() {
                    Some(home) => {
                        let home_str = home.to_string_lossy().to_string();
                        if cwd.starts_with(&home_str) {
                            format!("~{}", &cwd[home_str.len()..])
                        } else {
                            cwd
                        }
                    }
                    None => cwd,
                })
                .unwrap_or_else(|| "~".to_string());

            let skill_entries: Vec<crate::input_bar::SkillEntry> = self
                .harness
                .skill_summaries()
                .into_iter()
                .map(|(name, desc)| crate::input_bar::SkillEntry {
                    name,
                    description: desc,
                })
                .collect();
            self.input_bar.update(cx, |bar, cx| {
                bar.set_panes(pane_infos, focused_pane_id, window, cx);
                bar.set_cwd(display_cwd, cx);
                bar.set_skills(skill_entries.clone(), cx);
            });
            // Up/Down is command-bar recall, not shell suggestion ranking. Keep it
            // backed by the global submitted-input history across all modes.
            let recent_commands = self.recent_input_history(80);
            self.input_bar
                .update(cx, |bar, cx| bar.set_recent_commands(recent_commands, cx));

            // Sync model name, inline input, and skills to agent panel
            let active_agent_config = self.active_tab_agent_config();
            let model_name = AgentHarness::active_model_name_for(&active_agent_config);
            let provider = active_agent_config.provider.clone();
            let available_models = self.provider_models_for_config(&active_agent_config);
            let show_inline = !self.input_bar_visible && self.agent_panel_open;
            self.agent_panel.update(cx, |panel, cx| {
                panel.set_session_provider_options(
                    AgentPanel::configured_session_providers(&active_agent_config),
                    window,
                    cx,
                );
                panel.set_provider_name(provider, window, cx);
                panel.set_model_name(model_name, cx);
                panel.set_session_model_options(available_models, window, cx);
                panel.set_show_inline_input(show_inline, cx);
                panel.set_skills(skill_entries, cx);
                panel.set_recent_inputs(self.recent_input_history(80), cx);
            });
        }

        // Resolve boolean state here so all reveal paths (keyboard, Files,
        // agent requests, restored state) share the same motion contract.
        self.left_panel_motion
            .set_target(if self.left_panel_open { 1.0 } else { 0.0 });
        self.agent_panel_motion
            .set_target(if self.agent_panel_open { 1.0 } else { 0.0 });
        let left_panel_progress = self.left_panel_motion.value(window, cx);
        #[cfg(target_os = "macos")]
        {
            let tools_open = (self.left_panel_open || left_panel_progress > 0.0)
                && self.vertical_tabs_enabled()
                && self.sidebar_tools_open;
            let slot = self.activity_slot;
            self.sidebar.update(cx, |sidebar, _| {
                sidebar.set_tools_panel_open(tools_open, slot)
            });
        }
        let rail_transitioning = self
            .sidebar
            .update(cx, |sidebar, cx| sidebar.advance_width_motion(window, cx));
        let left_panel_transitioning = self.left_panel_motion.is_animating() || rail_transitioning;
        #[cfg(target_os = "macos")]
        let caption = {
            let minimum = frame::caption_minimum_width(top_bar::top_bar_leading_pad(window));
            let sidebar = self.sidebar.read(cx);
            let eligible = self.vertical_tabs_enabled();
            // A rail-only override preserves the expanded preference. It must
            // not preserve the expanded caption geometry as well.
            self.caption_dock = frame::CaptionLayout::new(if eligible {
                sidebar.caption_dock_progress(left_panel_progress, minimum)
            } else {
                0.0
            });
            self.caption_dock
        };
        self.input_bar_motion
            .set_target(if self.input_bar_visible { 1.0 } else { 0.0 });
        let agent_panel_progress = self.agent_panel_motion.value(window, cx);
        let input_bar_progress = self.input_bar_motion.value(window, cx);
        #[cfg(target_os = "macos")]
        let canvas_depth = {
            let depth = frame::canvas_depth_for_layout(
                self.horizontal_tabs_visible(),
                self.has_active_tab() && self.tabs[self.active_tab].pane_tree.pane_count() > 1,
                left_panel_progress,
                agent_panel_progress,
            );
            self.canvas_depth_progress = depth;
            depth
        };
        let tab_strip_progress = self.tab_strip_motion.value(window, cx);
        let agent_panel_transitioning = self.agent_panel_motion.is_animating();
        let input_bar_transitioning = self.input_bar_motion.is_animating();
        let tab_strip_transitioning = self.tab_strip_motion.is_animating();
        #[cfg(target_os = "macos")]
        let (agent_panel_snap_guard_active, agent_panel_snap_guard_expired) =
            Self::snap_guard_state(&mut self.agent_panel_snap_guard_until, window);
        #[cfg(not(target_os = "macos"))]
        let agent_panel_snap_guard_active = false;
        #[cfg(target_os = "macos")]
        {
            let release_cover = Duration::from_millis(CHROME_RELEASE_COVER_MS);
            if agent_panel_snap_guard_expired && !self.agent_panel_open {
                Self::extend_guard(&mut self.agent_panel_release_cover_until, release_cover);
            }
        }
        #[cfg(target_os = "macos")]
        let agent_panel_release_cover_active =
            Self::snap_guard_active(&mut self.agent_panel_release_cover_until, window);
        #[cfg(not(target_os = "macos"))]
        let agent_panel_release_cover_active = false;
        #[cfg(target_os = "macos")]
        {
            let allow_native_transition_underlay = self.terminal_opacity >= 0.999;
            let guard_active = if allow_native_transition_underlay {
                if let Some(until) = self.chrome_transition_underlay_until {
                    if Instant::now() < until {
                        window.request_animation_frame();
                        true
                    } else {
                        self.chrome_transition_underlay_until = None;
                        false
                    }
                } else {
                    false
                }
            } else {
                if self.chrome_transition_underlay_until.is_some() {
                    self.chrome_transition_underlay_until = None;
                }
                false
            };
            let pane_dragging =
                self.has_active_tab() && self.tabs[self.active_tab].pane_tree.is_dragging();
            let underlay_active = allow_native_transition_underlay
                && (left_panel_transitioning
                    || agent_panel_transitioning
                    || input_bar_transitioning
                    || tab_strip_transitioning
                    || self.agent_panel_drag.is_some()
                    || self.sidebar_drag.is_some()
                    || pane_dragging
                    || guard_active);
            self.sync_chrome_transition_underlay(underlay_active, cx);
        }

        let window_width = window.bounds().size.width.as_f32();
        let effective_agent_panel_width = self
            .agent_panel_width
            .min(max_agent_panel_width(window_width));
        #[cfg(not(target_os = "macos"))]
        let animated_panel_width = effective_agent_panel_width * agent_panel_progress;
        let agent_panel_reserved_for_layout = self.agent_panel_open || agent_panel_progress > 0.0;
        let agent_panel_outer_width = (effective_agent_panel_width + 1.0) * agent_panel_progress;
        let sidebar_max_width = max_sidebar_panel_width(window_width, agent_panel_outer_width);
        self.sidebar.update(cx, |sidebar, _| {
            sidebar.set_effective_panel_max_width(sidebar_max_width);
        });
        let vertical_tabs_enabled = self.vertical_tabs_enabled();
        let show_left_panel = self.left_panel_open || left_panel_progress > 0.0;
        let show_vertical_tabs = show_left_panel && vertical_tabs_enabled;
        let show_sidebar_tools =
            show_left_panel && (!vertical_tabs_enabled || self.sidebar_tools_open);
        #[cfg(not(target_os = "macos"))]
        {
            let activity_slot = self.activity_slot;
            self.sidebar.update(cx, |sidebar, _cx| {
                sidebar.set_tools_panel_open(
                    show_left_panel && vertical_tabs_enabled && show_sidebar_tools,
                    activity_slot,
                );
            });
        }

        let theme = cx.theme();
        let ui_surface_opacity = self.ui_surface_opacity();
        let elevated_ui_surface_opacity = self.elevated_ui_surface_opacity();
        let terminal_background = self.terminal_theme.background;
        let terminal_surface_color: Hsla = Rgba {
            r: f32::from(terminal_background.r) / 255.0,
            g: f32::from(terminal_background.g) / 255.0,
            b: f32::from(terminal_background.b) / 255.0,
            // Seam covers do not get Ghostty's native blur/compositing.
            // Keep them opaque and tiny; a translucent GPUI seam is the
            // exact path that lets bright desktop/window backdrops leak
            // through during fast macOS chrome motion.
            a: 1.0,
        }
        .into();
        let chrome_transition_seam_color = terminal_surface_color;
        let chrome_static_seam_color =
            terminal_separator_over_backdrop(terminal_surface_color, theme);
        #[cfg(not(target_os = "macos"))]
        let top_bar_surface_color = theme.title_bar.opacity(ui_surface_opacity);
        #[cfg(target_os = "macos")]
        let top_bar_surface_color = theme
            .title_bar
            .blend(theme.foreground.opacity(0.045 * canvas_depth))
            .opacity(ui_surface_opacity);
        #[cfg(not(target_os = "macos"))]
        let input_bar_surface_color = top_bar_surface_color;
        let pane_divider_color = if cfg!(target_os = "macos") {
            top_bar_surface_color
        } else {
            terminal_separator_over_backdrop(terminal_surface_color, theme)
        };
        // Auxiliary chrome shares the titlebar's material. These quiet inner
        // edges are decoration, not the native transition/drag seam covers.
        // A slight material step makes the panel's header readable even
        // when the compact titlebar and desktop backdrop are both light.
        #[cfg(not(target_os = "macos"))]
        let agent_panel_surface_color =
            top_bar_surface_color.blend(theme.foreground.opacity(if theme.is_dark() {
                0.025
            } else {
                0.018
            }));
        let chrome_edge_color = theme.foreground.opacity(if cfg!(target_os = "macos") {
            0.0
        } else if theme.is_dark() {
            0.055
        } else {
            0.045
        });
        let (tab_sidebar_width, sidebar_content_width) = if show_left_panel {
            let sidebar = self.sidebar.read(cx);
            let tab_sidebar_width = if vertical_tabs_enabled {
                sidebar.rendered_width()
            } else {
                0.0
            };
            let sidebar_content_width = if show_sidebar_tools {
                if vertical_tabs_enabled {
                    (sidebar.effective_panel_width() - tab_sidebar_width).max(0.0)
                } else {
                    sidebar.effective_panel_width()
                }
            } else {
                0.0
            };
            (tab_sidebar_width, sidebar_content_width)
        } else {
            (0.0, 0.0)
        };
        let full_left_panel_width = if vertical_tabs_enabled {
            tab_sidebar_width + sidebar_content_width
        } else {
            sidebar_content_width
        };
        let left_panel_width = full_left_panel_width * left_panel_progress;
        let terminal_content_left = left_panel_width;
        let terminal_content_width =
            (window_width - terminal_content_left - agent_panel_outer_width).max(0.0);
        #[cfg(target_os = "macos")]
        let (terminal_content_left, terminal_content_width) = {
            let (inset, width) = frame::input_island_geometry(terminal_content_width, canvas_depth);
            (terminal_content_left + inset, width)
        };
        let pane_tree_rendered = {
            let workspace = cx.weak_entity();
            let begin_drag_cb =
                move |split_id: usize, start_pos: f32, _window: &mut Window, cx: &mut App| {
                    if let Some(workspace) = workspace.upgrade() {
                        workspace.update(cx, |workspace, cx| {
                            if workspace.active_tab >= workspace.tabs.len() {
                                return;
                            }
                            let tab_idx = workspace.active_tab;
                            let direction =
                                workspace.tabs[tab_idx].pane_tree.split_direction(split_id);
                            let offset = match direction {
                                Some(SplitDirection::Horizontal) => {
                                    workspace.leading_panel_width_for_pane_resize(cx)
                                }
                                _ => 0.0,
                            };
                            #[cfg(target_os = "macos")]
                            let offset = workspace
                                .pane_content_bounds
                                .lock()
                                .ok()
                                .and_then(|bounds| *bounds)
                                .map_or(offset, |bounds| match direction {
                                    Some(SplitDirection::Horizontal) => bounds.left().as_f32(),
                                    _ => bounds.top().as_f32(),
                                });
                            let start_pos = start_pos - offset;
                            workspace.release_active_terminal_mouse_selection(cx);
                            workspace.tabs[tab_idx]
                                .pane_tree
                                .begin_drag(split_id, start_pos);
                            cx.notify();
                        });
                    }
                };
            let workspace = cx.weak_entity();
            let focus_surface_cb = move |surface_id: usize, window: &mut Window, cx: &mut App| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.focus_surface_in_active_tab(surface_id, window, cx);
                    });
                }
            };
            let workspace = cx.weak_entity();
            let focus_pane_cb = move |pane_id: usize, window: &mut Window, cx: &mut App| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.focus_pane_in_active_tab(pane_id, window, cx);
                    });
                }
            };
            let workspace = cx.weak_entity();
            let rename_surface_cb = move |surface_id: usize, window: &mut Window, cx: &mut App| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.begin_surface_rename(surface_id, window, cx);
                    });
                }
            };
            let workspace = cx.weak_entity();
            let close_surface_cb = move |surface_id: usize, window: &mut Window, cx: &mut App| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.close_surface_by_id_in_active_tab(surface_id, window, cx);
                    });
                }
            };
            let workspace = cx.weak_entity();
            let close_pane_cb = move |pane_id: usize, window: &mut Window, cx: &mut App| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        let tab_idx = workspace.active_tab;
                        let _ = workspace.close_pane_in_tab(tab_idx, pane_id, window, cx);
                    });
                }
            };
            let workspace = cx.weak_entity();
            let toggle_zoom_cb = move |pane_id: usize, window: &mut Window, cx: &mut App| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.toggle_pane_zoom_for_pane(pane_id, window, cx);
                    });
                }
            };
            let session_id = self.tabs[self.active_tab].summary_id;
            let tab_accent_color = self.tabs[self.active_tab].color;
            self.tabs[self.active_tab].pane_tree.render(
                session_id,
                begin_drag_cb,
                focus_surface_cb,
                focus_pane_cb,
                rename_surface_cb,
                close_surface_cb,
                close_pane_cb,
                toggle_zoom_cb,
                self.surface_rename.clone(),
                pane_divider_color,
                chrome_transition_seam_color,
                tab_accent_color,
                self.tab_accent_inactive_alpha,
                self.hide_pane_title_bar,
                {
                    #[cfg(target_os = "macos")]
                    {
                        canvas_depth
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        0.0
                    }
                },
                cx,
            )
        };

        let pane_content_bounds = self.pane_content_bounds.clone();
        #[cfg(target_os = "macos")]
        let native_tab_handoff = self.native_tab_handoff_pending.then(|| cx.weak_entity());
        let mut pane_content = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .child(pane_tree_rendered)
            .on_children_prepainted(move |bounds_list, _window, _cx| {
                let Some(bounds) = bounds_list.first().copied() else {
                    return;
                };
                if let Ok(mut guard) = pane_content_bounds.lock() {
                    *guard = Some(bounds);
                }
                #[cfg(target_os = "macos")]
                if let Some(workspace) = &native_tab_handoff {
                    _window.request_native_surface_presentation_sync();
                    let _ = workspace.update(_cx, |workspace, cx| {
                        workspace.commit_native_tab_handoff(cx);
                    });
                }
            })
            .on_drag_move::<DraggedTab>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                    if event.drag(cx).origin != DraggedTabOrigin::Pane {
                        return;
                    }
                    let Some(pane_id) = event.drag(cx).pane_id else {
                        return;
                    };
                    let content_bounds = this
                        .pane_content_bounds
                        .lock()
                        .ok()
                        .and_then(|guard| *guard);
                    let Some(content_bounds) = content_bounds else {
                        return;
                    };
                    let panes = this.tabs[this.active_tab]
                        .pane_tree
                        .pane_bounds(content_bounds);
                    let candidate =
                        pane_split_drop_target_from_position(pane_id, event.event.position, &panes);
                    let pane_tree = &this.tabs[this.active_tab].pane_tree;
                    let new_target = candidate
                        .filter(|t| {
                            !pane_tree.is_noop_pane_move(
                                pane_id,
                                t.target_pane_id,
                                t.direction,
                                t.placement,
                            )
                        })
                        .map(PaneDropTarget::Split);
                    // Store in pane_title_drag so the split preview overlay renders.
                    this.update_pane_title_drag_state(
                        pane_id,
                        event.event.position,
                        new_target,
                        cx,
                    );
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _window, cx| {
                if dragged.origin != DraggedTabOrigin::Pane {
                    return;
                }
                let Some(pane_id) = dragged.pane_id else {
                    return;
                };
                if let Some(drag) = this.pane_title_drag.take()
                    && let Some(PaneDropTarget::Split(target)) = drag.target
                {
                    this.tab_strip_drop_slot = None;
                    let moved = this.tabs[this.active_tab].pane_tree.move_pane(
                        pane_id,
                        target.target_pane_id,
                        target.direction,
                        target.placement,
                    );
                    #[cfg(target_os = "macos")]
                    if moved {
                        this.mark_active_tab_terminal_native_layout_pending(cx);
                    }
                    #[cfg(not(target_os = "macos"))]
                    let _ = moved;
                    this.sync_sidebar(cx);
                    cx.notify();
                }
                // No split target — drop on pane content without a target does nothing.
                // (Tab-strip drops are handled by the tab strip's own on_drop.)
            }));

        if let Some(drag) = self.pane_title_drag.as_ref().filter(|drag| drag.active)
            && let Some(PaneDropTarget::Split(target)) = drag.target
        {
            let content_bounds = self
                .pane_content_bounds
                .lock()
                .ok()
                .and_then(|guard| *guard);
            if let Some(content_bounds) = content_bounds {
                let regions =
                    split_preview_regions(target.bounds, target.direction, target.placement);
                let (existing_left, existing_top, existing_width, existing_height) =
                    split_preview_local_rect(regions.existing, content_bounds);
                let (incoming_left, incoming_top, incoming_width, incoming_height) =
                    split_preview_local_rect(regions.incoming, content_bounds);
                let (seam_left, seam_top, seam_width, seam_height) =
                    split_preview_local_rect(regions.seam, content_bounds);
                #[cfg(target_os = "macos")]
                let preview_inset = frame::PANE_INSET;
                #[cfg(not(target_os = "macos"))]
                let preview_inset = 0.0;
                #[cfg(target_os = "macos")]
                let preview_radius = frame::PANE_RADIUS;
                #[cfg(not(target_os = "macos"))]
                let preview_radius = 0.0;
                pane_content = pane_content
                    .child(
                        div()
                            .absolute()
                            .left(px(existing_left + preview_inset))
                            .top(px(existing_top + preview_inset))
                            .w(px((existing_width - 2.0 * preview_inset).max(0.0)))
                            .h(px((existing_height - 2.0 * preview_inset).max(0.0)))
                            .rounded(px(preview_radius))
                            .bg(theme.background.opacity(0.18)),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(incoming_left + preview_inset))
                            .top(px(incoming_top + preview_inset))
                            .w(px((incoming_width - 2.0 * preview_inset).max(0.0)))
                            .h(px((incoming_height - 2.0 * preview_inset).max(0.0)))
                            .rounded(px(preview_radius))
                            .flex()
                            .items_center()
                            .justify_center()
                            .px(px(12.0))
                            .bg(theme.primary.opacity(if cfg!(target_os = "macos") {
                                0.12
                            } else {
                                0.22
                            }))
                            .font_family(theme.font_family.clone())
                            .text_size(px(12.0))
                            .text_color(theme.foreground.opacity(0.78))
                            .child(div().truncate().child(drag.title.clone())),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(seam_left))
                            .top(px(seam_top))
                            .w(px(seam_width.max(0.0)))
                            .h(px(seam_height.max(0.0)))
                            .bg(theme.primary.opacity(if cfg!(target_os = "macos") {
                                0.0
                            } else {
                                0.62
                            })),
                    );
            }
        }

        let mut terminal_area = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .map(|area| {
                #[cfg(target_os = "macos")]
                let area = area
                    .relative()
                    .m(px(frame::CANVAS_INSET * canvas_depth))
                    .mt(px(frame::canvas_top_inset(
                        canvas_depth,
                        tab_strip_progress,
                        caption.header,
                    )));
                area
            })
            .child(pane_content);

        #[cfg(not(target_os = "macos"))]
        if input_bar_transitioning && input_bar_progress > 0.01 {
            terminal_area = terminal_area.child(
                div()
                    .h(px(CHROME_TRANSITION_SEAM_COVER))
                    .flex_shrink_0()
                    .bg(chrome_transition_seam_color),
            );
        }

        let input_bar_reserved_for_layout = self.input_bar_visible || input_bar_progress > 0.0;
        let full_input_bar_height = if input_bar_reserved_for_layout {
            self.input_bar.read(cx).rendered_height(cx).as_f32()
        } else {
            0.0
        };
        #[cfg(target_os = "macos")]
        let input_popup_progress = input_bar_progress;
        #[cfg(not(target_os = "macos"))]
        let input_popup_progress = input_bar_progress;

        if input_bar_reserved_for_layout {
            #[cfg(not(target_os = "macos"))]
            {
                let input_bar_height = full_input_bar_height * input_bar_progress;
                terminal_area = terminal_area.child(
                    div()
                        .relative()
                        .overflow_hidden()
                        .h(px(input_bar_height))
                        .flex_shrink_0()
                        .bg(input_bar_surface_color)
                        .pt(px(1.0))
                        // Separate output from input without adding another base fill.
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .w_full()
                                .h(px(1.0))
                                .bg(chrome_edge_color),
                        )
                        .child(
                            div().h(px((full_input_bar_height - 1.0).max(0.0))).child(
                                self.input_bar
                                    .clone()
                                    .cached(
                                        StyleRefinement::default()
                                            .w_full()
                                            .h(px(full_input_bar_height - 1.0)),
                                    )
                                    .into_any_element(),
                            ),
                        ),
                );
            }
            #[cfg(target_os = "macos")]
            {
                let width = (window_width - left_panel_width - agent_panel_outer_width).max(0.0);
                let island = terminal_surface_color.opacity(self.terminal_opacity);
                // Keep moving content uncached while the full-size tray slides
                // through its changing reveal mask. Cache the settled endpoint.
                let input = if input_bar_transitioning {
                    self.input_bar.clone().into_any_element()
                } else {
                    self.input_bar
                        .clone()
                        .cached(
                            StyleRefinement::default()
                                .w_full()
                                .h(px(full_input_bar_height)),
                        )
                        .into_any_element()
                };
                // Reveal and terminal layout share one progress sample. Keep
                // content full-sized and clip it rather than scaling typography.
                let tray = frame::InputTray::default();
                terminal_area = terminal_area
                    .child(
                        div()
                            .relative()
                            .h(px(frame::input_reserved_height(
                                full_input_bar_height,
                                self.input_bar_visible,
                                input_bar_progress,
                                canvas_depth,
                            )))
                            .flex_shrink_0()
                            .child(tray.surround(top_bar_surface_color)),
                    )
                    .child(
                        tray.track_reveal(
                            frame::input_reveal_clip(
                                full_input_bar_height,
                                input_bar_progress,
                                canvas_depth,
                            )
                            .child(
                                tray.track_card(
                                    frame::input_card(
                                        width,
                                        full_input_bar_height,
                                        canvas_depth,
                                        island,
                                        theme.foreground,
                                        theme.is_dark(),
                                        top_bar_surface_color,
                                    )
                                    .child(input),
                                ),
                            ),
                        ),
                    );
            }
        }

        #[cfg(target_os = "macos")]
        let terminal_area = {
            let bleed = frame::canvas_top_bleed(canvas_depth, tab_strip_progress, caption.header);
            frame::canvas_viewport().mt(px(-bleed)).pt(px(bleed)).child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.canvas_surround.render(
                        top_bar_surface_color,
                        canvas_depth,
                        tab_strip_progress,
                        caption.header,
                        0.0,
                        terminal_surface_color.opacity(self.terminal_opacity),
                    ))
                    .child(terminal_area),
            )
        };

        let mut main_area = div().relative().flex().flex_1().min_h_0();

        if show_left_panel {
            let mut left_sidebar = div()
                .w(px(full_left_panel_width))
                .h_full()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .overflow_hidden()
                .occlude()
                .bg(top_bar_surface_color)
                .map(|sidebar| {
                    #[cfg(target_os = "macos")]
                    let sidebar = sidebar.pt(px(TOP_BAR_COMPACT_HEIGHT * (1.0 - caption.header)));
                    sidebar
                });
            if show_vertical_tabs {
                left_sidebar = left_sidebar.child(
                    div()
                        .w(px(tab_sidebar_width))
                        .h_full()
                        .flex_shrink_0()
                        .child(self.sidebar.clone()),
                );
            }
            if show_sidebar_tools {
                let sidebar_content: AnyElement = match self.activity_slot {
                    ActivitySlot::Files => self.file_tree_view.clone().into_any_element(),
                    ActivitySlot::Search => self.search_view.clone().into_any_element(),
                };
                let mut sidebar_panel = div()
                    .w(px(sidebar_content_width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .overflow_hidden()
                    .occlude();
                if !vertical_tabs_enabled {
                    sidebar_panel = sidebar_panel.child(self.activity_bar.clone());
                }
                left_sidebar = left_sidebar.child(sidebar_panel.child(sidebar_content));
            }
            main_area = main_area.child(
                div()
                    .w(px(left_panel_width))
                    .min_w_0()
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .overflow_hidden()
                    .child(left_sidebar),
            );
        }
        // ── Main column: terminal area only (editor panes live inside pane tree) ──
        let main_column = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(terminal_area);

        main_area = main_area.child(main_column);

        let sidebar_resizable = show_left_panel
            && !left_panel_transitioning
            && (show_sidebar_tools || self.sidebar.read(cx).has_resizable_content());
        if sidebar_resizable {
            // On macOS the terminal starts one card gutter beyond the sidebar.
            // Center the grip in that gutter instead of on the painted card.
            #[cfg(target_os = "macos")]
            let center = left_panel_width + frame::CARD_OUTER_INSET * 0.5 * canvas_depth;
            #[cfg(not(target_os = "macos"))]
            let center = left_panel_width;
            main_area = main_area.child(
                column_resize_handle(
                    "left-sidebar-resize-handle",
                    center,
                    self.sidebar_drag.is_some(),
                    theme.foreground,
                    theme.is_dark(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                        this.release_active_terminal_mouse_selection(cx);
                        // Begin at the visible width after any window-size clamp.
                        let width = this.leading_panel_width_for_pane_resize(cx);
                        this.sidebar_drag = Some((f32::from(event.position.x), width));
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
            );
        } else if show_left_panel {
            main_area = main_area.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px((left_panel_width - 1.0).max(0.0)))
                    .w(px(1.0))
                    .bg(chrome_edge_color),
            );
        }

        let render_agent_panel = agent_panel_reserved_for_layout;

        if render_agent_panel {
            let panel_width = agent_panel_outer_width;

            main_area = main_area.child(
                div()
                    .relative()
                    .w(px(panel_width))
                    .min_w_0()
                    .overflow_hidden()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .map(|panel| {
                        #[cfg(target_os = "macos")]
                        {
                            let split = self.has_active_tab()
                                && self.tabs[self.active_tab].pane_tree.pane_count() > 1;
                            let (top, bottom) = frame::agent_vertical_insets(
                                canvas_depth,
                                split,
                                frame::input_reserved_height(
                                    full_input_bar_height,
                                    self.input_bar_visible,
                                    input_bar_progress,
                                    canvas_depth,
                                ),
                                tab_strip_progress,
                                caption.header,
                            );
                            panel
                                .mt(px(top.min(0.0)))
                                .pt(px(top.max(0.0)))
                                .pb(px(bottom))
                                .bg(top_bar_surface_color)
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            panel.h_full().bg(agent_panel_surface_color)
                        }
                    })
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .h(px(1.0))
                            .bg(chrome_edge_color),
                    )
                    .when(!cfg!(target_os = "macos"), |panel| {
                        panel.child(
                            div()
                                .w(px(1.0))
                                .h_full()
                                .flex_shrink_0()
                                .bg(chrome_edge_color),
                        )
                    })
                    .child(
                        div()
                            .w(px(effective_agent_panel_width
                                + if cfg!(target_os = "macos") { 1.0 } else { 0.0 }))
                            .flex_shrink_0()
                            .min_w_0()
                            .h_full()
                            .map(|panel| {
                                #[cfg(target_os = "macos")]
                                {
                                    panel.relative().overflow_hidden()
                                }
                                #[cfg(not(target_os = "macos"))]
                                {
                                    panel
                                }
                            })
                            .child(
                                self.agent_panel
                                    .clone()
                                    .cached(StyleRefinement::default().size_full())
                                    .into_any_element(),
                            ),
                    ),
            );
        }

        if render_agent_panel && !agent_panel_transitioning {
            let seam = window_width - agent_panel_outer_width;
            // Terminal ends 5pt before the layout seam; agent content begins
            // 2pt after it. Both edges share this 7pt gutter.
            #[cfg(target_os = "macos")]
            let center =
                seam + (frame::AGENT_CONTENT_INSET - frame::CARD_OUTER_INSET) * 0.5 * canvas_depth;
            #[cfg(not(target_os = "macos"))]
            let center = seam;
            main_area = main_area.child(
                column_resize_handle(
                    "agent-panel-divider",
                    center,
                    self.agent_panel_drag.is_some(),
                    theme.foreground,
                    theme.is_dark(),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        this.release_active_terminal_mouse_selection(cx);
                        this.agent_panel_drag =
                            Some((f32::from(event.position.x), effective_agent_panel_width));
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ),
            );
        }

        // Top bar — compact titlebar for one tab, full strip for many
        let top_bar_height = self.current_top_bar_height();

        let top_bar = self.render_top_bar(
            window,
            cx,
            if cfg!(target_os = "macos") {
                TOP_BAR_COMPACT_HEIGHT
            } else {
                top_bar_height
            },
            tab_strip_progress,
            top_bar_surface_color,
        );
        let show_compact_top_bar_separator =
            cfg!(not(target_os = "macos")) && top_bar_height > 0.5 && tab_strip_progress <= 0.01;

        // Owned clone: `theme` outlives the sidebar overlay update below,
        // which mutably borrows `cx` (the `&Theme` borrow would conflict).
        let theme = cx.theme().clone();
        let mut root = workspace_focus_container(&self.workspace_focus)
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.transparent)
            .font_family(theme.mono_font_family.clone());

        root = root
            .key_context("ConWorkspace")
            .on_drag_move::<DraggedTab>(cx.listener(
                |this, event: &gpui::DragMoveEvent<DraggedTab>, win, cx| {
                    if event.drag(cx).origin != DraggedTabOrigin::HorizontalTabStrip {
                        return;
                    }
                    if let Some(preview) = this
                        .tab_drag_preview
                        .lock()
                        .ok()
                        .and_then(|guard| guard.clone())
                    {
                        let preview_size = preview.size;
                        let leading_pad = top_bar::top_bar_leading_pad(win);
                        let min_left = px(leading_pad);
                        let max_left =
                            (win.viewport_size().width - preview_size.width).max(min_left);
                        let probe = tab_drag_overlay_probe_position(
                            event.event.position,
                            &preview,
                            preview_size,
                            min_left,
                            max_left,
                        );
                        let tab_bounds = this
                            .tab_strip_tab_bounds
                            .lock()
                            .ok()
                            .map(|g| g.clone())
                            .unwrap_or_default();
                        let dragged_id = this
                            .active_dragged_tab_session_id
                            .lock()
                            .ok()
                            .and_then(|id| *id);
                        if let Some(slot) = this
                            .tabs
                            .iter()
                            .position(|tab| Some(tab.summary_id) == dragged_id)
                            .and_then(|source| {
                                horizontal_tab_slot_from_bounds(
                                    probe,
                                    &tab_bounds,
                                    this.tabs.len(),
                                    source,
                                    preview.source_visual_index,
                                )
                            })
                        {
                            let new_target = Some(TabDragTarget::Reorder { slot });
                            if this.tab_strip_drop_slot != Some(slot)
                                || this.tab_drag_target != new_target
                            {
                                this.tab_strip_drop_slot = Some(slot);
                                this.tab_drag_target = new_target;
                                cx.notify();
                            }
                        }
                    }
                },
            ))
            // Pane drag-to-resize: capture mouse move/up on root so it works
            // even when cursor is over terminal views (which capture mouse events).
            .on_mouse_move({
                cx.listener(move |this, event: &MouseMoveEvent, win, cx| {
                    if this.handle_active_resize_mouse_move(event, win, cx) {
                        return;
                    }

                    if this.active_tab >= this.tabs.len() {
                        return;
                    }

                    let top_bar_height = this.current_top_bar_height();
                    let input_bar_height = this.input_bar.read(cx).rendered_height(cx).as_f32()
                        * this.input_bar_motion.current();

                    // Compute layout-dependent inputs *before* re-borrowing
                    // `this` mutably for the pane tree, otherwise we
                    // collide with the immutable borrow needed by
                    // `left_panel_open` state.
                    let win_w = f32::from(win.bounds().size.width);
                    let win_h = f32::from(win.bounds().size.height);
                    let effective_agent_panel_width =
                        this.agent_panel_width.min(max_agent_panel_width(win_w));
                    let agent_panel_drag_width = if this.agent_panel_open {
                        effective_agent_panel_width + 7.0
                    } else {
                        0.0
                    };
                    let leading_panel_w = this.leading_panel_width_for_pane_resize(cx);

                    let pane_tree = &mut this.tabs[this.active_tab].pane_tree;

                    if !pane_tree.is_dragging() {
                        return;
                    }

                    // Estimate terminal area from window bounds minus fixed chrome
                    // (tab bar ~38px, input bar ~40px, agent panel if open,
                    // file tree panel on the leading edge if enabled).
                    let (current_pos, total_size) =
                        if let Some(dir) = pane_tree.dragging_direction() {
                            match dir {
                                SplitDirection::Horizontal => (
                                    f32::from(event.position.x) - leading_panel_w,
                                    win_w - agent_panel_drag_width - leading_panel_w,
                                ),
                                SplitDirection::Vertical => (
                                    f32::from(event.position.y),
                                    win_h - top_bar_height - input_bar_height,
                                ),
                            }
                        } else {
                            return;
                        };

                    if pane_tree.update_drag(current_pos, total_size) {
                        cx.notify();
                    }
                })
            })
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.finish_active_resize_drag(cx) {
                        return;
                    }
                    // Clear any stale pane_title_drag state (GPUI drag handles
                    // completion via on_drop; this is just a safety cleanup).
                    if this.pane_title_drag.is_some() {
                        clear_pane_tab_promotion_drag_state(
                            &mut this.pane_title_drag,
                            &mut this.tab_strip_drop_slot,
                            &mut this.tab_drag_target,
                        );
                        cx.notify();
                    }
                    if this.active_tab >= this.tabs.len() {}
                }),
            )
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::minimize))
            .on_action(cx.listener(Self::toggle_agent_panel))
            .on_action(cx.listener(Self::toggle_input_bar))
            .on_action(cx.listener(Self::collapse_sidebar))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::toggle_command_palette))
            .on_action(cx.listener(Self::new_window))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::select_tab_1))
            .on_action(cx.listener(Self::select_tab_2))
            .on_action(cx.listener(Self::select_tab_3))
            .on_action(cx.listener(Self::select_tab_4))
            .on_action(cx.listener(Self::select_tab_5))
            .on_action(cx.listener(Self::select_tab_6))
            .on_action(cx.listener(Self::select_tab_7))
            .on_action(cx.listener(Self::select_tab_8))
            .on_action(cx.listener(Self::select_tab_9))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::close_pane))
            .on_action(cx.listener(Self::toggle_pane_zoom))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::split_left))
            .on_action(cx.listener(Self::split_up))
            .on_action(cx.listener(Self::clear_terminal))
            .on_action(cx.listener(Self::find_in_terminal))
            .on_action(cx.listener(Self::clear_restored_terminal_history_action))
            .on_action(cx.listener(Self::export_workspace_layout))
            .on_action(cx.listener(Self::add_workspace_layout_tabs))
            .on_action(cx.listener(Self::open_workspace_layout_window))
            .on_action(cx.listener(Self::new_surface))
            .on_action(cx.listener(Self::new_surface_split_right))
            .on_action(cx.listener(Self::new_surface_split_down))
            .on_action(cx.listener(Self::next_surface))
            .on_action(cx.listener(Self::previous_surface))
            .on_action(cx.listener(Self::rename_current_surface))
            .on_action(cx.listener(Self::close_surface))
            .on_action(cx.listener(Self::focus_input))
            .on_action(cx.listener(Self::ask_ai))
            .on_action(cx.listener(Self::open_agent_handoff))
            .on_action(cx.listener(Self::editor_move_left))
            .on_action(cx.listener(Self::editor_move_right))
            .on_action(cx.listener(Self::editor_move_up))
            .on_action(cx.listener(Self::editor_move_down))
            .on_action(cx.listener(Self::editor_move_home))
            .on_action(cx.listener(Self::editor_move_end))
            .on_action(cx.listener(Self::editor_move_line_start))
            .on_action(cx.listener(Self::editor_move_line_end))
            .on_action(cx.listener(Self::editor_select_left))
            .on_action(cx.listener(Self::editor_select_right))
            .on_action(cx.listener(Self::editor_select_up))
            .on_action(cx.listener(Self::editor_select_down))
            .on_action(cx.listener(Self::editor_select_home))
            .on_action(cx.listener(Self::editor_select_end))
            .on_action(cx.listener(Self::editor_delete_backward))
            .on_action(cx.listener(Self::editor_delete_forward))
            .on_action(cx.listener(Self::editor_insert_newline))
            .on_action(cx.listener(Self::editor_save))
            .on_action(cx.listener(Self::editor_toggle_preview))
            .on_action(cx.listener(Self::editor_undo))
            .on_action(cx.listener(Self::editor_copy))
            .on_action(cx.listener(Self::editor_cut))
            .on_action(cx.listener(Self::editor_paste))
            .on_action(cx.listener(Self::editor_select_all))
            .on_action(cx.listener(Self::cycle_input_mode))
            .on_action(cx.listener(Self::toggle_pane_scope_picker))
            .on_action(cx.listener(Self::toggle_left_panel))
            .on_action(cx.listener(Self::focus_files_panel))
            .on_action(cx.listener(Self::search_files_panel))
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.editor_has_keyboard_focus(window, cx)
                    && this.handle_editor_text_key_down(event, window, cx)
                {
                    return;
                }
                if focused_editor_pane_key_fallback_allowed(
                    this.is_input_surface_focused(window, cx),
                ) && this.handle_focused_editor_pane_key_down(event, window, cx)
                {
                    return;
                }

                if !this.pane_scope_picker_open {
                    return;
                }

                let mods = &event.keystroke.modifiers;
                let key = event.keystroke.key.as_str();
                let local_picker_key = !mods.control && !mods.alt && !mods.platform;
                let unshifted_local_picker_key = local_picker_key && !mods.shift;

                let mut handled = false;
                if key == "escape" {
                    this.close_pane_scope_picker(cx);
                    handled = true;
                } else if local_picker_key && key.eq_ignore_ascii_case("a") {
                    this.set_scope_broadcast(window, cx);
                    handled = true;
                } else if local_picker_key && key.eq_ignore_ascii_case("f") {
                    this.set_scope_focused(window, cx);
                    handled = true;
                } else if unshifted_local_picker_key
                    && let Some(digit) = key.chars().next().and_then(|c| c.to_digit(10))
                {
                    let pane_index = if digit == 0 { 9 } else { (digit - 1) as usize };
                    this.toggle_scope_pane_by_index(pane_index, window, cx);
                    handled = true;
                }

                if handled {
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                let rename_focused = this
                    .surface_rename
                    .as_ref()
                    .is_some_and(|editor| editor.input.focus_handle(cx).is_focused(window));
                if rename_focused {
                    this.surface_rename = None;
                    this.refocus_active_terminal(window, cx);
                    cx.notify();
                } else {
                    cx.propagate();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                // Don't handle workspace shortcuts when a modal overlay is open
                if this.settings_panel.read(cx).is_overlay_visible()
                    || this.command_palette.read(cx).is_visible()
                {
                    return;
                }

                let mods = &event.keystroke.modifiers;
                let key = event.keystroke.key.as_str();

                if key == "escape" && this.surface_rename.take().is_some() {
                    this.refocus_active_terminal(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                #[cfg(target_os = "macos")]
                if mods.platform
                    && !mods.control
                    && !mods.alt
                    && matches!(key, "`" | "~" | ">" | "<")
                {
                    if mods.shift || matches!(key, "~" | "<") {
                        cx.dispatch_action(&crate::PreviousWindow);
                    } else {
                        cx.dispatch_action(&crate::NextWindow);
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }

                // Browser-style fallbacks. The configurable actions also bind
                // Control-Tab / Control-Shift-Tab by default.
                if mods.platform && mods.shift && key == "[" {
                    this.previous_tab(&PreviousTab, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    return;
                }

                if mods.platform && mods.shift && key == "]" {
                    this.next_tab(&NextTab, window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                }
            }));
        #[cfg(not(target_os = "macos"))]
        {
            root = root.child(top_bar);
        }
        #[cfg(target_os = "macos")]
        {
            root = root.child(
                div()
                    .h(px(top_bar_height))
                    .flex_shrink_0()
                    .bg(top_bar_surface_color),
            );
        }
        if show_compact_top_bar_separator {
            root = root.child(
                div()
                    .h(px(1.0))
                    .flex_shrink_0()
                    .bg(chrome_static_seam_color),
            );
        }
        root = root.child(main_area);
        #[cfg(target_os = "macos")]
        {
            let caption_width = frame::caption_width(
                window_width,
                left_panel_width,
                caption.dock,
                top_bar::top_bar_leading_pad(window),
            );
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w(px(caption_width))
                    .h(px(TOP_BAR_COMPACT_HEIGHT))
                    .child(top_bar),
            );
        }

        // Vertical-tab rail hover card — composed here as a root overlay
        // (window coordinates) so it paints above the terminal and is not
        // clipped by the sidebar's overflow-hidden container. Only composed
        // while the rail is actually in the layout; when it is not, clear
        // stale hover state (an unmounted rail cannot deliver hover-leave).
        if !show_vertical_tabs {
            self.sidebar
                .update(cx, |sidebar, _cx| sidebar.clear_hovered_rail());
        }
        if show_vertical_tabs
            && let Some(hover_card) = self.sidebar.update(cx, |sidebar, cx| {
                sidebar.render_hover_card_overlay(window, cx)
            })
        {
            root = root.child(hover_card);
        }

        if let Some(preview) = self
            .tab_drag_preview
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
        {
            let preview_size = preview.size;
            let leading_pad = top_bar::top_bar_leading_pad(window);
            let min_left = px(leading_pad);
            let max_left = (window.viewport_size().width - preview_size.width).max(min_left);
            let left =
                tab_drag_overlay_origin(window.mouse_position(), &preview, min_left, max_left).x;
            root = root.child(
                top_bar::tab_drag_capsule(&preview, &theme)
                    .absolute()
                    .left(left)
                    .top(preview.source_top)
                    .cursor_grab(),
            );
        }

        if let Some(drag) = self.pane_title_drag.as_ref().filter(|drag| drag.active) {
            let preview_size = Size {
                width: px(160.0),
                height: px(32.0),
            };
            let preview_origin = if let Some(PaneDropTarget::NewTab { .. }) = drag.target {
                tab_drag_preview_origin(
                    drag.current_pos,
                    preview_size,
                    px(TAB_DROP_PREVIEW_TOP),
                    px(TAB_DROP_PREVIEW_HEIGHT),
                )
            } else {
                pane_drag_floating_preview_origin(drag.current_pos, preview_size)
            };
            root = root.child(
                div()
                    .absolute()
                    .left(preview_origin.x)
                    .top(preview_origin.y)
                    .w(preview_size.width)
                    .h(preview_size.height)
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .cursor_grab()
                    .bg(theme
                        .background
                        .blend(
                            theme
                                .foreground
                                .opacity(if theme.is_dark() { 0.10 } else { 0.045 }),
                        )
                        .opacity(1.0))
                    .font_family(theme.font_family.clone())
                    .text_size(px(12.0))
                    .text_color(theme.foreground)
                    .child(crate::assets::tab_identity_icon(
                        crate::assets::tab_identity_asset("phosphor/terminal.svg"),
                        px(14.0),
                        theme.foreground,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(drag.title.clone()),
                    ),
            );
        }

        if tab_strip_transitioning {
            root = root.child(
                div()
                    .absolute()
                    .top(px(top_bar_height))
                    .left_0()
                    .right_0()
                    .h(px(CHROME_TRANSITION_SEAM_COVER))
                    .bg(chrome_transition_seam_color),
            );
        }

        if agent_panel_snap_guard_active && self.agent_panel_open {
            let agent_panel_seam_right = (effective_agent_panel_width
                - (CHROME_MOTION_SEAM_OVERDRAW - CHROME_TRANSITION_SEAM_COVER))
                .max(0.0);
            root = root.child(
                div()
                    .absolute()
                    .top(px(top_bar_height))
                    .bottom_0()
                    .right(px(agent_panel_seam_right))
                    .w(px(CHROME_MOTION_SEAM_OVERDRAW))
                    .bg(chrome_transition_seam_color),
            );
        }

        if agent_panel_release_cover_active {
            root = root.child(
                div()
                    .absolute()
                    .top(px(top_bar_height))
                    .bottom_0()
                    .right_0()
                    .w(px(effective_agent_panel_width + 1.0))
                    .bg(chrome_transition_seam_color),
            );
        }

        #[cfg(not(target_os = "macos"))]
        if agent_panel_transitioning && render_agent_panel {
            root = root.child(
                div()
                    .absolute()
                    .top(px(top_bar_height))
                    .bottom_0()
                    .right(px(animated_panel_width))
                    .w(px(CHROME_TRANSITION_SEAM_COVER))
                    .bg(chrome_transition_seam_color),
            );
        }

        if self.has_active_resize_drag() {
            let resize_direction = if self.sidebar_drag.is_some() || self.agent_panel_drag.is_some()
            {
                SplitDirection::Horizontal
            } else {
                self.tabs
                    .get(self.active_tab)
                    .and_then(|tab| tab.pane_tree.dragging_direction())
                    .unwrap_or(SplitDirection::Horizontal)
            };
            let mut overlay = div()
                .id("resize-capture-overlay")
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .left_0();
            overlay = match resize_direction {
                SplitDirection::Horizontal => overlay.cursor_col_resize(),
                SplitDirection::Vertical => overlay.cursor_row_resize(),
            };
            root = root.child(
                overlay
                    .occlude()
                    .bg(theme.transparent)
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                        this.handle_active_resize_mouse_move(event, window, cx);
                        cx.stop_propagation();
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.finish_active_resize_drag(cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }

        // Workspace-level popups render above embedded terminal surfaces.
        if has_skill_popup
            && let Some(popup) = self.render_skill_popup(
                terminal_content_left,
                terminal_content_width,
                elevated_ui_surface_opacity,
                cx,
            )
        {
            root = root.child(popup);
        }

        if has_path_popup
            && !has_skill_popup
            && let Some(popup) = self.render_path_popup(
                terminal_content_left,
                terminal_content_width,
                elevated_ui_surface_opacity,
                cx,
            )
        {
            root = root.child(popup);
        }

        if let Some(picker) = self.render_pane_scope_picker(
            terminal_content_left,
            terminal_content_width,
            input_popup_progress,
            ui_surface_opacity,
            elevated_ui_surface_opacity,
            window,
            cx,
        ) {
            root = root.child(picker);
        }

        if has_inline_skill_popup
            && let Some(popup) =
                self.render_inline_skill_popup(elevated_ui_surface_opacity, window, cx)
        {
            root = root.child(popup);
        }

        let settings_visible = self.settings_panel.read(cx).is_overlay_visible();
        if settings_visible {
            root = root.child(self.settings_panel.clone());
        }

        let palette_visible = self.command_palette.read(cx).is_visible();
        if palette_visible {
            root = root.child(self.command_palette.clone());
        }

        #[cfg(target_os = "linux")]
        {
            let mut workspace_frame = root;
            let decorations = window.window_decorations();
            let is_maximized = window.is_maximized();
            let is_fullscreen = window.is_fullscreen();
            let mut shape_radii = crate::LinuxWindowShapeRadii::default();

            if let Decorations::Client { tiling } = decorations
                && !is_maximized
                && !is_fullscreen
            {
                let radius_px = (LINUX_WINDOW_CORNER_RADIUS.as_f32()
                    * window.scale_factor().max(f32::EPSILON))
                .round()
                .max(0.0) as u32;
                if !(tiling.top || tiling.left) {
                    workspace_frame = workspace_frame.rounded_tl(LINUX_WINDOW_CORNER_RADIUS);
                    shape_radii.top_left = radius_px;
                }
                if !(tiling.top || tiling.right) {
                    workspace_frame = workspace_frame.rounded_tr(LINUX_WINDOW_CORNER_RADIUS);
                    shape_radii.top_right = radius_px;
                }
                if !(tiling.bottom || tiling.left) {
                    workspace_frame = workspace_frame.rounded_bl(LINUX_WINDOW_CORNER_RADIUS);
                    shape_radii.bottom_left = radius_px;
                }
                if !(tiling.bottom || tiling.right) {
                    workspace_frame = workspace_frame.rounded_br(LINUX_WINDOW_CORNER_RADIUS);
                    shape_radii.bottom_right = radius_px;
                }
                workspace_frame = workspace_frame.overflow_hidden();
            }

            window.set_client_inset(px(0.0));

            let scale = window.scale_factor().max(f32::EPSILON);
            let size = window.bounds().size;
            let width_px = (size.width.as_f32() * scale).round().max(1.0) as u32;
            let height_px = (size.height.as_f32() * scale).round().max(1.0) as u32;
            let shape_signature = (width_px, height_px, shape_radii);
            if self.linux_window_shape_signature != Some(shape_signature) {
                let linux_x11 = std::env::var_os("DISPLAY").is_some()
                    && std::env::var_os("WAYLAND_DISPLAY").is_none();
                if !linux_x11
                    || crate::sync_linux_x11_window_shape(window, width_px, height_px, shape_radii)
                {
                    self.linux_window_shape_signature = Some(shape_signature);
                }
            }

            root = div()
                .relative()
                .size_full()
                .bg(transparent_black())
                .child(workspace_frame);
        }

        root.into_any_element()
    }
}

#[cfg(test)]
mod focus_tests;
