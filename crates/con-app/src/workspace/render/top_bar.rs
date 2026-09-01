use super::super::*;
use gpui_component::menu::ContextMenuExt;

fn sanitize_tab_accent_alpha(alpha: f32) -> f32 {
    if alpha.is_finite() {
        alpha.clamp(0.0, 1.0)
    } else {
        crate::tab_colors::TAB_ACCENT_INACTIVE_ALPHA
    }
}

pub(super) fn top_bar_leading_pad(window: &Window) -> f32 {
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        78.0
    }
    #[cfg(target_os = "linux")]
    {
        if linux_client_decorated(window) {
            10.0
        } else {
            8.0
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = window;
        8.0
    }
}

#[cfg(target_os = "linux")]
fn linux_client_decorated(window: &Window) -> bool {
    matches!(window.window_decorations(), Decorations::Client { .. })
}

fn top_bar_trailing_pad(window: &Window) -> f32 {
    #[cfg(target_os = "windows")]
    {
        let _ = window;
        0.0
    }
    #[cfg(target_os = "linux")]
    {
        if linux_client_decorated(window) {
            8.0
        } else {
            6.0
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = window;
        6.0
    }
}

fn chrome_icon_tone(theme: &gpui_component::Theme, compact_titlebar_progress: f32) -> Hsla {
    let base = if theme.is_dark() { 0.62 } else { 0.52 };
    theme
        .foreground
        .opacity(base + (0.08 * compact_titlebar_progress))
}

fn chrome_toggle_tone(
    theme: &gpui_component::Theme,
    active: bool,
    compact_titlebar_progress: f32,
) -> Hsla {
    if active {
        theme.primary
    } else {
        chrome_icon_tone(theme, compact_titlebar_progress)
    }
}

fn chrome_control_hover_bg(theme: &gpui_component::Theme) -> Hsla {
    theme
        .foreground
        .opacity(if theme.is_dark() { 0.12 } else { 0.075 })
}

fn chrome_control_active_bg(theme: &gpui_component::Theme) -> Hsla {
    theme
        .primary
        .opacity(if theme.is_dark() { 0.14 } else { 0.09 })
}

impl ConWorkspace {
    pub(super) fn render_top_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        top_bar_height: f32,
        top_bar_controls_offset: f32,
        compact_titlebar_progress: f32,
        tab_strip_progress: f32,
        elevated_ui_surface_opacity: f32,
        top_bar_surface_color: Hsla,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        // macOS: leave 78px for the system traffic-light cluster that
        // the OS paints over our content. Windows / Linux: start flush
        // at the left; the Min/Max/Close cluster gets appended at the
        // right end of the bar below. Marking the whole bar as a
        // `Drag` control area makes it move the window on non-macOS
        // (GPUI's hit-test walks buttons first so child clickables
        // still work) and still lets macOS react to
        // `titlebar_double_click`.
        let leading_pad = top_bar_leading_pad(window);
        let trailing_pad = top_bar_trailing_pad(window);
        let mut top_bar = div()
            .id("tab-bar")
            .flex()
            .h(px(top_bar_height))
            .items_end()
            .pl(px(leading_pad))
            .pr(px(trailing_pad))
            .bg(top_bar_surface_color);

        #[cfg(target_os = "macos")]
        {
            top_bar = top_bar
                .window_control_area(WindowControlArea::Drag)
                .on_mouse_down_out(cx.listener(|this, _, _, _| {
                    this.top_bar_should_move = false;
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.top_bar_should_move = false;
                    }),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        // NSWindow.isMovable is set to false so GPUI
                        // element drags (tab reorder, etc.) don't also
                        // move the window. Match Zed's GPUI titlebar
                        // pattern: arm on mouse-down, start native drag
                        // only after actual mouse movement so double
                        // click can still zoom/minimize the window.
                        this.top_bar_should_move = true;
                    }),
                )
                .on_mouse_move(cx.listener(|this, _, window, _| {
                    if this.top_bar_should_move {
                        this.top_bar_should_move = false;
                        window.start_window_move();
                    }
                }))
                .on_click(|event, window, _cx| {
                    if event.click_count() == 2 {
                        window.titlebar_double_click();
                    }
                });
        }

        #[cfg(target_os = "linux")]
        {
            if linux_client_decorated(window) {
                top_bar = top_bar
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, window, _cx| {
                        window.start_window_move();
                    })
                    .on_click(|event, window, _cx| {
                        if event.click_count() == 2 {
                            window.titlebar_double_click();
                        }
                    });
            }
        }

        #[cfg(target_os = "windows")]
        {
            top_bar = top_bar
                .window_control_area(WindowControlArea::Drag)
                .on_click(|event, window, _cx| {
                    if event.click_count() == 2 {
                        window.titlebar_double_click();
                    }
                });
        }

        // Tabs container — appears only when there is real tab selection to do.
        // Clear stale tab reorder indicators only when neither GPUI tab drag nor
        // pane-origin GPUI drag is active.
        let pane_title_drag_to_tab_active =
            pane_title_drag_to_tab_active(self.pane_title_drag.as_ref());
        // Pane-origin drags keep `pane_title_drag` alive from their
        // drag-move handlers. Use that explicit state to keep the tab strip
        // visible instead of inferring pane drags from `cx.has_active_drag()`.
        let pane_origin_drag_active = self
            .pane_title_drag
            .as_ref()
            .is_some_and(|drag| drag.active);
        let tab_strip_preview_active =
            is_tab_strip_preview_active(cx.has_active_drag(), pane_title_drag_to_tab_active)
                || pane_origin_drag_active;
        if !tab_strip_preview_active {
            self.tab_strip_drop_slot = None;
            self.tab_drag_target = None;
            if let Ok(mut guard) = self.active_dragged_tab_session_id.lock() {
                *guard = None;
            }
            if let Ok(mut guard) = self.tab_drag_preview.lock() {
                *guard = None;
            }
        }

        // Also show the tab strip during pane-to-tab drag so the ghost tab
        // preview is visible even when there is currently only one tab.
        let show_horizontal_tabs = self.horizontal_tabs_visible()
            || pane_title_drag_to_tab_active
            || pane_origin_drag_active;
        let tab_count = self.tabs.len();
        let tab_strip_drop_slot = self.tab_strip_drop_slot;
        // Snapshot rename state for this render frame.
        let renaming_tab_index = self.tab_rename.as_ref().map(|r| r.tab_index);
        let rename_input = self.tab_rename.as_ref().map(|r| r.input.clone());
        let mut tabs_container = div()
            .id("tab-strip-container")
            .flex()
            .flex_1()
            .min_w_0()
            .items_end()
            // Container-level drag-move: position-based slot calculation.
            // This is the authoritative handler for HorizontalTabStrip drags.
            // It uses the full tab_strip_tab_bounds array to compute the drop
            // slot from the cursor's absolute X position, so it works even
            // when the cursor jumps over tabs without hovering them individually.
            // The per-tab on_drag_move handlers are kept as a fast path but
            // this container handler guarantees correctness for all paths.
            .on_drag_move::<DraggedTab>(cx.listener({
                let tab_strip_tab_bounds = self.tab_strip_tab_bounds.clone();
                move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                    if event.drag(cx).origin != DraggedTabOrigin::HorizontalTabStrip {
                        return;
                    }
                    if !point_in_bounds(&event.event.position, &event.bounds) {
                        return;
                    }
                    let tab_bounds = tab_strip_tab_bounds
                        .lock()
                        .ok()
                        .map(|g| g.clone())
                        .unwrap_or_default();
                    let current_tab_count = this.tabs.len();
                    let Some(slot) = horizontal_tab_slot_from_bounds(
                        event.event.position,
                        &tab_bounds,
                        current_tab_count,
                    ) else {
                        return;
                    };
                    let new_target = Some(TabDragTarget::Reorder { slot });
                    if this.tab_strip_drop_slot != Some(slot) || this.tab_drag_target != new_target
                    {
                        this.tab_strip_drop_slot = Some(slot);
                        this.tab_drag_target = new_target;
                        cx.notify();
                    }
                }
            }))
            // Pane-origin drag: update tab_strip_drop_slot when cursor is in
            // the tab strip so the ghost tab preview follows the cursor.
            .on_drag_move::<DraggedTab>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                    if event.drag(cx).origin != DraggedTabOrigin::Pane {
                        return;
                    }
                    if !point_in_bounds(&event.event.position, &event.bounds) {
                        return;
                    }
                    let tab_bounds = this
                        .pane_title_drag_tab_bounds
                        .lock()
                        .ok()
                        .map(|g| g.clone())
                        .unwrap_or_default();
                    let tab_count = this.tabs.len();
                    let drop_slot =
                        pane_title_drag_tab_slot(event.event.position, &tab_bounds, tab_count);
                    if this.tab_strip_drop_slot != Some(drop_slot) {
                        this.tab_strip_drop_slot = Some(drop_slot);
                    }
                    let Some(pane_id) = event.drag(cx).pane_id else {
                        return;
                    };
                    this.update_pane_title_drag_state(
                        pane_id,
                        event.event.position,
                        Some(PaneDropTarget::NewTab { slot: drop_slot }),
                        cx,
                    );
                },
            ))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                let to = this.tab_strip_drop_slot.unwrap_or(tab_count);
                clear_pane_tab_promotion_drag_state(
                    &mut this.pane_title_drag,
                    &mut this.tab_strip_drop_slot,
                    &mut this.tab_drag_target,
                );
                if let Ok(mut guard) = this.active_dragged_tab_session_id.lock() {
                    *guard = None;
                }
                if dragged.origin == DraggedTabOrigin::HorizontalTabStrip
                    && let Ok(mut guard) = this.tab_drag_preview.lock()
                {
                    *guard = None;
                }
                if dragged.origin == DraggedTabOrigin::Pane {
                    if let Some(pane_id) = dragged.pane_id {
                        this.detach_pane_to_new_tab_at_slot(pane_id, to, window, cx);
                        return;
                    }
                }
                this.reorder_tab_by_id(dragged.session_id, to, cx);
                cx.notify();
            }));

        if show_horizontal_tabs {
            // Clear stale tab bounds so this frame's prepaint callbacks
            // write a fresh, render-order-indexed snapshot. Without this
            // the ghost-tab gap leaves holes in the bounds array and
            // pane_title_drag_tab_slot computes wrong slots.
            if let Ok(mut guard) = self.tab_strip_tab_bounds.lock() {
                guard.clear();
            }
            // Clear real-tabs-only bounds used by pane-title-drag slot calc.
            if let Ok(mut guard) = self.pane_title_drag_tab_bounds.lock() {
                guard.clear();
            }

            // Compute the visual render order for live drag preview.
            // When a tab is being dragged, we render tabs in the order they
            // would appear if the drag were dropped at the current slot.
            // This gives Chrome-like live reordering during drag.
            let dragged_source_id = self
                .active_dragged_tab_session_id
                .lock()
                .ok()
                .and_then(|guard| *guard);
            // For pane-title-to-tab drag: treat the dragged pane as a new
            // "ghost" tab being inserted at the drop slot. We use a sentinel
            // source_idx of `tab_count` (one past the end) so the reorder
            // logic inserts a gap at the right position without removing any
            // existing tab. The ghost tab is rendered as the drop indicator.
            // For pane-to-tab drag: show ghost tab only when the explicit
            // pane drag state says the cursor is over the tab strip.
            let pane_title_drag_drop_slot = if cx.has_active_drag()
                && matches!(
                    self.pane_title_drag.as_ref().and_then(|drag| drag.target),
                    Some(PaneDropTarget::NewTab { .. })
                ) {
                self.tab_strip_drop_slot
            } else {
                None
            };

            let render_indices: Vec<usize> = if let Some(dragged_id) = dragged_source_id
                && let Some(source_idx) = self.tabs.iter().position(|t| t.summary_id == dragged_id)
                && let Some(drop_slot) = self.tab_strip_drop_slot
            {
                // Build a reordered index list: remove source, insert at drop slot.
                // This gives Chrome-like live reordering during GPUI tab drag.
                let insert_at = if source_idx < drop_slot {
                    (drop_slot - 1).min(self.tabs.len().saturating_sub(1))
                } else {
                    drop_slot.min(self.tabs.len().saturating_sub(1))
                };
                let mut indices: Vec<usize> = (0..self.tabs.len()).collect();
                indices.remove(source_idx);
                indices.insert(insert_at, source_idx);
                indices
            } else {
                (0..self.tabs.len()).collect()
            };

            // visual_pos tracks the actual rendered position in the tab strip,
            // accounting for ghost tab insertions (each ghost shifts subsequent
            // tabs by +1). Used as the key into tab_strip_tab_bounds so that
            // pane_title_drag_tab_slot sees a contiguous, gap-free bounds array.
            let mut visual_pos: usize = 0;
            for render_pos in 0..render_indices.len() {
                let index = render_indices[render_pos];
                let tab = &self.tabs[index];
                let is_active = index == self.active_tab;
                let needs_attention = tab.needs_attention && !is_active;
                let session_id = tab.summary_id;
                let tab_id = session_id;
                let tab_color = tab.color;
                let terminal_progress = tab
                    .pane_tree
                    .focused_pane_terminal()
                    .and_then(|terminal| terminal.progress(cx));
                let is_dragged_source = is_dragged_tab_source(dragged_source_id, session_id);
                let is_editor_only = tab.pane_tree.pane_terminals().is_empty();
                let (hostname_for_tab, title_for_tab, dir_for_tab) =
                    if let Some(terminal) = tab.pane_tree.try_focused_terminal() {
                        (
                            self.effective_remote_host_for_tab(index, terminal, cx),
                            terminal.title(cx),
                            terminal.current_dir(cx),
                        )
                    } else {
                        (None, Some(tab.title.clone()), None)
                    };
                let presentation = smart_tab_presentation(
                    tab.user_label.as_deref(),
                    tab.ai_label.as_deref(),
                    tab.ai_icon.map(|k| k.svg_path()),
                    hostname_for_tab.as_deref(),
                    title_for_tab.as_deref(),
                    dir_for_tab.as_deref(),
                    index,
                    is_editor_only,
                );
                let tab_icon = presentation.icon;

                let display_title: String = if presentation.name.chars().count() > 24 {
                    format!(
                        "{}…",
                        &presentation.name[..presentation.name.floor_char_boundary(22)]
                    )
                } else {
                    presentation.name
                };

                let close_id = ElementId::Name(format!("tab-close-{}", index).into());

                let mut close_el = div()
                    .id(close_id)
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(17.0))
                    .flex_shrink_0()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    // Windows: without `.occlude()` the parent top_bar's
                    // `WindowControlArea::Drag` hit-test swallows the
                    // click (returns HTCAPTION → window drag starts).
                    .occlude()
                    .hover(|s| s.bg(theme.muted.opacity(0.12)));
                if !is_active {
                    close_el = close_el.invisible().group_hover("tab", |s| s.visible());
                }
                let close_button = close_el
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.close_tab_by_id(tab_id, window, cx);
                        }),
                    )
                    .child(
                        svg()
                            .path("phosphor/x.svg")
                            .size(px(10.0))
                            .text_color(theme.muted_foreground.opacity(0.42)),
                    );

                // Drop indicator: 2px vertical line on the left edge of
                // this tab when drop_slot == render_pos, or on the right edge
                // of the last tab when drop_slot == tab_count.
                let show_indicator_left =
                    tab_strip_drop_slot == Some(render_pos) && tab_strip_preview_active;
                let show_indicator_right = render_pos + 1 == tab_count
                    && tab_strip_drop_slot == Some(tab_count)
                    && tab_strip_preview_active;
                let indicator_color = theme.primary;

                let preview_size = tab_like_drag_preview_size();
                let dragged = DraggedTab {
                    session_id,
                    label: display_title.clone().into(),
                    icon: tab_icon,
                    origin: DraggedTabOrigin::HorizontalTabStrip,
                    preview_constraint: Some(crate::sidebar::DraggedTabPreviewConstraint {
                        // Filled with GPUI's real cursor-in-element offset in
                        // on_drag below. The dummy values avoid guessing tab
                        // geometry during render construction.
                        cursor_offset_y: px(0.0),
                        top: px(TOP_BAR_COMPACT_HEIGHT),
                        height: preview_size.height,
                        preview_height: preview_size.height,
                        cursor_offset_x: px(0.0),
                        left: px(leading_pad),
                        bar_width: window.viewport_size().width - px(leading_pad),
                        preview_width: preview_size.width,
                    }),
                    pane_id: None,
                };
                let active_dragged_tab_session_id = self.active_dragged_tab_session_id.clone();
                let tab_drag_preview = self.tab_drag_preview.clone();
                let dragged_tab_source_index = dragged_source_id.and_then(|dragged_id| {
                    self.tabs
                        .iter()
                        .position(|tab| tab.summary_id == dragged_id)
                });

                let mut tab_el = div()
                    .id(ElementId::Name(format!("tab-{}", index).into()))
                    .group("tab")
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(220.0))
                    .items_center()
                    .px(px(10.0))
                    .h(px(30.0))
                    .text_size(px(11.5))
                    .cursor_pointer()
                    // Windows: without `.occlude()` the parent top_bar's
                    // `WindowControlArea::Drag` hit-test routes the click
                    // to the OS (HTCAPTION) and starts a window drag
                    // before GPUI fires `on_click` — so the tab never
                    // activates. Same treatment as the `+`, caption
                    // buttons, and tab-close controls in this file.
                    .occlude()
                    // Left mouse-down: stop propagation so the event
                    // doesn't bubble to top_bar's start_window_move()
                    // on macOS (which steals mouseDragged events and
                    // prevents on_drag from firing). pending_mouse_down
                    // is registered after mouse_down_listeners in GPUI
                    // and runs first in bubble phase (rev order), so
                    // on_click / on_drag still work correctly.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |_this, _, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            if event.click_count() == 2 {
                                this.begin_tab_rename_by_id(tab_id, window, cx);
                            } else {
                                this.activate_tab_by_id(tab_id, window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            this.close_tab_by_id(tab_id, window, cx);
                        }),
                    )
                    .on_drag(
                        dragged,
                        move |dragged: &DraggedTab, offset, window, cx: &mut App| {
                            // Only track session id for tab-strip drags.
                            // Pane-origin drags must not set this or the dragged
                            // tab will be hidden and live-reorder will misfire.
                            if dragged.origin == DraggedTabOrigin::HorizontalTabStrip {
                                if let Ok(mut guard) = active_dragged_tab_session_id.lock() {
                                    *guard = Some(dragged.session_id);
                                }
                            }
                            let mut d = dragged.clone();
                            if let Some(ref mut c) = d.preview_constraint {
                                // GPUI's offset is cursor-in-source-element. Store it
                                // for drop math/debug helpers, but the visible preview
                                // is rendered by Workspace's overlay below.
                                c.cursor_offset_x = offset.x;
                                c.cursor_offset_y = offset.y;
                                c.top = window.mouse_position().y - offset.y;
                                c.height = c.preview_height;
                                if dragged.origin == DraggedTabOrigin::HorizontalTabStrip
                                    && let Ok(mut guard) = tab_drag_preview.lock()
                                {
                                    *guard = Some(TabDragPreviewState {
                                        title: dragged.label.clone(),
                                        icon: dragged.icon,
                                        source_top: c.top,
                                        cursor_offset_x: offset.x,
                                    });
                                }
                            }
                            cx.new(|_| d)
                        },
                    )
                    .on_drag_move::<DraggedTab>(cx.listener(
                        move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                            // Pane-origin drags are handled by the pane content
                            // on_drag_move — skip tab reorder logic here.
                            if event.drag(cx).origin == DraggedTabOrigin::Pane {
                                return;
                            }
                            let slot = match horizontal_tab_slot_from_drag(
                                event.event.position,
                                event.bounds,
                                index,
                                dragged_tab_source_index,
                            ) {
                                Some(s) => s,
                                None => {
                                    // Cursor is on the source tab itself — keep
                                    // the current drop slot unchanged so live
                                    // reorder from a previous frame is preserved.
                                    // (GPUI event.bounds can be one frame stale
                                    // after a reorder, so we must not reset here.)
                                    if this.tab_strip_drop_slot.is_none() {
                                        let source_slot = dragged_tab_source_index.unwrap_or(index);
                                        let keep =
                                            Some(TabDragTarget::Reorder { slot: source_slot });
                                        this.tab_drag_target = keep;
                                        this.tab_strip_drop_slot = Some(source_slot);
                                        cx.notify();
                                    }
                                    return;
                                }
                            };
                            let new_target = Some(TabDragTarget::Reorder { slot });
                            if this.tab_strip_drop_slot != Some(slot)
                                || this.tab_drag_target != new_target
                            {
                                this.tab_drag_target = new_target;
                                this.tab_strip_drop_slot = Some(slot);
                                cx.notify();
                            }
                        },
                    ))
                    .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                        let to = this.tab_strip_drop_slot.unwrap_or(index);
                        if dragged.origin == DraggedTabOrigin::Pane {
                            clear_pane_tab_promotion_drag_state(
                                &mut this.pane_title_drag,
                                &mut this.tab_strip_drop_slot,
                                &mut this.tab_drag_target,
                            );
                        } else {
                            this.tab_strip_drop_slot = None;
                            this.tab_drag_target = None;
                        }
                        if let Ok(mut guard) = this.active_dragged_tab_session_id.lock() {
                            *guard = None;
                        }
                        if dragged.origin == DraggedTabOrigin::HorizontalTabStrip
                            && let Ok(mut guard) = this.tab_drag_preview.lock()
                        {
                            *guard = None;
                        }
                        if dragged.origin == DraggedTabOrigin::Pane {
                            if let Some(pane_id) = dragged.pane_id {
                                this.detach_pane_to_new_tab_at_slot(pane_id, to, window, cx);
                                return;
                            }
                        }
                        this.reorder_tab_by_id(dragged.session_id, to, cx);
                        cx.notify();
                    }));

                if is_active {
                    tab_el = tab_el
                        .rounded_t(px(7.0))
                        .map(|el| match tab_color {
                            Some(color) => el.bg(crate::tab_colors::tab_accent_surface_hsla(
                                color,
                                crate::tab_colors::TAB_ACCENT_ACTIVE_ALPHA,
                                cx,
                            )),
                            None => el.bg(theme.background.opacity(elevated_ui_surface_opacity)),
                        })
                        .text_color(theme.foreground)
                        .font_weight(FontWeight::MEDIUM);
                } else {
                    let inactive_alpha = sanitize_tab_accent_alpha(self.tab_accent_inactive_alpha);
                    let inactive_hover_alpha =
                        sanitize_tab_accent_alpha(self.tab_accent_inactive_hover_alpha)
                            .max(inactive_alpha);
                    let inactive_bg = tab_color
                        .map(|color| {
                            crate::tab_colors::tab_accent_surface_hsla(color, inactive_alpha, cx)
                        })
                        .unwrap_or(theme.background.opacity(0.14));
                    let inactive_hover_bg = tab_color
                        .map(|color| {
                            crate::tab_colors::tab_accent_surface_hsla(
                                color,
                                inactive_hover_alpha,
                                cx,
                            )
                        })
                        .unwrap_or(theme.background.opacity(0.20));
                    tab_el = tab_el
                        .rounded_t(px(6.0))
                        .bg(inactive_bg)
                        .text_color(theme.muted_foreground.opacity(0.72))
                        .hover(move |s: gpui::StyleRefinement| {
                            s.bg(inactive_hover_bg)
                                .text_color(theme.foreground.opacity(0.82))
                        });
                }

                // Left drop indicator line
                if show_indicator_left {
                    tab_el = tab_el.child(
                        div()
                            .absolute()
                            .top(px(4.0))
                            .bottom(px(4.0))
                            .left(px(-1.0))
                            .w(px(2.0))
                            .rounded(px(1.0))
                            .bg(indicator_color),
                    );
                }
                // Right drop indicator line (after last tab)
                if show_indicator_right {
                    tab_el = tab_el.child(
                        div()
                            .absolute()
                            .top(px(4.0))
                            .bottom(px(4.0))
                            .right(px(-1.0))
                            .w(px(2.0))
                            .rounded(px(1.0))
                            .bg(indicator_color),
                    );
                }

                if let Some(progress) = terminal_progress {
                    let (color, percent) = match progress {
                        TerminalProgress::Running(percent) => (theme.progress_bar, percent),
                        TerminalProgress::Error(percent) => (theme.danger, percent),
                        TerminalProgress::Indeterminate => (theme.progress_bar, None),
                        TerminalProgress::Paused(percent) => (theme.warning, percent),
                    };
                    let mut progress_indicator =
                        div().absolute().left_0().right_0().bottom_0().h(px(2.0));
                    if let Some(percent) = percent {
                        progress_indicator = progress_indicator
                            .bg(color.opacity(if is_active { 0.16 } else { 0.10 }))
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(f32::from(percent) / 100.0))
                                    .bg(color.opacity(if is_active { 0.82 } else { 0.58 })),
                            );
                    } else {
                        progress_indicator = progress_indicator.bg(color.opacity(if is_active {
                            0.62
                        } else {
                            0.42
                        }));
                    }
                    tab_el = tab_el.child(progress_indicator);
                }

                let mut tab_content = div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .size_full()
                    .min_w_0();

                if needs_attention {
                    tab_content = tab_content.child(
                        div()
                            .size(px(5.0))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(theme.primary),
                    );
                }

                tab_content = tab_content.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .child(
                            svg()
                                .path(tab_icon)
                                .size(px(12.5))
                                .flex_shrink_0()
                                .text_color(if is_active {
                                    tab_color
                                        .map(|color| {
                                            crate::tab_colors::tab_accent_color_hsla(color, cx)
                                        })
                                        .unwrap_or_else(|| theme.foreground.opacity(0.68))
                                } else {
                                    theme.muted_foreground.opacity(0.38)
                                }),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_x_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family(theme.mono_font_family.clone())
                                .line_height(px(14.0))
                                .font_weight(if is_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if is_active {
                                    theme.foreground.opacity(0.88)
                                } else {
                                    theme.muted_foreground.opacity(0.62)
                                })
                                .child(display_title),
                        ),
                );

                tab_content = tab_content.child(close_button);

                // When this tab is being renamed, replace the title text
                // with an inline Input. Escape cancels; Enter confirms
                // (handled in begin_tab_rename's subscribe_in).
                let is_renaming = renaming_tab_index == Some(index);
                if is_renaming {
                    if let Some(input) = rename_input.clone() {
                        tab_content = div()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .w_full()
                            .min_w_0()
                            .on_action(cx.listener(|this, _: &InputEscape, _, cx| {
                                if let Some(editor) = this.tab_rename.as_ref() {
                                    this.tab_rename_cancelled_generation = Some(editor.generation);
                                }
                                this.tab_rename = None;
                                cx.notify();
                            }))
                            .child(Input::new(&input).small().appearance(false));
                    }
                }

                if is_dragged_source {
                    tab_el = tab_el.opacity(0.0);
                }

                // Capture visual_pos for this tab *before* the ghost-tab check
                // below may increment it. The ghost (if any) occupies visual_pos,
                // and this real tab occupies visual_pos+1 — but we record that
                // after the ghost block via the `tab_visual_pos` snapshot here.
                let tab_visual_pos = if pane_title_drag_drop_slot == Some(render_pos) {
                    visual_pos + 1
                } else {
                    visual_pos
                };
                let tab_strip_tab_bounds_for_prepaint = self.tab_strip_tab_bounds.clone();
                let pane_title_drag_tab_bounds_for_prepaint =
                    self.pane_title_drag_tab_bounds.clone();
                tab_el = tab_el.on_prepaint(move |bounds, _, _| {
                    if let Ok(mut guard) = tab_strip_tab_bounds_for_prepaint.lock() {
                        if guard.len() <= tab_visual_pos {
                            guard.resize(tab_visual_pos + 1, Bounds::default());
                        }
                        guard[tab_visual_pos] = bounds;
                    }
                    // Also record in the real-tabs-only array (render_pos order,
                    // no ghost) so pane-title-drag slot calc is ghost-agnostic.
                    if let Ok(mut guard) = pane_title_drag_tab_bounds_for_prepaint.lock() {
                        if guard.len() <= render_pos {
                            guard.resize(render_pos + 1, Bounds::default());
                        }
                        guard[render_pos] = bounds;
                    }
                });

                // Ghost tab gap: when a pane is being dragged to become a new
                // tab, show a placeholder tab at the target slot so the other
                // tabs visually shift to make room — same feel as Chrome tab drag.
                if pane_title_drag_drop_slot == Some(render_pos) {
                    let ghost_title = self
                        .pane_title_drag
                        .as_ref()
                        .map(|d| d.title.clone())
                        .unwrap_or_default();
                    let ghost_visual_pos = visual_pos;
                    let tab_strip_tab_bounds_for_ghost = self.tab_strip_tab_bounds.clone();
                    tabs_container = tabs_container.child(
                        div()
                            .id(ElementId::Name(format!("tab-ghost-{render_pos}").into()))
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .max_w(px(200.0))
                            .items_center()
                            .px(px(10.0))
                            .h(px(30.0))
                            .text_size(px(11.5))
                            .rounded_t(px(6.0))
                            .bg(theme.primary.opacity(0.18))
                            .text_color(theme.foreground.opacity(0.6))
                            .on_prepaint(move |bounds, _, _| {
                                if let Ok(mut guard) = tab_strip_tab_bounds_for_ghost.lock() {
                                    if guard.len() <= ghost_visual_pos {
                                        guard.resize(ghost_visual_pos + 1, Bounds::default());
                                    }
                                    guard[ghost_visual_pos] = bounds;
                                }
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(5.0))
                                    .w_full()
                                    .min_w_0()
                                    .child(
                                        svg()
                                            .path("phosphor/terminal.svg")
                                            .size(px(12.0))
                                            .flex_shrink_0()
                                            .text_color(theme.primary.opacity(0.7)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_x_hidden()
                                            .whitespace_nowrap()
                                            .child(ghost_title),
                                    ),
                            ),
                    );
                    visual_pos += 1;
                }

                tabs_container = tabs_container.child({
                    let weak = cx.weak_entity();
                    let has_right = index + 1 < tab_count;
                    let has_others = tab_count > 1;
                    let tab_id = session_id;
                    tab_el.child(tab_content).context_menu(
                        move |menu: gpui_component::menu::PopupMenu, _window, _cx| {
                            use crate::tab_context_menu::{TabMenuOptions, build_tab_context_menu};
                            let w = weak.clone();
                            build_tab_context_menu(
                                menu,
                                TabMenuOptions {
                                    rename: {
                                        let w = w.clone();
                                        Box::new(move |window, cx| {
                                            if let Some(e) = w.upgrade() {
                                                e.update(cx, |this, cx| {
                                                    this.begin_tab_rename_by_id(tab_id, window, cx)
                                                });
                                            }
                                        })
                                    },
                                    duplicate: {
                                        let w = w.clone();
                                        Box::new(move |window, cx| {
                                            if let Some(e) = w.upgrade() {
                                                e.update(cx, |this, cx| {
                                                    this.duplicate_tab_by_id(tab_id, window, cx)
                                                });
                                            }
                                        })
                                    },
                                    reset_name: None,
                                    move_up: None,
                                    move_down: None,
                                    close_to_right: if has_right {
                                        let w = w.clone();
                                        Some(Box::new(move |window, cx| {
                                            if let Some(e) = w.upgrade() {
                                                e.update(cx, |this, cx| {
                                                    this.close_tabs_to_right_by_id(
                                                        tab_id, window, cx,
                                                    )
                                                });
                                            }
                                        }))
                                    } else {
                                        None
                                    },
                                    close_tab: {
                                        let w = w.clone();
                                        Box::new(move |window, cx| {
                                            if let Some(e) = w.upgrade() {
                                                e.update(cx, |this, cx| {
                                                    this.close_tab_by_id(tab_id, window, cx)
                                                });
                                            }
                                        })
                                    },
                                    close_others: if has_others {
                                        let w = w.clone();
                                        Some(Box::new(move |window, cx| {
                                            if let Some(e) = w.upgrade() {
                                                e.update(cx, |this, cx| {
                                                    this.close_other_tabs_by_id(tab_id, window, cx)
                                                });
                                            }
                                        }))
                                    } else {
                                        None
                                    },
                                    set_color: {
                                        let w = w.clone();
                                        Box::new(move |color, _window, cx| {
                                            if let Some(e) = w.upgrade() {
                                                e.update(cx, |this, cx| {
                                                    this.set_tab_color_by_id(tab_id, color, cx)
                                                });
                                            }
                                        })
                                    },
                                    current_color: tab_color,
                                },
                            )
                        },
                    )
                });
                visual_pos += 1;

                if index + 1 == tab_count {
                    tabs_container = tabs_container.child(
                        div()
                            .id(ElementId::Name(format!("tab-trailing-drop-{index}").into()))
                            .w(px(12.0))
                            .h_full()
                            .flex_shrink_0()
                            .on_drag_move::<DraggedTab>(cx.listener(
                                move |this, event: &gpui::DragMoveEvent<DraggedTab>, _, cx| {
                                    if event.drag(cx).origin == DraggedTabOrigin::Pane {
                                        return;
                                    }
                                    let Some(slot) = trailing_drop_slot_from_position(
                                        event.event.position,
                                        event.bounds,
                                        tab_count,
                                    ) else {
                                        return;
                                    };
                                    let new_target = Some(TabDragTarget::Reorder { slot });
                                    if this.tab_strip_drop_slot != Some(slot)
                                        || this.tab_drag_target != new_target
                                    {
                                        this.tab_drag_target = new_target;
                                        this.tab_strip_drop_slot = Some(slot);
                                        cx.notify();
                                    }
                                },
                            ))
                            .on_drop(cx.listener(move |this, dragged: &DraggedTab, window, cx| {
                                let to = this.tab_strip_drop_slot.unwrap_or(tab_count);
                                if dragged.origin == DraggedTabOrigin::Pane {
                                    clear_pane_tab_promotion_drag_state(
                                        &mut this.pane_title_drag,
                                        &mut this.tab_strip_drop_slot,
                                        &mut this.tab_drag_target,
                                    );
                                } else {
                                    this.tab_strip_drop_slot = None;
                                    this.tab_drag_target = None;
                                }
                                if let Ok(mut guard) = this.active_dragged_tab_session_id.lock() {
                                    *guard = None;
                                }
                                if dragged.origin == DraggedTabOrigin::Pane {
                                    if let Some(pane_id) = dragged.pane_id {
                                        this.detach_pane_to_new_tab_at_slot(
                                            pane_id, to, window, cx,
                                        );
                                        return;
                                    }
                                }
                                this.reorder_tab_by_id(dragged.session_id, to, cx);
                                cx.notify();
                            })),
                    );

                    // Ghost tab at the end when pane drag targets slot == tab_count
                    if pane_title_drag_drop_slot == Some(tab_count) {
                        let ghost_title = self
                            .pane_title_drag
                            .as_ref()
                            .map(|d| d.title.clone())
                            .unwrap_or_default();
                        let ghost_visual_pos = visual_pos;
                        let tab_strip_tab_bounds_for_end_ghost = self.tab_strip_tab_bounds.clone();
                        tabs_container = tabs_container.child(
                            div()
                                .id(ElementId::Name("tab-ghost-end".into()))
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .max_w(px(200.0))
                                .items_center()
                                .px(px(10.0))
                                .h(px(30.0))
                                .text_size(px(11.5))
                                .rounded_t(px(6.0))
                                .bg(theme.primary.opacity(0.18))
                                .text_color(theme.foreground.opacity(0.6))
                                .on_prepaint(move |bounds, _, _| {
                                    if let Ok(mut guard) = tab_strip_tab_bounds_for_end_ghost.lock()
                                    {
                                        if guard.len() <= ghost_visual_pos {
                                            guard.resize(ghost_visual_pos + 1, Bounds::default());
                                        }
                                        guard[ghost_visual_pos] = bounds;
                                    }
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.0))
                                        .w_full()
                                        .min_w_0()
                                        .child(
                                            svg()
                                                .path("phosphor/terminal.svg")
                                                .size(px(12.0))
                                                .flex_shrink_0()
                                                .text_color(theme.primary.opacity(0.7)),
                                        )
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .overflow_x_hidden()
                                                .whitespace_nowrap()
                                                .child(ghost_title),
                                        ),
                                ),
                        );
                        visual_pos += 1;
                    }
                }
            }
        }

        let mut leading_chrome = div()
            .id("top-bar-leading-chrome")
            .flex()
            .flex_1()
            .min_w_0()
            .items_end();
        #[cfg(target_os = "linux")]
        {
            // Keep Linux's client-side titlebar draggable without
            // putting the right-side controls under a parent drag
            // handler. GPUI Linux does not dispatch window-control
            // hit tests like Windows, so the draggable area must stay
            // clear of normal buttons.
            if linux_client_decorated(window) {
                leading_chrome = leading_chrome
                    .window_control_area(WindowControlArea::Drag)
                    .on_mouse_down(MouseButton::Left, |_, window, _cx| {
                        window.start_window_move();
                    })
                    .on_click(|event, window, _cx| {
                        if event.click_count() == 2 {
                            window.titlebar_double_click();
                        }
                    });
            }
        }
        // Show the tab strip when animating/visible OR when a pane is being
        // dragged to become a new tab (so the ghost tab preview is visible
        // even when there is currently only one tab).
        if tab_strip_progress > 0.01 || pane_title_drag_to_tab_active {
            let opacity = if pane_title_drag_to_tab_active && tab_strip_progress < 0.01 {
                1.0
            } else {
                tab_strip_progress
            };
            leading_chrome = leading_chrome.child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .opacity(opacity)
                    .child(tabs_container),
            );
        }

        top_bar = top_bar.child(leading_chrome);

        // Right-side controls — compact row
        let mut tab_controls = div()
            .flex()
            .items_center()
            .gap(px(3.0))
            .mb(px(top_bar_controls_offset))
            .ml(px(if show_horizontal_tabs { 8.0 } else { 0.0 }))
            .flex_shrink_0();
        #[cfg(target_os = "linux")]
        {
            if linux_client_decorated(window) {
                tab_controls = tab_controls.mr(px(4.0));
            }
        }

        let new_tab_icon_color = chrome_icon_tone(theme, compact_titlebar_progress);
        let chrome_hover_bg = chrome_control_hover_bg(theme);
        let chrome_active_bg = chrome_control_active_bg(theme);
        let new_tab_button = div()
            .id("tab-new")
            .flex()
            .items_center()
            .justify_center()
            .size(px(24.0))
            .rounded(px(6.0))
            .cursor_pointer()
            // `.occlude()` is required on Windows so the parent
            // top_bar's `WindowControlArea::Drag` hit-test doesn't
            // swallow this button (the OS would return HTCAPTION and
            // start a window-drag on click instead of firing the
            // click listener). Same treatment as the Min/Max/Close
            // caption buttons at the top of this file.
            .occlude()
            .hover(move |s| s.bg(chrome_hover_bg))
            .tooltip(|window, cx| {
                chrome_tooltip(
                    "New tab",
                    crate::keycaps::first_action_keystroke(&NewTab, window),
                    window,
                    cx,
                )
            });
        let new_tab_button = new_tab_button
            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                cx.stop_propagation();
            })
            .on_click(cx.listener(|this, _, window, cx| {
                this.new_tab(&NewTab, window, cx);
            }));
        tab_controls = tab_controls.child(
            new_tab_button.child(
                svg()
                    .path("phosphor/plus.svg")
                    .size(px(12.0))
                    .text_color(new_tab_icon_color),
            ),
        );

        let left_sidebar_tooltip = if self.left_panel_open {
            "Hide left sidebar"
        } else {
            "Show left sidebar"
        };
        tab_controls = tab_controls.child(
            div()
                .id("toggle-left-sidebar")
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .occlude()
                .bg(if self.left_panel_open {
                    chrome_active_bg
                } else {
                    theme.transparent
                })
                .hover(move |s| s.bg(chrome_hover_bg))
                .tooltip(move |window, cx| {
                    chrome_tooltip(
                        left_sidebar_tooltip,
                        crate::keycaps::first_action_keystroke(&ToggleLeftPanel, window),
                        window,
                        cx,
                    )
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_left_panel(&ToggleLeftPanel, window, cx);
                }))
                .child(
                    svg()
                        .path("phosphor/sidebar.svg")
                        .size(px(12.0))
                        .text_color(chrome_toggle_tone(
                            theme,
                            self.left_panel_open,
                            compact_titlebar_progress,
                        )),
                ),
        );

        // Input bar toggle
        let input_bar_tooltip = if self.input_bar_visible {
            "Hide input bar"
        } else {
            "Show input bar"
        };
        tab_controls = tab_controls.child(
            div()
                .id("toggle-input-bar")
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .occlude()
                .bg(if self.input_bar_visible {
                    chrome_active_bg
                } else {
                    theme.transparent
                })
                .hover(move |s| s.bg(chrome_hover_bg))
                .tooltip(move |window, cx| {
                    chrome_tooltip(
                        input_bar_tooltip,
                        crate::keycaps::first_action_keystroke(&crate::ToggleInputBar, window),
                        window,
                        cx,
                    )
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_input_bar(&crate::ToggleInputBar, window, cx);
                }))
                .child(
                    svg()
                        .path("phosphor/square-half-bottom-fill.svg")
                        .size(px(12.0))
                        .text_color(chrome_toggle_tone(
                            theme,
                            self.input_bar_visible,
                            compact_titlebar_progress,
                        )),
                ),
        );

        // Agent panel toggle
        let agent_panel_tooltip = if self.agent_panel_open {
            "Hide agent panel"
        } else {
            "Show agent panel"
        };
        tab_controls = tab_controls.child(
            div()
                .id("toggle-agent-panel")
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.0))
                .rounded(px(5.0))
                .cursor_pointer()
                .occlude()
                .bg(if self.agent_panel_open {
                    chrome_active_bg
                } else {
                    theme.transparent
                })
                .hover(move |s| s.bg(chrome_hover_bg))
                .tooltip(move |window, cx| {
                    chrome_tooltip(
                        agent_panel_tooltip,
                        crate::keycaps::first_action_keystroke(&ToggleAgentPanel, window),
                        window,
                        cx,
                    )
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                    cx.stop_propagation();
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_agent_panel(&ToggleAgentPanel, window, cx);
                }))
                .child(
                    svg()
                        .path("phosphor/square-half-fill.svg")
                        .size(px(12.0))
                        .text_color(chrome_toggle_tone(
                            theme,
                            self.agent_panel_open,
                            compact_titlebar_progress,
                        )),
                ),
        );

        // Settings button — only on platforms without a native menu
        // bar. macOS exposes Settings through `App → Settings…` (and
        // ⌘,) so a gear in the chrome would be redundant there. On
        // Windows and Linux it's the primary discovery surface for
        // Settings alongside the command palette.
        #[cfg(not(target_os = "macos"))]
        {
            tab_controls = tab_controls.child(
                div()
                    .id("toggle-settings")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(22.0))
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .occlude()
                    .hover(move |s| s.bg(chrome_hover_bg))
                    .tooltip(|window, cx| {
                        chrome_tooltip(
                            "Settings",
                            crate::keycaps::first_action_keystroke(
                                &settings_panel::ToggleSettings,
                                window,
                            ),
                            window,
                            cx,
                        )
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.toggle_settings(&settings_panel::ToggleSettings, window, cx);
                    }))
                    .child(
                        svg()
                            .path("phosphor/gear.svg")
                            .size(px(12.0))
                            .text_color(chrome_icon_tone(theme, compact_titlebar_progress)),
                    ),
            );
        }

        top_bar = top_bar.child(tab_controls);

        // Non-macOS caption buttons: Min / (Max|Restore) / Close.
        // macOS gets its traffic-light cluster from the system. We
        // render these *inside* the top bar so they share the same
        // vertical strip and never occlude terminal content.
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            #[cfg(target_os = "linux")]
            let workspace_handle = cx.weak_entity();
            #[cfg(target_os = "linux")]
            if linux_client_decorated(window) {
                top_bar = top_bar.child(caption_buttons(
                    window,
                    theme,
                    top_bar_height,
                    workspace_handle,
                ));
            }
            #[cfg(target_os = "windows")]
            {
                top_bar = top_bar.child(caption_buttons(window, theme, top_bar_height));
            }
        }

        top_bar
    }
}
