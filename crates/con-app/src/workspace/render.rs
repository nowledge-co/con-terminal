mod popups;
mod top_bar;

use super::*;

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
        if !self.left_panel_open {
            return 0.0;
        }

        let sidebar = self.sidebar.read(cx);
        if self.vertical_tabs_enabled() {
            if self.sidebar_tools_open {
                sidebar.panel_width()
            } else {
                sidebar.rendered_width()
            }
        } else {
            sidebar.panel_width()
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
            let max_width = max_agent_panel_width(win.bounds().size.width.as_f32());
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
        let input_bar_height = if self.input_bar_visible {
            self.input_bar.read(cx).rendered_height(cx).as_f32()
        } else {
            0.0
        };
        let win_w = f32::from(win.bounds().size.width);
        let win_h = f32::from(win.bounds().size.height);
        let effective_agent_panel_width = self.agent_panel_width.min(max_agent_panel_width(win_w));
        let agent_panel_drag_width = if self.agent_panel_open {
            effective_agent_panel_width + 7.0
        } else {
            0.0
        };
        let leading_panel_w = self.leading_panel_width_for_pane_resize(cx);

        let pane_tree = &mut self.tabs[self.active_tab].pane_tree;
        if !pane_tree.is_dragging() {
            return false;
        }

        let (current_pos, total_size) = if let Some(dir) = pane_tree.dragging_direction() {
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

        if self.modal_was_open && !is_modal_open {
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
        self.tabs[self.active_tab].pane_tree.sync_focus(window, cx);
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
            panel.set_model_name(model_name);
            panel.set_session_model_options(available_models, window, cx);
            panel.set_show_inline_input(show_inline);
            panel.set_skills(skill_entries, cx);
            panel.set_recent_inputs(self.recent_input_history(80));
        });

        let agent_panel_progress = self.agent_panel_motion.value(window);
        let input_bar_progress = self.input_bar_motion.value(window);
        let tab_strip_progress = self.tab_strip_motion.value(window);
        let agent_panel_transitioning = self.agent_panel_motion.is_animating();
        let input_bar_transitioning = self.input_bar_motion.is_animating();
        let tab_strip_transitioning = self.tab_strip_motion.is_animating();
        #[cfg(target_os = "macos")]
        let (agent_panel_snap_guard_active, agent_panel_snap_guard_expired) =
            Self::snap_guard_state(&mut self.agent_panel_snap_guard_until, window);
        #[cfg(target_os = "macos")]
        let (input_bar_snap_guard_active, input_bar_snap_guard_expired) =
            Self::snap_guard_state(&mut self.input_bar_snap_guard_until, window);
        #[cfg(target_os = "macos")]
        let (top_chrome_snap_guard_active, top_chrome_snap_guard_expired) =
            Self::snap_guard_state(&mut self.top_chrome_snap_guard_until, window);
        #[cfg(not(target_os = "macos"))]
        let agent_panel_snap_guard_active = false;
        #[cfg(not(target_os = "macos"))]
        let input_bar_snap_guard_active = false;
        #[cfg(not(target_os = "macos"))]
        let top_chrome_snap_guard_active = false;
        #[cfg(target_os = "macos")]
        {
            let release_cover = Duration::from_millis(CHROME_RELEASE_COVER_MS);
            if agent_panel_snap_guard_expired && !self.agent_panel_open {
                Self::extend_guard(&mut self.agent_panel_release_cover_until, release_cover);
            }
            if input_bar_snap_guard_expired && !self.input_bar_visible {
                Self::extend_guard(&mut self.input_bar_release_cover_until, release_cover);
            }
            if top_chrome_snap_guard_expired && !self.horizontal_tabs_visible() {
                Self::extend_guard(&mut self.top_chrome_release_cover_until, release_cover);
            }
        }
        #[cfg(target_os = "macos")]
        let agent_panel_release_cover_active =
            Self::snap_guard_active(&mut self.agent_panel_release_cover_until, window);
        #[cfg(target_os = "macos")]
        let input_bar_release_cover_active =
            Self::snap_guard_active(&mut self.input_bar_release_cover_until, window);
        #[cfg(target_os = "macos")]
        let top_chrome_release_cover_active =
            Self::snap_guard_active(&mut self.top_chrome_release_cover_until, window);
        #[cfg(not(target_os = "macos"))]
        let agent_panel_release_cover_active = false;
        #[cfg(not(target_os = "macos"))]
        let input_bar_release_cover_active = false;
        #[cfg(not(target_os = "macos"))]
        let top_chrome_release_cover_active = false;
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
                && (agent_panel_transitioning
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
        let agent_panel_reserved_for_layout =
            self.agent_panel_open || agent_panel_progress > 0.01 || agent_panel_snap_guard_active;
        #[cfg(target_os = "macos")]
        let agent_panel_outer_width = if agent_panel_reserved_for_layout {
            effective_agent_panel_width + 1.0
        } else {
            0.0
        };
        #[cfg(not(target_os = "macos"))]
        let agent_panel_outer_width = if agent_panel_progress > 0.01 {
            animated_panel_width + 1.0
        } else if agent_panel_reserved_for_layout {
            effective_agent_panel_width + 1.0
        } else {
            0.0
        };
        let vertical_tabs_enabled = self.vertical_tabs_enabled();
        let show_left_panel = self.left_panel_open;
        let show_vertical_tabs = show_left_panel && vertical_tabs_enabled;
        let show_sidebar_tools =
            show_left_panel && (!vertical_tabs_enabled || self.sidebar_tools_open);
        let activity_slot = self.activity_slot;
        self.sidebar.update(cx, |sidebar, _cx| {
            sidebar.set_tools_panel_open(
                show_left_panel && vertical_tabs_enabled && show_sidebar_tools,
                activity_slot,
            );
        });

        let theme = cx.theme();
        let ui_surface_opacity = self.ui_surface_opacity();
        let elevated_ui_surface_opacity = self.elevated_ui_surface_opacity();
        let agent_panel_content_progress = ((agent_panel_progress - 0.16) / 0.84)
            .clamp(0.0, 1.0)
            .powf(0.9);
        let input_bar_content_progress = ((input_bar_progress - 0.08) / 0.92)
            .clamp(0.0, 1.0)
            .powf(0.92);
        let compact_titlebar_progress = 1.0 - tab_strip_progress;
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
        let portable_terminal_backdrop_color =
            terminal_surface_color.opacity(self.terminal_opacity);
        let chrome_transition_seam_color = terminal_surface_color;
        let chrome_static_seam_color =
            terminal_separator_over_backdrop(terminal_surface_color, theme);
        let pane_divider_color = terminal_separator_over_backdrop(terminal_surface_color, theme);
        #[cfg(target_os = "macos")]
        let top_bar_surface_color = theme.title_bar.opacity(ui_surface_opacity);
        #[cfg(not(target_os = "macos"))]
        let top_bar_surface_color = theme.title_bar.opacity(ui_surface_opacity);
        #[cfg(target_os = "macos")]
        let input_bar_surface_color = theme.title_bar.opacity(ui_surface_opacity);
        #[cfg(not(target_os = "macos"))]
        let input_bar_surface_color = theme.title_bar.opacity(ui_surface_opacity);
        #[cfg(target_os = "macos")]
        let elevated_panel_surface_color = theme.background.opacity(elevated_ui_surface_opacity);
        #[cfg(not(target_os = "macos"))]
        let elevated_panel_surface_color = theme.background.opacity(elevated_ui_surface_opacity);
        let sidebar_seam_color =
            theme
                .foreground
                .opacity(if theme.is_dark() { 0.055 } else { 0.045 });
        let (tab_sidebar_width, sidebar_content_width) = if show_left_panel {
            let sidebar = self.sidebar.read(cx);
            let tab_sidebar_width = if vertical_tabs_enabled {
                sidebar.rendered_width()
            } else {
                0.0
            };
            let sidebar_content_width = if show_sidebar_tools {
                if vertical_tabs_enabled {
                    (sidebar.panel_width() - tab_sidebar_width).max(0.0)
                } else {
                    sidebar.panel_width()
                }
            } else {
                0.0
            };
            (tab_sidebar_width, sidebar_content_width)
        } else {
            (0.0, 0.0)
        };
        let left_panel_width = if vertical_tabs_enabled {
            tab_sidebar_width + sidebar_content_width
        } else {
            sidebar_content_width
        };
        let terminal_content_left = left_panel_width;
        let terminal_content_width =
            (window_width - terminal_content_left - agent_panel_outer_width).max(0.0);
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
                            let start_pos =
                                match workspace.tabs[tab_idx].pane_tree.split_direction(split_id) {
                                    Some(SplitDirection::Horizontal) => {
                                        start_pos
                                            - workspace.leading_panel_width_for_pane_resize(cx)
                                    }
                                    Some(SplitDirection::Vertical) | None => start_pos,
                                };
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
                cx,
            )
        };

        let pane_content_bounds = self.pane_content_bounds.clone();
        let mut pane_content = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .when(cfg!(not(target_os = "macos")), |pane_content| {
                pane_content.bg(portable_terminal_backdrop_color)
            })
            .child(pane_tree_rendered)
            .on_children_prepainted(move |bounds_list, _, _| {
                let Some(bounds) = bounds_list.first().copied() else {
                    return;
                };
                if let Ok(mut guard) = pane_content_bounds.lock() {
                    *guard = Some(bounds);
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
                if let Some(drag) = this.pane_title_drag.take() {
                    if let Some(PaneDropTarget::Split(target)) = drag.target {
                        this.tab_strip_drop_slot = None;
                        this.tabs[this.active_tab].pane_tree.move_pane(
                            pane_id,
                            target.target_pane_id,
                            target.direction,
                            target.placement,
                        );
                        cx.notify();
                        return;
                    }
                }
                // No split target — drop on pane content without a target does nothing.
                // (Tab-strip drops are handled by the tab strip's own on_drop.)
            }));

        if let Some(drag) = self.pane_title_drag.as_ref().filter(|drag| drag.active) {
            if let Some(PaneDropTarget::Split(target)) = drag.target {
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
                    pane_content = pane_content
                        .child(
                            div()
                                .absolute()
                                .left(px(existing_left))
                                .top(px(existing_top))
                                .w(px(existing_width.max(0.0)))
                                .h(px(existing_height.max(0.0)))
                                .bg(theme.background.opacity(0.18)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(incoming_left))
                                .top(px(incoming_top))
                                .w(px(incoming_width.max(0.0)))
                                .h(px(incoming_height.max(0.0)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .px(px(12.0))
                                .bg(theme.primary.opacity(0.22))
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
                                .bg(theme.primary.opacity(0.62)),
                        );
                }
            }
        }

        let mut terminal_area = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(cfg!(not(target_os = "macos")), |terminal_area| {
                terminal_area.bg(portable_terminal_backdrop_color)
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

        let input_bar_reserved_for_layout =
            input_bar_progress > 0.01 || input_bar_snap_guard_active;

        if input_bar_reserved_for_layout {
            let full_input_bar_height = self.input_bar.read(cx).rendered_height(cx).as_f32();
            let input_bar_height = if input_bar_progress > 0.01 {
                full_input_bar_height * input_bar_progress
            } else {
                full_input_bar_height
            };
            let input_bar_content_opacity = if input_bar_progress > 0.01 {
                input_bar_content_progress
            } else {
                0.0
            };
            terminal_area = terminal_area.child(
                div()
                    .overflow_hidden()
                    .h(px(input_bar_height))
                    .flex_shrink_0()
                    .bg(input_bar_surface_color)
                    .child(div().h(px(1.0)).bg(chrome_static_seam_color))
                    .child(
                        div()
                            .min_h(px((input_bar_height - 1.0).max(0.0)))
                            .opacity(input_bar_content_opacity)
                            .child(self.input_bar.clone()),
                    ),
            );
        }

        let mut main_area = div()
            .relative()
            .flex()
            .flex_1()
            .min_h_0()
            .when(cfg!(not(target_os = "macos")), |main_area| {
                main_area.bg(portable_terminal_backdrop_color)
            });

        if show_left_panel {
            let mut left_sidebar = div()
                .w(px(left_panel_width))
                .h_full()
                .flex()
                .flex_row()
                .flex_shrink_0()
                .overflow_hidden()
                .occlude()
                .bg(elevated_panel_surface_color);
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
            main_area = main_area.child(left_sidebar);
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

        if show_sidebar_tools {
            let resize_edge = left_panel_width;
            main_area = main_area.child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px((resize_edge - 1.0).max(0.0)))
                    .w(px(1.0))
                    .bg(sidebar_seam_color),
            );
            main_area = main_area.child(
                div()
                    .id("left-sidebar-resize-handle")
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px((resize_edge - 3.0).max(0.0)))
                    .w(px(6.0))
                    .cursor_col_resize()
                    .occlude()
                    .bg(theme.transparent)
                    .hover(|s| s.bg(theme.muted.opacity(0.08)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.release_active_terminal_mouse_selection(cx);
                            let width = this.sidebar.read(cx).panel_width();
                            this.sidebar_drag = Some((f32::from(event.position.x), width));
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
                    .bg(sidebar_seam_color),
            );
        }

        let render_agent_panel = agent_panel_reserved_for_layout;

        if render_agent_panel {
            #[cfg(target_os = "macos")]
            let panel_width = effective_agent_panel_width + 1.0;
            #[cfg(not(target_os = "macos"))]
            let panel_width = if agent_panel_progress > 0.01 {
                animated_panel_width + 1.0
            } else {
                effective_agent_panel_width + 1.0
            };
            #[cfg(target_os = "macos")]
            let agent_panel_content_opacity =
                if self.agent_panel_open || agent_panel_progress > 0.01 {
                    agent_panel_content_progress
                } else {
                    0.0
                };
            #[cfg(not(target_os = "macos"))]
            let agent_panel_content_opacity = agent_panel_content_progress;

            main_area = main_area.child(
                div()
                    .w(px(panel_width))
                    .h_full()
                    .overflow_hidden()
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .bg(elevated_panel_surface_color)
                    .child(
                        div()
                            .id("agent-panel-divider")
                            .relative()
                            .w(px(1.0))
                            .h_full()
                            .flex_shrink_0()
                            .bg(chrome_static_seam_color)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(px(-2.0))
                                    .w(px(5.0))
                                    .cursor_col_resize()
                                    .bg(theme.transparent)
                                    .hover(|s| s.bg(chrome_static_seam_color.opacity(0.18)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, _window, cx| {
                                                this.release_active_terminal_mouse_selection(cx);
                                                this.agent_panel_drag = Some((
                                                    f32::from(event.position.x),
                                                    effective_agent_panel_width,
                                                ));
                                                cx.notify();
                                            },
                                        ),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .opacity(agent_panel_content_opacity)
                            .child(self.agent_panel.clone()),
                    ),
            );
        }

        // Top bar — compact titlebar for one tab, full strip for many
        let top_bar_height = if top_chrome_snap_guard_active {
            TOP_BAR_TABS_HEIGHT
        } else {
            self.current_top_bar_height()
        };
        let top_bar_controls_offset = 1.0 + (3.0 * tab_strip_progress);

        let top_bar = self.render_top_bar(
            window,
            cx,
            top_bar_height,
            top_bar_controls_offset,
            compact_titlebar_progress,
            tab_strip_progress,
            elevated_ui_surface_opacity,
            top_bar_surface_color,
        );
        let show_compact_top_bar_separator =
            cfg!(not(target_os = "macos")) && top_bar_height > 0.5 && tab_strip_progress <= 0.01;

        // Owned clone: `theme` outlives the sidebar overlay update below,
        // which mutably borrows `cx` (the `&Theme` borrow would conflict).
        let theme = cx.theme().clone();
        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.transparent)
            .font_family(theme.mono_font_family.clone())
            .track_focus(&self.workspace_focus);

        root = root
            .key_context("ConWorkspace")
            // Pane drag-to-resize: capture mouse move/up on root so it works
            // even when cursor is over terminal views (which capture mouse events).
            .on_mouse_move({
                cx.listener(move |this, event: &MouseMoveEvent, win, cx| {
                    if this.handle_active_resize_mouse_move(event, win, cx) {
                        return;
                    }

                    if let Some(preview) = this
                        .tab_drag_preview
                        .lock()
                        .ok()
                        .and_then(|guard| guard.clone())
                    {
                        let preview_size = tab_like_drag_preview_size();
                        let leading_pad = top_bar::top_bar_leading_pad(win);
                        let min_left = px(leading_pad);
                        let max_left =
                            (win.viewport_size().width - preview_size.width).max(min_left);
                        let probe = tab_drag_overlay_probe_position(
                            event.position,
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
                        if let Some(slot) =
                            horizontal_tab_slot_from_bounds(probe, &tab_bounds, this.tabs.len())
                        {
                            let new_target = Some(TabDragTarget::Reorder { slot });
                            if this.tab_strip_drop_slot != Some(slot)
                                || this.tab_drag_target != new_target
                            {
                                this.tab_strip_drop_slot = Some(slot);
                                this.tab_drag_target = new_target;
                                cx.notify();
                            }
                        } else {
                            // Cursor left valid tab bounds — clear stale target.
                            if this.tab_strip_drop_slot.is_some() || this.tab_drag_target.is_some()
                            {
                                this.tab_strip_drop_slot = None;
                                this.tab_drag_target = None;
                                cx.notify();
                            }
                        }
                    }

                    if this.active_tab >= this.tabs.len() {
                        return;
                    }

                    let top_bar_height = this.current_top_bar_height();
                    let input_bar_height = if this.input_bar_visible {
                        this.input_bar.read(cx).rendered_height(cx).as_f32()
                    } else {
                        0.0
                    };

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
                    if this.active_tab >= this.tabs.len() {
                        return;
                    }
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
            }))
            .child(top_bar);
        if show_compact_top_bar_separator {
            root = root.child(
                div()
                    .h(px(1.0))
                    .flex_shrink_0()
                    .bg(chrome_static_seam_color),
            );
        }
        root = root.child(main_area);

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
            let preview_size = tab_like_drag_preview_size();
            let leading_pad = top_bar::top_bar_leading_pad(window);
            let min_left = px(leading_pad);
            let max_left = (window.viewport_size().width - preview_size.width).max(min_left);
            let left =
                tab_drag_overlay_origin(window.mouse_position(), &preview, min_left, max_left).x;
            root = root.child(
                div()
                    .absolute()
                    .left(left)
                    .top(preview.source_top)
                    .w(preview_size.width)
                    .h(preview_size.height)
                    .px(px(10.0))
                    .rounded(px(4.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_grab()
                    .bg(theme.title_bar.opacity(0.92))
                    .font_family(theme.font_family.clone())
                    .text_size(px(12.0))
                    .text_color(theme.foreground.opacity(0.82))
                    .child(
                        svg()
                            .path(preview.icon)
                            .size(px(12.0))
                            .flex_shrink_0()
                            .text_color(theme.foreground),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(preview.title)),
            );
        }

        if let Some(drag) = self.pane_title_drag.as_ref().filter(|drag| drag.active) {
            let preview_size = Size {
                width: px(120.0),
                height: px(28.0),
            };
            let preview_origin = if let Some(PaneDropTarget::NewTab { .. }) = drag.target {
                tab_drag_preview_origin(
                    drag.current_pos,
                    preview_size,
                    px(TOP_BAR_COMPACT_HEIGHT),
                    px(TOP_BAR_TABS_HEIGHT - TOP_BAR_COMPACT_HEIGHT),
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
                    .px(px(10.0))
                    .rounded(px(4.0))
                    .flex()
                    .items_center()
                    .cursor_grab()
                    .bg(theme.title_bar.opacity(0.92))
                    .font_family(theme.font_family.clone())
                    .text_size(px(12.0))
                    .text_color(theme.foreground.opacity(0.82))
                    .child(div().truncate().child(drag.title.clone())),
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

        if top_chrome_release_cover_active {
            root = root.child(
                div()
                    .absolute()
                    .top(px(TOP_BAR_COMPACT_HEIGHT))
                    .left(px(terminal_content_left))
                    .right(px(agent_panel_outer_width))
                    .h(px(TOP_BAR_TABS_HEIGHT - TOP_BAR_COMPACT_HEIGHT))
                    .bg(chrome_transition_seam_color),
            );
        }

        if self.sidebar_drag.is_some() {
            root = root.child(
                div()
                    .absolute()
                    .top(px(top_bar_height))
                    .bottom_0()
                    .left(px((left_panel_width - CHROME_MOTION_SEAM_OVERDRAW).max(0.0)))
                    .w(px(CHROME_MOTION_SEAM_OVERDRAW))
                    .bg(chrome_transition_seam_color),
            );
        }

        if input_bar_snap_guard_active {
            if self.input_bar_visible {
                root = root.child(
                    div()
                        .absolute()
                        .left(px(terminal_content_left))
                        .right(px(agent_panel_outer_width))
                        .bottom(self.input_bar.read(cx).rendered_height(cx))
                        .h(px(CHROME_MOTION_SEAM_OVERDRAW))
                        .bg(chrome_transition_seam_color),
                );
            }
        }

        if input_bar_release_cover_active {
            root = root.child(
                div()
                    .absolute()
                    .left(px(terminal_content_left))
                    .right(px(agent_panel_outer_width))
                    .bottom_0()
                    .h(self.input_bar.read(cx).rendered_height(cx))
                    .bg(chrome_transition_seam_color),
            );
        }

        if agent_panel_snap_guard_active {
            if self.agent_panel_open {
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

        if self.agent_panel_drag.is_some() && self.agent_panel_open {
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
            input_bar_progress,
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
