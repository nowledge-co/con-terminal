use super::super::*;
use crate::ui_scale::ui_px;
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::menu::{ContextMenuExt, DropdownMenu as _, PopupMenuItem};
use gpui_component::tooltip::Tooltip;

const CHROME_CONTROL_SIZE: f32 = 22.0;
const TAB_STRIP_INSET: f32 = (TOP_BAR_TABS_HEIGHT - TOP_BAR_TAB_HEIGHT) * 0.5;
const HORIZONTAL_TAB_MIN_WIDTH: f32 = 120.0;
const HORIZONTAL_TAB_MAX_WIDTH: f32 = 200.0;
const TAB_WINDOW_DRAG_GAP: f32 = 64.0;
const TAB_CLOSE_SIZE: f32 = 20.0;
const TAB_SELECTION_INSET: f32 = 2.0;
const TAB_SURFACE_HEIGHT: f32 = TOP_BAR_TAB_HEIGHT - 2.0 * TAB_SELECTION_INSET;
const TAB_CLOSE_CIRCLE_SIZE: f32 = 14.0;
const TAB_IDENTITY_LABEL_GAP: f32 = 6.0;
const TAB_IDENTITY_INSET: f32 = 8.0;
const TAB_CONTENT_INSET: f32 = TAB_IDENTITY_INSET + TAB_CLOSE_SIZE + TAB_IDENTITY_LABEL_GAP;
const TAB_TITLE_TRAILING_INSET: f32 =
    TAB_CLOSE_SIZE + TAB_IDENTITY_LABEL_GAP + (TAB_SURFACE_HEIGHT - TAB_CLOSE_SIZE) * 0.5;
const TAB_TRAILING_DROP_WIDTH: f32 = 12.0;

fn tab_trailing_drop_target() -> gpui::Div {
    // Keep the end-of-strip drop affordance without extending the painted tray.
    div()
        .absolute()
        .right(px(-TAB_TRAILING_DROP_WIDTH))
        .top_0()
        .w(px(TAB_TRAILING_DROP_WIDTH))
        .h_full()
}

fn titlebar_icon_size(theme: &gpui_component::Theme, base: f32) -> Pixels {
    // Even logical dimensions centre on the same pixel grid as the fixed
    // square controls at both 1x and 2x, including when UI scaling is applied.
    px((ui_icon_px(theme, base).as_f32() * 0.5).round() * 2.0)
}

fn tab_identity_icon_size(theme: &gpui_component::Theme) -> Pixels {
    titlebar_icon_size(theme, 16.0).min(px(TAB_CLOSE_SIZE))
}

fn horizontal_tab_icon(icon: &'static str) -> &'static str {
    crate::assets::tab_identity_asset(icon)
}

fn tab_menu_row(
    session_id: u64,
    title: SharedString,
    icons: &[&'static str],
    active: bool,
    theme: &gpui_component::Theme,
) -> AnyElement {
    div()
        .id(("tab-menu-row", session_id))
        .role(gpui::Role::Label)
        .aria_label(title.clone())
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(ui_px(theme, 28.0))
        .px(px(8.0))
        .gap(px(10.0))
        .font_family(theme.font_family.clone())
        .text_size(ui_px(theme, 12.0))
        .line_height(ui_px(theme, 18.0))
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.foreground)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(22.0))
                .flex_shrink_0()
                .child(crate::assets::tab_identity_group(
                    icons,
                    ui_icon_px(theme, 16.0),
                    theme.foreground,
                )),
        )
        .child(div().flex_1().min_w_0().truncate().child(title))
        .child(
            div()
                .size(px(16.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .children(active.then(|| {
                    svg()
                        .path("phosphor/check.svg")
                        .size(px(12.0))
                        .text_color(theme.foreground)
                })),
        )
        .into_any_element()
}

fn schedule_tab_reveal(
    selection: (u64, usize, usize),
    bounds: Bounds<Pixels>,
    scroll: &gpui::ScrollHandle,
    state: &HorizontalTabRevealState,
    window: &mut Window,
) {
    let viewport = scroll.bounds();
    if viewport.size.width <= px(0.0) {
        return;
    }
    let key = (selection.0, selection.1, selection.2, viewport.size.width);
    if state.get() == Some(key) {
        return;
    }
    state.set(Some(key));
    let offset = scroll.offset();
    let scroll = scroll.clone();
    let state = state.clone();
    window.on_next_frame(move |window, cx| {
        // A newer selection or manual scroll wins over this frame's request.
        if cx.has_active_drag() || state.get() != Some(key) || scroll.offset() != offset {
            return;
        }
        let viewport = scroll.bounds();
        let shift = if bounds.size.width > viewport.size.width || bounds.left() < viewport.left() {
            viewport.left() - bounds.left()
        } else if bounds.right() > viewport.right() {
            viewport.right() - bounds.right()
        } else {
            px(0.0)
        };
        if shift != px(0.0) {
            scroll.set_offset(point(offset.x + shift, offset.y));
            window.refresh();
        }
    });
}

pub(super) fn tab_drag_capsule(
    preview: &TabDragPreviewState,
    theme: &gpui_component::Theme,
) -> gpui::Div {
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(TAB_IDENTITY_LABEL_GAP))
        .pl(px(TAB_CONTENT_INSET))
        .pr(px(TAB_TITLE_TRAILING_INSET))
        .w(preview.size.width)
        .h(preview.size.height)
        .rounded(px(preview.size.height.as_f32() * 0.5))
        .bg(tab_selected_surface(
            theme.title_bar,
            theme.foreground,
            theme.is_dark(),
        ))
        .font_family(theme.font_family.clone())
        .text_size(ui_px(theme, 11.5))
        .line_height(ui_px(theme, 16.0))
        .font_weight(FontWeight::NORMAL)
        .text_color(theme.foreground)
        .child(
            tab_identity_icon_slot().child(
                div()
                    .debug_selector(|| "drag-capsule-icon".into())
                    .size(tab_identity_icon_size(theme))
                    .flex_shrink_0()
                    .child(crate::assets::tab_identity_icon(
                        horizontal_tab_icon(preview.icon),
                        tab_identity_icon_size(theme),
                        theme.foreground,
                    )),
            ),
        )
        .child(
            div()
                .debug_selector(|| "drag-capsule-label".into())
                .flex_1()
                .min_w_0()
                .truncate()
                .child(preview.title.clone()),
        )
}

fn tab_tray_container() -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .h(px(TOP_BAR_TAB_HEIGHT))
        .mb(px(TAB_STRIP_INSET))
}

fn horizontal_tab_viewport(tab_count: usize) -> gpui::Div {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .max_w(px(tab_count as f32 * HORIZONTAL_TAB_MAX_WIDTH))
}

fn scroll_occluded_tab(
    event: &ScrollWheelEvent,
    scroll: &gpui::ScrollHandle,
    window: &mut Window,
    cx: &mut App,
) {
    let delta = event.delta.pixel_delta(window.line_height());
    let delta_x = if delta.x.is_zero() { delta.y } else { delta.x };
    let offset = scroll.offset();
    let next = (offset.x + delta_x).clamp(-scroll.max_offset().x, px(0.0));
    if next != offset.x {
        scroll.set_offset(point(next, offset.y));
        window.refresh();
    }
    cx.stop_propagation();
}

fn tab_overflow_sides(scroll: &gpui::ScrollHandle) -> (bool, bool) {
    let max = scroll.max_offset().x.as_f32().max(0.0);
    let position = (-scroll.offset().x.as_f32()).clamp(0.0, max);
    (position > 0.5, max - position > 0.5)
}

#[allow(clippy::too_many_arguments)]
fn horizontal_tab_strip(
    count: usize,
    content: AnyElement,
    scroll: &gpui::ScrollHandle,
    edges: &Rc<Cell<(bool, bool)>>,
    material: Hsla,
    theme: &gpui_component::Theme,
    owner: gpui::EntityId,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let visible = edges.get();
    let overflow = visible.0 || visible.1;
    let mut viewport = Some(
        div()
            .id("horizontal-tab-viewport")
            .debug_selector(|| "horizontal-tab-viewport".into())
            .flex_1()
            .min_w_0()
            .when(overflow, |viewport| {
                viewport
                    .h(px(TOP_BAR_TAB_HEIGHT))
                    .mb(px(TAB_STRIP_INSET))
                    .bg(tab_resting_surface(theme.foreground, theme.is_dark()))
                    .when(!visible.0, |viewport| {
                        viewport.rounded_l(px(TOP_BAR_TAB_HEIGHT * 0.5))
                    })
                    .when(!visible.1, |viewport| {
                        viewport.rounded_r(px(TOP_BAR_TAB_HEIGHT * 0.5))
                    })
            })
            .overflow_x_scroll()
            .track_scroll(scroll)
            .child(content),
    );
    let mut strip = horizontal_tab_viewport(count)
        .id("horizontal-tab-container")
        .relative()
        .items_center()
        .h(px(TOP_BAR_TAB_HEIGHT + TAB_STRIP_INSET));
    for (left, show) in [(true, visible.0), (false, visible.1)] {
        if !left {
            strip = strip.child(viewport.take().expect("tab viewport is inserted once"));
        }
        if !overflow {
            continue;
        }
        let scroll = scroll.clone();
        let wheel_scroll = scroll.clone();
        let id = if left {
            "tab-scroll-left"
        } else {
            "tab-scroll-right"
        };
        strip = strip.child(
            div()
                // Keep both gutters while overflowing: reaching an endpoint must
                // not move the viewport or cover the first/last tab's label.
                .w(px(24.0))
                .h(px(TOP_BAR_TAB_HEIGHT))
                .mb(px(TAB_STRIP_INSET))
                .when(show, |slot| {
                    slot.bg(tab_resting_surface(theme.foreground, theme.is_dark()))
                })
                .when(left, |slot| slot.rounded_l(px(TOP_BAR_TAB_HEIGHT * 0.5)))
                .when(!left, |slot| slot.rounded_r(px(TOP_BAR_TAB_HEIGHT * 0.5)))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .when(show, |slot| {
                    slot.child(
                        Button::new(id)
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(material.opacity(0.0))
                                    .hover(
                                        material
                                            .opacity(1.0)
                                            .blend(theme.foreground.opacity(0.065)),
                                    )
                                    .active(
                                        material.opacity(1.0).blend(theme.foreground.opacity(0.10)),
                                    ),
                            )
                            .size(px(18.0))
                            .w(px(18.0))
                            .h(px(18.0))
                            .p_0()
                            .rounded_full()
                            .tab_stop(false)
                            .occlude()
                            .debug_selector(move || id.into())
                            .accessibility_label(if left {
                                "Scroll tabs left"
                            } else {
                                "Scroll tabs right"
                            })
                            .tooltip(if left {
                                "Scroll tabs left"
                            } else {
                                "Scroll tabs right"
                            })
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_scroll_wheel(move |event, window, cx| {
                                scroll_occluded_tab(event, &wheel_scroll, window, cx);
                            })
                            .on_click(move |_, window, cx| {
                                let offset = scroll.offset();
                                let step = scroll.bounds().size.width * 0.7;
                                let next = (offset.x + if left { step } else { -step })
                                    .clamp(-scroll.max_offset().x, px(0.0));
                                scroll.set_offset(point(next, offset.y));
                                window.refresh();
                                cx.stop_propagation();
                            })
                            .child(
                                svg()
                                    .path(if left {
                                        "phosphor/caret-left.svg"
                                    } else {
                                        "phosphor/caret-right.svg"
                                    })
                                    .size(px(11.0))
                                    .text_color(theme.foreground),
                            ),
                    )
                }),
        );
    }
    let scroll = scroll.clone();
    let edges = edges.clone();
    strip.on_prepaint(move |_, window, _| {
        let next = tab_overflow_sides(&scroll);
        if edges.replace(next) != next {
            // Only endpoint visibility changes invalidate chrome; no idle frame loop.
            window.on_next_frame(move |_, cx| cx.notify(owner));
        }
    })
}

fn tab_tray_layer(background: Hsla) -> gpui::Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .left_0()
        .right_0()
        .rounded(px(TOP_BAR_TAB_HEIGHT * 0.5))
        .bg(background)
}

fn tab_selection_layer(background: Hsla, hover_background: Hsla) -> gpui::Div {
    div()
        .absolute()
        .top(px(TAB_SELECTION_INSET))
        .bottom(px(TAB_SELECTION_INSET))
        .left_0()
        .right_0()
        .rounded(px(TAB_SURFACE_HEIGHT * 0.5))
        .bg(background)
        .group_hover("tab", move |style| style.bg(hover_background))
}

fn horizontal_tab_target() -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_1()
        .min_w(px(HORIZONTAL_TAB_MIN_WIDTH))
        .items_center()
        .justify_center()
        .px(px(TAB_SELECTION_INSET))
        .h(px(TOP_BAR_TAB_HEIGHT))
}

fn animated_tab_target(weight: f32) -> gpui::Div {
    horizontal_tab_target()
        .flex_grow(weight)
        .min_w(px(HORIZONTAL_TAB_MIN_WIDTH * weight))
        .px(px(TAB_SELECTION_INSET * weight))
        .when(weight < 1.0, |tab| tab.overflow_hidden())
}

fn departing_tab(
    exit: &super::super::tab_motion::DepartingTab,
    theme: &gpui_component::Theme,
) -> gpui::Stateful<gpui::Div> {
    let (background, ink) = exit.colors.unwrap_or((theme.transparent, theme.foreground));
    animated_tab_target(exit.weight)
        .id(("departing-tab", exit.id))
        .font_family(theme.font_family.clone())
        .text_size(ui_px(theme, 11.5))
        .line_height(ui_px(theme, 16.0))
        .font_weight(FontWeight::NORMAL)
        .text_color(ink)
        .child(
            div()
                .relative()
                .flex()
                .flex_1()
                .h_full()
                .min_w_0()
                .child(tab_selection_layer(background, background))
                .child(
                    tab_identity_icon_slot().child(crate::assets::tab_identity_icon(
                        horizontal_tab_icon(exit.icon),
                        tab_identity_icon_size(theme),
                        ink,
                    )),
                )
                .child(tab_identity_row().child(div().truncate().child(exit.title.clone()))),
        )
}

fn tab_identity_row() -> gpui::Div {
    div()
        .flex()
        .flex_1()
        .items_center()
        .justify_center()
        .gap(px(TAB_IDENTITY_LABEL_GAP))
        .pl(px(TAB_CONTENT_INSET))
        .pr(px(TAB_TITLE_TRAILING_INSET))
        .h_full()
        .min_w_0()
}

fn tab_identity_icon_slot() -> gpui::Div {
    div()
        .absolute()
        .left(px(TAB_IDENTITY_INSET))
        .top_0()
        .flex()
        .items_center()
        .justify_center()
        .w(px(TAB_CLOSE_SIZE))
        .h_full()
        .flex_shrink_0()
}

fn tab_name_label() -> gpui::Div {
    div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .h(px(TAB_SURFACE_HEIGHT))
}

fn tab_attention_mark(color: Hsla) -> gpui::Div {
    div()
        .absolute()
        .left(px(TAB_IDENTITY_INSET + TAB_CLOSE_SIZE - 2.0))
        .top(px(TAB_SELECTION_INSET))
        .size(px(5.0))
        .rounded_full()
        .bg(color)
}

fn tab_close_slot() -> gpui::Div {
    div()
        .absolute()
        .right(px((TAB_SURFACE_HEIGHT - TAB_CLOSE_SIZE) * 0.5))
        .top(px((TOP_BAR_TAB_HEIGHT - TAB_CLOSE_SIZE) * 0.5))
        .size(px(TAB_CLOSE_SIZE))
        .flex()
        .items_center()
        .justify_center()
}

fn tab_label_detail(name: &str, subtitle: Option<&str>, status: Option<&str>) -> String {
    let mut lines = vec![name];
    for detail in [subtitle, status].into_iter().flatten() {
        if !detail.is_empty() && !lines.contains(&detail) {
            lines.push(detail);
        }
    }
    lines.join("\n")
}

fn tab_close_button(id: ElementId, cx: &App) -> Button {
    let theme = cx.theme();
    let ink = theme.foreground.blend(theme.background.opacity(0.15));
    let hover_ink = theme.foreground;
    let circle = theme
        .foreground
        .opacity(if theme.is_dark() { 0.065 } else { 0.04 });
    let hover_circle = theme
        .foreground
        .opacity(if theme.is_dark() { 0.14 } else { 0.10 });
    let pressed_circle = theme.foreground.opacity(0.18);
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(ink)
                .hover(theme.transparent)
                .active(theme.transparent),
        )
        .small()
        .size(px(TAB_CLOSE_SIZE))
        .w(px(TAB_CLOSE_SIZE))
        .h(px(TAB_CLOSE_SIZE))
        .p_0()
        .bg(theme.transparent)
        .rounded(px(TAB_CLOSE_SIZE * 0.5))
        .tab_stop(false)
        .accessibility_label("Close tab")
        .tooltip("Close tab")
        // Occluding the tab's hover hitbox makes this hover-only control
        // disappear as the pointer enters it. Consume mouse-down instead.
        // The containing tab still occludes native titlebar dragging.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .id("close-circle")
                .debug_selector(|| "tab-close-circle".into())
                .flex()
                .items_center()
                .justify_center()
                .size(px(TAB_CLOSE_CIRCLE_SIZE))
                .flex_shrink_0()
                .rounded(px(TAB_CLOSE_CIRCLE_SIZE * 0.5))
                .bg(circle)
                .group_hover("tab-close-control", move |s| s.bg(hover_circle))
                .group_active("tab-close-control", move |s| s.bg(pressed_circle))
                .child(
                    svg()
                        .debug_selector(|| "tab-close-glyph".into())
                        .path("phosphor/x-thin.svg")
                        .size(titlebar_icon_size(theme, 9.0))
                        .flex_shrink_0()
                        .text_color(ink)
                        .group_hover("tab-close-control", move |s| s.text_color(hover_ink)),
                ),
        )
}

// Resolve the small selection surface against chrome, not the unknown desktop
// behind the window. The tab hit target itself remains transparent.
fn tab_selected_surface(backdrop: Hsla, foreground: Hsla, is_dark: bool) -> Hsla {
    let backdrop = backdrop.alpha(1.0);
    let surface = if is_dark {
        backdrop.blend(foreground.opacity(0.22))
    } else {
        Hsla {
            l: (backdrop.l + 0.04).min(1.0),
            ..backdrop
        }
    };
    surface.opacity(0.96)
}

fn tab_resting_surface(foreground: Hsla, is_dark: bool) -> Hsla {
    foreground.opacity(if is_dark { 0.045 } else { 0.035 })
}

fn tab_hover_surface(foreground: Hsla, is_dark: bool) -> Hsla {
    foreground.opacity(if is_dark { 0.075 } else { 0.06 })
}

fn chrome_controls_row(top_bar_height: f32) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(3.0))
        .mb(px((top_bar_height - CHROME_CONTROL_SIZE).max(0.0) * 0.5))
        .flex_shrink_0()
}

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

fn chrome_button(
    id: &'static str,
    icon: &'static str,
    toggled: Option<bool>,
    label: &'static str,
    action: Option<Box<dyn Action>>,
    cx: &App,
) -> Button {
    chrome_control(id, icon, toggled, label, action, cx)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

// Dropdown triggers must bubble mouse-down to the component's popover wrapper.
fn chrome_control(
    id: &'static str,
    icon: &'static str,
    toggled: Option<bool>,
    label: &'static str,
    action: Option<Box<dyn Action>>,
    cx: &App,
) -> Button {
    let theme = cx.theme();
    let active = toggled == Some(true);
    let ink = if active {
        theme.foreground
    } else {
        theme.foreground.blend(theme.background.opacity(0.28))
    };
    let surface = theme.foreground;
    let selected_fill = if theme.is_dark() { 0.12 } else { 0.085 };
    let hover_fill = if active {
        if theme.is_dark() { 0.17 } else { 0.13 }
    } else if theme.is_dark() {
        0.09
    } else {
        0.06
    };
    let mut button = Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .foreground(ink)
                .hover(surface.opacity(hover_fill))
                .active(surface.opacity(if theme.is_dark() { 0.20 } else { 0.16 })),
        )
        .small()
        .size(px(CHROME_CONTROL_SIZE))
        .p_0()
        .bg(surface.opacity(if active { selected_fill } else { 0.0 }))
        .rounded(px(6.0))
        .accessibility_label(label)
        .when_some(toggled, |button, toggled| button.toggled(toggled))
        // Keep title-bar dragging from swallowing controls on Windows, and
        // preserve terminal focus on pointer activation on all platforms.
        .child(
            svg()
                .debug_selector(move || format!("{id}-icon"))
                .path(icon)
                .size(titlebar_icon_size(theme, 13.0))
                .flex_shrink_0()
                .text_color(ink),
        );
    button.interactivity().tooltip(move |window, cx| {
        chrome_tooltip(
            label,
            action
                .as_deref()
                .and_then(|action| crate::keycaps::first_action_keystroke(action, window)),
            window,
            cx,
        )
    });
    button
}

#[cfg(test)]
mod chrome_tests {
    use super::{
        chrome_button, chrome_controls_row, horizontal_tab_target, horizontal_tab_viewport,
        tab_attention_mark, tab_close_button, tab_close_slot, tab_drag_capsule,
        tab_identity_icon_size, tab_identity_icon_slot, tab_identity_row, tab_name_label,
        tab_selection_layer, tab_trailing_drop_target, tab_tray_container, tab_tray_layer,
    };
    use gpui::{
        Context, ElementId, FocusHandle, InteractiveElement, IntoElement, MouseButton,
        ParentElement, Render, StatefulInteractiveElement, Styled, Window, div, px,
    };
    use gpui_component::ActiveTheme;
    use gpui_component::ElementExt;
    use gpui_component::menu::DropdownMenu as _;

    #[test]
    fn overflow_navigation_assets_are_bundled() {
        use gpui::AssetSource;
        for path in ["phosphor/caret-left.svg", "phosphor/caret-right.svg"] {
            assert!(
                crate::assets::ConAssets.load(path).unwrap().is_some(),
                "{path}"
            );
        }
    }

    #[gpui::test]
    fn animated_slots_share_space_and_settle_to_equal_widths(cx: &mut gpui::TestAppContext) {
        use std::time::{Duration, Instant};
        struct AnimatedSlots {
            motion: super::super::super::tab_motion::TabSlotMotion,
            now: Instant,
            ids: Vec<u64>,
        }
        impl Render for AnimatedSlots {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.motion.sync(self.ids.iter().copied(), true, self.now);
                let (units, _) = self.motion.advance(self.now, false);
                div().w_full().h(px(40.0)).child(
                    super::horizontal_tab_viewport(self.ids.len())
                        .max_w(px(units * super::HORIZONTAL_TAB_MAX_WIDTH))
                        .child(
                            tab_tray_container()
                                .min_w(px(units * super::HORIZONTAL_TAB_MIN_WIDTH))
                                .children(self.ids.iter().copied().map(|id| {
                                    super::animated_tab_target(self.motion.weight(id, self.now))
                                        .id(("slot", id))
                                        .debug_selector(move || format!("slot-{id}").into())
                                        .child(div().min_w_0().truncate().child("Long tab title"))
                                })),
                        ),
                )
            }
        }
        let start = Instant::now();
        let (view, cx) = cx.add_window_view(|_, _| AnimatedSlots {
            motion: Default::default(),
            now: start,
            ids: vec![1, 2],
        });
        cx.simulate_resize(gpui::size(px(520.0), px(40.0)));
        cx.run_until_parked();
        let first = cx.debug_bounds("slot-1").unwrap();
        assert_eq!(first.size.width, px(200.0));
        view.update(cx, |view, cx| {
            view.ids.push(3);
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("slot-3").unwrap().size.width, px(0.0));
        assert_eq!(cx.debug_bounds("slot-1").unwrap(), first);
        view.update(cx, |view, cx| {
            view.now = start + Duration::from_millis(60);
            cx.notify();
        });
        cx.run_until_parked();
        let newcomer = cx.debug_bounds("slot-3").unwrap();
        assert!(newcomer.size.width > px(0.0) && newcomer.size.width < px(520.0 / 3.0));
        let first_mid = cx.debug_bounds("slot-1").unwrap();
        let second_mid = cx.debug_bounds("slot-2").unwrap();
        assert_eq!(first_mid.size.width, second_mid.size.width);
        assert_eq!(first_mid.right(), second_mid.left());
        assert_eq!(second_mid.right(), newcomer.left());
        view.update(cx, |view, cx| {
            view.now = start + Duration::from_millis(220);
            cx.notify();
        });
        cx.run_until_parked();
        let widths: Vec<_> = ["slot-1", "slot-2", "slot-3"]
            .into_iter()
            .map(|id| cx.debug_bounds(id).unwrap().size.width)
            .collect();
        assert!((widths[0] - widths[2]).abs() < px(0.1));
        assert!((widths.iter().copied().sum::<gpui::Pixels>() - px(520.0)).abs() < px(0.1));
    }

    #[gpui::test]
    fn overflow_edges_page_to_both_ends_and_disappear_when_tabs_fit(cx: &mut gpui::TestAppContext) {
        struct OverflowView {
            scroll: gpui::ScrollHandle,
            edges: std::rc::Rc<std::cell::Cell<(bool, bool)>>,
            count: usize,
        }
        impl Render for OverflowView {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let content = tab_tray_container()
                    .min_w(px(self.count as f32 * super::HORIZONTAL_TAB_MIN_WIDTH))
                    .children((0..self.count).map(|_| horizontal_tab_target()));
                super::horizontal_tab_strip(
                    self.count,
                    content.into_any_element(),
                    &self.scroll,
                    &self.edges,
                    cx.theme().background,
                    cx.theme(),
                    cx.entity_id(),
                    cx,
                )
            }
        }
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| OverflowView {
            scroll: gpui::ScrollHandle::new(),
            edges: Default::default(),
            count: 6,
        });
        cx.simulate_resize(gpui::size(px(320.0), px(40.0)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("tab-scroll-left").is_none());
        let initial_viewport = cx.debug_bounds("horizontal-tab-viewport").unwrap();
        let right = cx.debug_bounds("tab-scroll-right").unwrap();
        assert!(right.origin.x >= initial_viewport.right());
        for _ in 0..3 {
            let right = cx.debug_bounds("tab-scroll-right").unwrap();
            cx.simulate_click(right.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.refresh();
            });
            cx.run_until_parked();
        }
        assert!(cx.debug_bounds("tab-scroll-right").is_none());
        assert!(cx.debug_bounds("tab-scroll-left").is_some());
        let final_viewport = cx.debug_bounds("horizontal-tab-viewport").unwrap();
        assert_eq!(initial_viewport, final_viewport);
        let left = cx.debug_bounds("tab-scroll-left").unwrap();
        assert!(left.right() <= final_viewport.origin.x);
        for _ in 0..3 {
            let left = cx.debug_bounds("tab-scroll-left").unwrap();
            cx.simulate_click(left.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
                window.refresh();
            });
            cx.run_until_parked();
        }
        assert!(cx.debug_bounds("tab-scroll-left").is_none());
        view.update(cx, |view, cx| {
            view.count = 2;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("tab-scroll-left").is_none());
        assert!(cx.debug_bounds("tab-scroll-right").is_none());
    }

    #[gpui::test]
    fn tab_menu_trigger_bubbles_mouse_down_to_open_popover(cx: &mut gpui::TestAppContext) {
        struct MenuProbe(std::rc::Rc<std::cell::Cell<bool>>);
        impl Render for MenuProbe {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let opened = self.0.clone();
                div().size_full().child(
                    div().occlude().child(
                        super::chrome_control(
                            "probe-menu",
                            "phosphor/caret-down.svg",
                            None,
                            "All tabs",
                            None,
                            cx,
                        )
                        .debug_selector(|| "probe-menu".into())
                        .dropdown_menu(|menu, _, _| menu)
                        .on_open_change(move |open, _, _| opened.set(*open)),
                    ),
                )
            }
        }
        cx.update(gpui_component::init);
        let opened = std::rc::Rc::new(std::cell::Cell::new(false));
        let (_, view) = cx.add_window_view(|_, _| MenuProbe(opened.clone()));
        let button = view.debug_bounds("probe-menu").unwrap();
        view.simulate_click(button.center(), gpui::Modifiers::default());
        assert!(
            opened.get(),
            "the dropdown must receive the button's mouse-down"
        );
    }

    #[test]
    fn generic_tab_identities_use_badges_without_replacing_application_brands() {
        for (icon, badge) in [
            ("phosphor/terminal.svg", "tab-terminal-refined.png"),
            ("phosphor/code.svg", "tab-code.png"),
            ("phosphor/file-code.svg", "tab-file.png"),
            ("phosphor/globe.svg", "tab-remote.png"),
            ("phosphor/pulse.svg", "tab-monitor.png"),
            ("phosphor/book-open.svg", "tab-reader.png"),
        ] {
            assert_eq!(super::horizontal_tab_icon(icon), badge);
        }
        for icon in [
            "agents/opencode.svg",
            "providers/openai.svg",
            "phosphor/rocket.svg",
        ] {
            assert_eq!(super::horizontal_tab_icon(icon), icon);
        }
    }

    struct ChromeTestView {
        height: f32,
        focus: FocusHandle,
        clicks: usize,
        parent_mouse_downs: usize,
    }

    struct TabLayoutView {
        width: f32,
        renamed: bool,
        active: usize,
        scroll: gpui::ScrollHandle,
        reveal: super::HorizontalTabRevealState,
    }

    impl TabLayoutView {
        fn new(width: f32) -> Self {
            Self {
                width,
                renamed: false,
                active: 0,
                scroll: gpui::ScrollHandle::new(),
                reveal: Default::default(),
            }
        }
    }

    struct DragCapsuleView {
        width: f32,
    }

    struct CaptionGapView {
        width: f32,
        count: usize,
        caption_clicks: usize,
    }

    impl Render for CaptionGapView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("caption-probe")
                .flex()
                .w(px(self.width))
                .h(px(super::TOP_BAR_TABS_HEIGHT))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        this.caption_clicks += 1;
                    }),
                )
                .child(
                    div().flex().flex_1().min_w_0().child(
                        horizontal_tab_viewport(self.count)
                            .id("caption-tabs")
                            .debug_selector(|| "caption-tabs".into())
                            .overflow_x_scroll()
                            .mr(px(super::TAB_WINDOW_DRAG_GAP))
                            .child(
                                tab_tray_container()
                                    .min_w(px(self.count as f32 * super::HORIZONTAL_TAB_MIN_WIDTH))
                                    .children((0..self.count).map(|index| {
                                        horizontal_tab_target()
                                            .id(ElementId::Name(
                                                format!("caption-tab-{index}").into(),
                                            ))
                                            .debug_selector(move || format!("caption-tab-{index}"))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                    })),
                            ),
                    ),
                )
                .child(
                    div()
                        .debug_selector(|| "caption-controls".into())
                        .size(px(22.0))
                        .flex_shrink_0(),
                )
        }
    }

    #[gpui::test]
    fn capped_tabs_leave_a_caption_gap_without_losing_equal_widths(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| CaptionGapView {
            width: 2000.0,
            count: 2,
            caption_clicks: 0,
        });
        for count in [2, 3, 10] {
            for width in [2000.0, 900.0, 320.0] {
                view.update(cx, |view, cx| {
                    view.count = count;
                    view.width = width;
                    cx.notify();
                });
                cx.simulate_resize(gpui::size(px(width), px(100.0)));
                let viewport = cx.debug_bounds("caption-tabs").unwrap();
                let controls = cx.debug_bounds("caption-controls").unwrap();
                let first = cx.debug_bounds("caption-tab-0").unwrap();
                assert!(controls.left() - viewport.right() >= px(super::TAB_WINDOW_DRAG_GAP));
                assert!(first.size.width >= px(super::HORIZONTAL_TAB_MIN_WIDTH));
                assert!(first.size.width <= px(super::HORIZONTAL_TAB_MAX_WIDTH));
                for selector in [
                    "caption-tab-1",
                    "caption-tab-2",
                    "caption-tab-3",
                    "caption-tab-4",
                    "caption-tab-5",
                    "caption-tab-6",
                    "caption-tab-7",
                    "caption-tab-8",
                    "caption-tab-9",
                ]
                .into_iter()
                .take(count - 1)
                {
                    let tab = cx.debug_bounds(selector).unwrap();
                    // GPUI distributes fractional widths on the device pixel grid.
                    assert!(
                        (tab.size.width - first.size.width).abs() <= px(1.0),
                        "unequal allocation for {count} tabs at {width}: {first:?}, {tab:?}"
                    );
                }
                let gap = gpui::point(viewport.right() + px(20.0), viewport.center().y);
                cx.simulate_click(gap, gpui::Modifiers::default());
                cx.update(|_, cx| assert_eq!(view.read(cx).caption_clicks, 1));
                cx.simulate_click(first.center(), gpui::Modifiers::default());
                cx.update(|_, cx| assert_eq!(view.read(cx).caption_clicks, 1));
                view.update(cx, |view, _| view.caption_clicks = 0);
            }
        }
    }

    impl Render for DragCapsuleView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            tab_drag_capsule(
                &super::TabDragPreviewState {
                    title: "a-long-project-name-that-must-truncate".into(),
                    icon: "phosphor/terminal.svg",
                    source_top: px(0.0),
                    cursor_offset_x: px(0.0),
                    size: gpui::size(px(self.width), px(super::TAB_SURFACE_HEIGHT)),
                    source_visual_index: 0,
                },
                cx.theme(),
            )
            .debug_selector(|| "drag-capsule".into())
        }
    }

    #[gpui::test]
    fn drag_capsule_keeps_source_width_and_centred_identity(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| DragCapsuleView { width: 180.0 });
        for width in [136.0, 180.0, 350.0] {
            for font_size in [16.0, 24.0] {
                view.update(cx, |view, cx| {
                    view.width = width;
                    cx.global_mut::<gpui_component::Theme>().font_size = px(font_size);
                    cx.notify();
                });
                let capsule = cx.debug_bounds("drag-capsule").unwrap();
                let icon = cx.debug_bounds("drag-capsule-icon").unwrap();
                let label = cx.debug_bounds("drag-capsule-label").unwrap();
                assert_eq!(
                    capsule.size,
                    gpui::size(px(width), px(super::TAB_SURFACE_HEIGHT))
                );
                assert_eq!(icon.center().y, capsule.center().y);
                assert_eq!(label.left() - capsule.left(), px(super::TAB_CONTENT_INSET));
                assert_eq!(
                    capsule.right() - label.right(),
                    px(super::TAB_TITLE_TRAILING_INSET)
                );
                assert_eq!(
                    icon.center().x - capsule.left(),
                    px(super::TAB_IDENTITY_INSET + super::TAB_CLOSE_SIZE * 0.5)
                );
                assert!(icon.left() >= capsule.left());
                assert!(icon.right() <= capsule.right());
                assert_eq!(
                    label.left() - icon.center().x,
                    px(super::TAB_CLOSE_SIZE * 0.5 + super::TAB_IDENTITY_LABEL_GAP)
                );
            }
        }
    }

    struct HoverCloseView {
        clicks: usize,
        activations: usize,
    }

    impl Render for HoverCloseView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            horizontal_tab_target()
                .id("hover-tab")
                .text_size(px(12.0))
                .group("tab")
                .occlude()
                .debug_selector(|| "hover-tab".into())
                .on_click(cx.listener(|this, _, _, _| this.activations += 1))
                .child(
                    tab_identity_row()
                        .relative()
                        .child(
                            tab_selection_layer(
                                gpui::transparent_black(),
                                gpui::transparent_black(),
                            )
                            .debug_selector(|| "hover-surface".into()),
                        )
                        .child(
                            tab_attention_mark(cx.theme().primary)
                                .debug_selector(|| "hover-attention".into()),
                        )
                        .child(
                            tab_identity_icon_slot().child(
                                gpui::svg()
                                    .debug_selector(|| "hover-identity".into())
                                    .path("phosphor/terminal.svg")
                                    .size(tab_identity_icon_size(cx.theme()))
                                    .text_color(cx.theme().foreground),
                            ),
                        )
                        .child(
                            tab_close_slot()
                                .id("hover-slot")
                                .group("tab-close-control")
                                .invisible()
                                .group_hover("tab", |s| s.visible())
                                .child(
                                    tab_close_button("hover-close".into(), cx)
                                        .debug_selector(|| "hover-close".into())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.clicks += 1;
                                        })),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(|| "hover-label".into())
                                .child("Example"),
                        ),
                )
        }
    }

    #[gpui::test]
    fn hovering_a_tab_reveals_a_clickable_close_control(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| HoverCloseView {
            clicks: 0,
            activations: 0,
        });
        let target = cx.debug_bounds("hover-tab").unwrap();
        let label_before = cx.debug_bounds("hover-label").unwrap();
        let identity_before = cx.debug_bounds("hover-identity").unwrap();
        cx.simulate_mouse_move(target.center(), None, gpui::Modifiers::default());
        let close = cx.debug_bounds("hover-close").unwrap();
        let circle = cx.debug_bounds("tab-close-circle").unwrap();
        let attention = cx.debug_bounds("hover-attention").unwrap();
        let surface = cx.debug_bounds("hover-surface").unwrap();
        let clearance = (surface.size.height - circle.size.height) * 0.5;
        assert_eq!(surface.right() - circle.right(), clearance);
        assert_eq!(circle.top() - surface.top(), clearance);
        assert_eq!(surface.bottom() - circle.bottom(), clearance);
        assert_eq!(circle.center().y, target.center().y);
        assert!(identity_before.right() < close.left());
        assert_eq!(identity_before, cx.debug_bounds("hover-identity").unwrap());
        assert!(close.left() >= target.left());
        assert!(close.right() <= target.right());
        assert!(label_before.right() <= close.left());
        assert!(attention.right() < label_before.left());
        assert!(attention.right() < close.left());
        assert_eq!(label_before, cx.debug_bounds("hover-label").unwrap());
        cx.simulate_click(close.center(), gpui::Modifiers::default());
        cx.update(|_, cx| {
            assert_eq!(view.read(cx).clicks, 1);
            assert_eq!(view.read(cx).activations, 0);
        });
        cx.simulate_click(label_before.center(), gpui::Modifiers::default());
        cx.update(|_, cx| assert_eq!(view.read(cx).activations, 1));
    }

    impl Render for TabLayoutView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("layout-root")
                .debug_selector(|| "layout-root".into())
                .flex()
                .w(px(self.width))
                .h(px(super::TOP_BAR_TABS_HEIGHT))
                .items_end()
                .child(
                    horizontal_tab_viewport(2)
                        .id("layout-viewport")
                        .overflow_x_scroll()
                        .debug_selector(|| "layout-viewport".into())
                        .track_scroll(&self.scroll)
                        .child(
                            tab_tray_container()
                                .min_w(px(2.0 * super::HORIZONTAL_TAB_MIN_WIDTH))
                                .child(
                                    tab_tray_layer(gpui::transparent_black())
                                        .debug_selector(|| "tray-paint".into()),
                                )
                                .children(
                                    [
                                        ("short-tab", "~"),
                                        (
                                            "long-tab",
                                            if self.renamed {
                                                "Mem"
                                            } else {
                                                "a-long-project-name-with-a-long-working-directory"
                                            },
                                        ),
                                    ]
                                    .map(|(id, title)| {
                                        let index = usize::from(id == "long-tab");
                                        let is_active = self.active == index;
                                        let scroll = self.scroll.clone();
                                        let wheel_scroll = scroll.clone();
                                        let reveal = self.reveal.clone();
                                        horizontal_tab_target()
                                            .id(id)
                                            .text_size(px(12.0))
                                            .debug_selector(move || id.into())
                                            .occlude()
                                            .on_scroll_wheel(move |event, window, cx| {
                                                super::scroll_occluded_tab(
                                                    event,
                                                    &wheel_scroll,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .on_prepaint(move |bounds, window, _| {
                                                if is_active {
                                                    super::schedule_tab_reveal(
                                                        (index as u64, index, 2),
                                                        bounds,
                                                        &scroll,
                                                        &reveal,
                                                        window,
                                                    );
                                                }
                                            })
                                            .child(
                                                tab_identity_row()
                                                    .relative()
                                                    .child(
                                                        tab_selection_layer(
                                                            gpui::transparent_black(),
                                                            gpui::transparent_black(),
                                                        )
                                                        .debug_selector(move || {
                                                            format!("{id}-surface")
                                                        }),
                                                    )
                                                    .child(tab_identity_icon_slot().debug_selector(
                                                        move || format!("{id}-identity"),
                                                    ))
                                                    .child(
                                                        tab_name_label()
                                                            .debug_selector(move || {
                                                                format!("{id}-label")
                                                            })
                                                            .child(
                                                                div()
                                                                    .min_w_0()
                                                                    .overflow_x_hidden()
                                                                    .whitespace_nowrap()
                                                                    .text_ellipsis()
                                                                    .child(title),
                                                            ),
                                                    ),
                                            )
                                    }),
                                )
                                .child(
                                    tab_trailing_drop_target()
                                        .debug_selector(|| "trailing-drop".into()),
                                ),
                        ),
                )
        }
    }

    #[gpui::test]
    fn tabs_share_available_width_independent_of_title_length(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| TabLayoutView::new(700.0));
        let short = cx.debug_bounds("short-tab").unwrap();
        let long = cx.debug_bounds("long-tab").unwrap();
        assert_eq!(short.size.width, long.size.width);
        assert_eq!(short.size.width, px(super::HORIZONTAL_TAB_MAX_WIDTH));
        assert_eq!(short.size.width + long.size.width, px(400.0));
        assert_eq!(
            cx.debug_bounds("layout-root").unwrap().right() - long.right(),
            px(300.0)
        );
        let identity_before = cx.debug_bounds("long-tab-identity").unwrap();
        let label_before = cx.debug_bounds("long-tab-label").unwrap();
        view.update(cx, |view, cx| {
            view.renamed = true;
            cx.notify();
        });
        assert_eq!(cx.debug_bounds("short-tab").unwrap(), short);
        assert_eq!(cx.debug_bounds("long-tab").unwrap(), long);
        assert_eq!(
            cx.debug_bounds("long-tab-identity").unwrap(),
            identity_before
        );
        assert_eq!(cx.debug_bounds("long-tab-label").unwrap(), label_before);
        view.update(cx, |view, cx| {
            view.width = 320.0;
            cx.notify();
        });
        let root = cx.debug_bounds("layout-root").unwrap();
        let short = cx.debug_bounds("short-tab").unwrap();
        let long = cx.debug_bounds("long-tab").unwrap();
        assert_eq!(short.size.width, long.size.width);
        assert!(short.right() <= long.left());
        assert!(long.right() <= root.right());
        view.update(cx, |view, cx| {
            view.width = 120.0;
            cx.notify();
        });
        let short = cx.debug_bounds("short-tab").unwrap();
        let long = cx.debug_bounds("long-tab").unwrap();
        assert_eq!(short.size.width, px(super::HORIZONTAL_TAB_MIN_WIDTH));
        assert_eq!(long.size.width, short.size.width);
        assert!(long.right() > cx.debug_bounds("layout-root").unwrap().right());
    }

    #[gpui::test]
    fn tab_overflow_scrolls_and_repaints_preserve_manual_position(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| TabLayoutView::new(160.0));
        let root = cx.debug_bounds("layout-root").unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: root.center(),
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -4.0)),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        let offset = cx.update(|_, cx| view.read(cx).scroll.offset());
        assert!(offset.x < px(0.0));
        view.update(cx, |view, cx| {
            view.renamed = true;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.update(|_, cx| view.read(cx).scroll.offset()), offset);
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: root.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(30.0))),
            modifiers: Default::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        assert!(cx.update(|_, cx| view.read(cx).scroll.offset().x) > offset.x);
        view.update(cx, |view, cx| {
            view.active = 1;
            view.reveal.set(None);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        let last = cx.debug_bounds("long-tab").unwrap();
        assert!(last.left() >= root.left());
        assert!(last.right() <= root.right());
        view.update(cx, |view, cx| {
            view.active = 0;
            view.reveal.set(None);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        let first = cx.debug_bounds("short-tab").unwrap();
        assert!(first.left() >= root.left());
        assert!(first.right() <= root.right());
    }

    #[gpui::test]
    fn oversized_tabs_reveal_their_leading_identity(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| TabLayoutView::new(100.0));
        view.update(cx, |view, cx| {
            view.active = 1;
            view.reveal.set(None);
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        assert_eq!(
            cx.debug_bounds("long-tab").unwrap().left(),
            cx.debug_bounds("layout-root").unwrap().left()
        );
    }

    #[gpui::test]
    fn tab_reveal_discards_queued_requests_for_older_selections(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| TabLayoutView::new(160.0));
        let scroll = cx.update(|_, cx| view.read(cx).scroll.clone());
        let state: super::HorizontalTabRevealState = Default::default();
        let last = cx.debug_bounds("long-tab").unwrap();
        cx.update(|window, _| super::schedule_tab_reveal((1, 1, 2), last, &scroll, &state, window));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        assert!(scroll.offset().x < px(0.0));
        let offset = scroll.offset();
        let first = cx.debug_bounds("short-tab").unwrap();
        let last = cx.debug_bounds("long-tab").unwrap();
        cx.update(|window, _| {
            super::schedule_tab_reveal((0, 0, 2), first, &scroll, &state, window);
            super::schedule_tab_reveal((1, 1, 2), last, &scroll, &state, window);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        assert_eq!(scroll.offset(), offset);
        cx.update(|window, _| {
            super::schedule_tab_reveal((0, 0, 2), first, &scroll, &state, window);
            state.set(None);
        });
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.refresh();
        });
        assert_eq!(scroll.offset(), offset);
    }

    #[gpui::test]
    fn selected_tab_uses_equal_tray_insets_at_both_ends(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| TabLayoutView::new(700.0));
        for width in [700.0, 180.0, 420.0] {
            view.update(cx, |view, cx| {
                view.width = width;
                cx.notify();
            });
            let root = cx.debug_bounds("layout-root").unwrap();
            let tray = cx.debug_bounds("tray-paint").unwrap();
            let first = cx.debug_bounds("short-tab-surface").unwrap();
            let last = cx.debug_bounds("long-tab-surface").unwrap();
            let inset = px(super::TAB_SELECTION_INSET);
            let drop_target = cx.debug_bounds("trailing-drop").unwrap();
            assert_eq!(drop_target.left(), tray.right());
            assert_eq!(drop_target.size.width, px(super::TAB_TRAILING_DROP_WIDTH));
            assert_eq!(tray.top() - root.top(), root.bottom() - tray.bottom());
            assert_eq!(first.left() - tray.left(), tray.right() - last.right());
            for surface in [first, last] {
                assert_eq!(surface.top() - tray.top(), inset);
                assert_eq!(tray.bottom() - surface.bottom(), inset);
            }
            for (target, surface, identity_selector, label_selector) in [
                ("short-tab", first, "short-tab-identity", "short-tab-label"),
                ("long-tab", last, "long-tab-identity", "long-tab-label"),
            ] {
                let identity = cx.debug_bounds(identity_selector).unwrap();
                let label = cx.debug_bounds(label_selector).unwrap();
                assert_eq!(
                    label.left() - identity.right(),
                    px(super::TAB_IDENTITY_LABEL_GAP)
                );
                assert_eq!(label.left() - surface.left(), px(super::TAB_CONTENT_INSET));
                assert_eq!(
                    surface.right() - label.right(),
                    px(super::TAB_TITLE_TRAILING_INSET)
                );
                assert_eq!(
                    identity.left() - surface.left(),
                    px(super::TAB_IDENTITY_INSET)
                );
                let target = cx.debug_bounds(target).unwrap();
                assert_eq!(surface.center().x, target.center().x);
                assert_eq!(surface.size.width, target.size.width - inset * 2.0);
            }
        }
    }

    impl Render for ChromeTestView {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("chrome-test-parent")
                .debug_selector(|| "chrome-test-parent".into())
                .flex()
                .items_end()
                .w(px(160.0))
                .h(px(self.height))
                .track_focus(&self.focus)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| this.parent_mouse_downs += 1),
                )
                .child(
                    chrome_controls_row(self.height)
                        .children([false, true].map(|active| {
                            chrome_button(
                                if active { "chrome-on" } else { "chrome-off" },
                                "phosphor/sidebar.svg",
                                Some(active),
                                "Toggle sidebar",
                                None,
                                cx,
                            )
                            .debug_selector(move || {
                                if active { "chrome-on" } else { "chrome-off" }.into()
                            })
                            .on_click(cx.listener(|this, _, _, _| this.clicks += 1))
                        }))
                        .child(
                            tab_close_button(ElementId::Name("chrome-close".into()), cx)
                                .debug_selector(|| "chrome-close".into())
                                .on_click(cx.listener(|this, _, _, _| this.clicks += 1)),
                        ),
                )
        }
    }

    #[gpui::test]
    fn chrome_controls_center_and_preserve_focus_when_clicked(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let focus = cx.focus_handle();
            focus.focus(window, cx);
            ChromeTestView {
                height: 28.0,
                focus,
                clicks: 0,
                parent_mouse_downs: 0,
            }
        });
        for height in [28.0, 36.0] {
            for font_size in [16.0, 24.0] {
                view.update(cx, |view, cx| {
                    view.height = height;
                    cx.global_mut::<gpui_component::Theme>().font_size = px(font_size);
                    cx.notify();
                });
                let parent = cx.debug_bounds("chrome-test-parent").unwrap();
                for (selector, size) in [
                    ("chrome-off", 22.0),
                    ("chrome-on", 22.0),
                    ("chrome-close", 20.0),
                ] {
                    let button = cx.debug_bounds(selector).unwrap();
                    assert_eq!(button.size, gpui::size(px(size), px(size)));
                    assert_eq!(
                        button.top() - parent.top(),
                        parent.bottom() - button.bottom()
                    );
                    let glyph = cx
                        .debug_bounds(if selector == "chrome-close" {
                            "tab-close-glyph"
                        } else if selector == "chrome-on" {
                            "chrome-on-icon"
                        } else {
                            "chrome-off-icon"
                        })
                        .unwrap();
                    assert_eq!(glyph.size.width, glyph.size.height);
                    assert!(
                        (button.center().x - glyph.center().x).abs() < px(0.01),
                        "{selector}, font={font_size}, height={height}: button={button:?}, glyph={glyph:?}"
                    );
                    assert!(
                        (button.center().y - glyph.center().y).abs() < px(0.01),
                        "{selector}, font={font_size}, height={height}: button={button:?}, glyph={glyph:?}"
                    );
                    if selector == "chrome-close" {
                        let circle = cx.debug_bounds("tab-close-circle").unwrap();
                        assert_eq!(circle.size, gpui::size(px(14.0), px(14.0)));
                        assert_eq!(circle.center(), button.center());
                        assert_eq!(
                            circle.top() - button.top(),
                            button.bottom() - circle.bottom()
                        );
                    }
                    cx.simulate_click(button.center(), gpui::Modifiers::default());
                    cx.update(|window, cx| {
                        let state = view.read(cx);
                        assert!(state.focus.is_focused(window));
                        assert_eq!(state.parent_mouse_downs, 0);
                    });
                }
            }
        }
        cx.update(|_, cx| assert_eq!(view.read(cx).clicks, 12));
    }
}

impl ConWorkspace {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn render_top_bar(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        top_bar_height: f32,
        tab_strip_progress: f32,
        top_bar_surface_color: Hsla,
    ) -> impl IntoElement + use<> {
        self.tab_activity.read(cx).clear();
        let theme = cx.theme();
        // Solid ink keeps small labels crisp over translucent window chrome.
        let tab_secondary_ink = theme.foreground.blend(theme.background.opacity(0.20));
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
            .bg(if cfg!(target_os = "macos") {
                theme.transparent
            } else {
                top_bar_surface_color
            });

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
        let pane_title_drag_to_tab_active = pane_drag_needs_horizontal_tabs(
            cfg!(target_os = "macos") && self.vertical_tabs_enabled(),
            pane_title_drag_to_tab_active(self.pane_title_drag.as_ref()),
        );
        // Pane-origin drags keep `pane_title_drag` alive from their
        // drag-move handlers. Use that explicit state to keep the tab strip
        // visible instead of inferring pane drags from `cx.has_active_drag()`.
        let pane_origin_drag_active = pane_title_drag_to_tab_active;
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
        let caption_docked = {
            #[cfg(target_os = "macos")]
            {
                self.caption_dock.dock > 0.0
            }
            #[cfg(not(target_os = "macos"))]
            {
                false
            }
        };
        let show_horizontal_tabs = !caption_docked
            && (self.horizontal_tabs_visible()
                || pane_title_drag_to_tab_active
                || pane_origin_drag_active);
        let tab_count = self.tabs.len();
        let motion_now = std::time::Instant::now();
        let animate_slots = self.config.appearance.tabs_orientation == TabsOrientation::Horizontal
            && tab_count > 1
            && !tab_strip_preview_active
            && !cx.has_active_drag()
            && self.tab_rename.is_none()
            && !cx.reduce_motion();
        self.tab_slot_motion.sync(
            self.tabs.iter().map(|tab| tab.summary_id),
            animate_slots,
            motion_now,
        );
        let (mut slot_units, slots_animating) =
            self.tab_slot_motion.advance(motion_now, cx.reduce_motion());
        if slots_animating {
            window.request_animation_frame();
        }
        let mut rendered_tab_count = tab_count;
        if tab_strip_preview_active || pane_origin_drag_active {
            self.horizontal_tab_reveal_key.set(None);
        }
        let tab_strip_drop_slot = self.tab_strip_drop_slot;
        // Snapshot rename state for this render frame.
        let renaming_tab_index = self.tab_rename.as_ref().map(|r| r.tab_index);
        let rename_input = self.tab_rename.as_ref().map(|r| r.input.clone());
        let mut tabs_container = tab_tray_container()
            .id("tab-strip-container")
            .min_w(px(slot_units * HORIZONTAL_TAB_MIN_WIDTH))
            .when(
                !self.horizontal_tab_edges.get().0 && !self.horizontal_tab_edges.get().1,
                |tray| {
                    tray.child(tab_tray_layer(tab_resting_surface(
                        theme.foreground,
                        theme.is_dark(),
                    )))
                },
            )
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
                if dragged.origin == DraggedTabOrigin::Pane
                    && let Some(pane_id) = dragged.pane_id
                {
                    this.detach_pane_to_new_tab_at_slot(pane_id, to, window, cx);
                    return;
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

            slot_units += usize::from(pane_title_drag_drop_slot.is_some()) as f32;
            tabs_container = tabs_container.min_w(px(slot_units * HORIZONTAL_TAB_MIN_WIDTH));

            let render_indices: Vec<usize> = if let Some(dragged_id) = dragged_source_id
                && let Some(source_idx) = self.tabs.iter().position(|t| t.summary_id == dragged_id)
                && let Some(drop_slot) = self.tab_strip_drop_slot
            {
                // Build a reordered index list: remove source, insert at drop slot.
                // This gives Chrome-like live reordering during GPUI tab drag.
                let insert_at = tab_reorder_destination(source_idx, drop_slot, self.tabs.len());
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
                for exit in &self.tab_slot_motion.departing {
                    if self.tab_slot_motion.before(exit) == Some(session_id) {
                        tabs_container = tabs_container.child(departing_tab(exit, theme));
                    }
                }
                let tab_id = session_id;
                let tab_color = tab.color;
                let terminal_status = self
                    .terminal_presentation
                    .tabs
                    .get(&session_id)
                    .copied()
                    .flatten();
                let is_dragged_source = is_dragged_tab_source(dragged_source_id, session_id);
                // Sidebar and strip consume the same retained names and icons.
                // Paint-only ticks must not query terminal/runtime metadata.
                let presentation = self.sidebar.read(cx).session(index);
                let tab_icon = horizontal_tab_icon(
                    presentation.map_or("phosphor/terminal.svg", |entry| entry.icon),
                );
                let name = presentation.map_or(tab.title.as_str(), |entry| entry.name.as_str());
                let status_detail = self.program_status_detail(tab.summary_id);

                let display_title = name.to_owned();
                let label_detail = tab_label_detail(
                    name,
                    presentation.and_then(|entry| entry.subtitle.as_deref()),
                    status_detail.as_deref(),
                );

                let close_id = ElementId::Name(format!("tab-close-{}", session_id).into());

                let close_button = tab_close_slot()
                    .id(ElementId::Name(
                        format!("tab-close-slot-{session_id}").into(),
                    ))
                    .group("tab-close-control")
                    .invisible()
                    .group_hover("tab", |s| s.visible())
                    .child(tab_close_button(close_id, cx).on_click(cx.listener(
                        move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_tab_by_id(tab_id, window, cx);
                        },
                    )));

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
                        top: px(TAB_DROP_PREVIEW_TOP),
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
                let tab_drag_source_bounds = self.tab_strip_tab_bounds.clone();
                let wheel_scroll = self.horizontal_tab_scroll.clone();

                let mut tab_el =
                    animated_tab_target(self.tab_slot_motion.weight(session_id, motion_now))
                        .id(("tab", session_id))
                        .group("tab")
                        .text_size(ui_px(theme, 11.5))
                        .cursor_pointer()
                        // Windows: without `.occlude()` the parent top_bar's
                        // `WindowControlArea::Drag` hit-test routes the click
                        // to the OS (HTCAPTION) and starts a window drag
                        // before GPUI fires `on_click` — so the tab never
                        // activates. Same treatment as the `+`, caption
                        // buttons, and tab-close controls in this file.
                        .occlude()
                        // Caption hit-test occlusion also blocks the ancestor's scroll listener.
                        .on_scroll_wheel(move |event, window, cx| {
                            scroll_occluded_tab(event, &wheel_scroll, window, cx);
                        })
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
                                if dragged.origin == DraggedTabOrigin::HorizontalTabStrip
                                    && let Ok(mut guard) = active_dragged_tab_session_id.lock()
                                {
                                    *guard = Some(dragged.session_id);
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
                                        let target_width = tab_drag_source_bounds
                                            .lock()
                                            .ok()
                                            .and_then(|bounds| bounds.get(render_pos).copied())
                                            .map(|bounds| bounds.size.width)
                                            .unwrap_or(
                                                preview_size.width + px(2.0 * TAB_SELECTION_INSET),
                                            );
                                        let capsule_width = (target_width
                                            - px(2.0 * TAB_SELECTION_INSET))
                                        .max(px(TAB_CLOSE_SIZE));
                                        *guard = Some(TabDragPreviewState {
                                            title: dragged.label.clone(),
                                            icon: dragged.icon,
                                            source_top: c.top + px(TAB_SELECTION_INSET),
                                            cursor_offset_x: offset.x
                                                - (target_width - capsule_width) * 0.5,
                                            source_visual_index: render_pos,
                                            size: size(capsule_width, px(TAB_SURFACE_HEIGHT)),
                                        });
                                    }
                                }
                                cx.new(|_| d)
                            },
                        )
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
                            if dragged.origin == DraggedTabOrigin::Pane
                                && let Some(pane_id) = dragged.pane_id
                            {
                                this.detach_pane_to_new_tab_at_slot(pane_id, to, window, cx);
                                return;
                            }
                            this.reorder_tab_by_id(dragged.session_id, to, cx);
                            cx.notify();
                        }));

                tab_el = tab_el.tooltip(move |window, cx| {
                    Tooltip::new(label_detail.clone()).build(window, cx)
                });

                // Keep navigation geometry independent of selection and hover paint.
                let (capsule_bg, capsule_hover_bg) = if is_active {
                    let bg = match tab_color {
                        Some(color) => crate::tab_colors::tab_accent_surface_hsla(
                            color,
                            crate::tab_colors::TAB_ACCENT_ACTIVE_ALPHA,
                            cx,
                        ),
                        None => tab_selected_surface(
                            top_bar_surface_color,
                            theme.foreground,
                            theme.is_dark(),
                        ),
                    };
                    (bg, bg)
                } else {
                    let inactive_alpha = sanitize_tab_accent_alpha(self.tab_accent_inactive_alpha);
                    let inactive_hover_alpha =
                        sanitize_tab_accent_alpha(self.tab_accent_inactive_hover_alpha)
                            .max(inactive_alpha);
                    let inactive_bg = tab_color
                        .map(|color| {
                            crate::tab_colors::tab_accent_surface_hsla(color, inactive_alpha, cx)
                        })
                        .unwrap_or(theme.transparent);
                    let inactive_hover_bg = tab_color
                        .map(|color| {
                            crate::tab_colors::tab_accent_surface_hsla(
                                color,
                                inactive_hover_alpha,
                                cx,
                            )
                        })
                        .unwrap_or_else(|| tab_hover_surface(theme.foreground, theme.is_dark()));
                    (inactive_bg, inactive_hover_bg)
                };

                self.tab_slot_motion.remember_colors(
                    session_id,
                    capsule_bg,
                    if is_active {
                        theme.foreground
                    } else {
                        tab_secondary_ink
                    },
                );

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

                let mut tab_content = tab_identity_row()
                    .relative()
                    .child(tab_selection_layer(capsule_bg, capsule_hover_bg));

                let mut name_label = tab_name_label();

                if needs_attention {
                    tab_content = tab_content.child(tab_attention_mark(theme.primary));
                }

                tab_content =
                    tab_content.child(
                        tab_identity_icon_slot().child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .size(tab_identity_icon_size(theme))
                                .flex_shrink_0()
                                .child(self.tab_activity.read(cx).icon_group(
                                    presentation.map_or(std::slice::from_ref(&tab_icon), |entry| {
                                        entry.identity_icons()
                                    }),
                                    tab_identity_icon_size(theme),
                                    if tab_icon == "phosphor/terminal-window.svg" {
                                        tab_secondary_ink
                                    } else if is_active {
                                        tab_color
                                            .map(|color| {
                                                crate::tab_colors::tab_accent_color_hsla(color, cx)
                                            })
                                            .unwrap_or(theme.foreground)
                                    } else {
                                        tab_secondary_ink
                                    },
                                    terminal_status.filter(|_| {
                                        !is_dragged_source && tab_strip_progress > 0.01
                                    }),
                                    theme,
                                    false,
                                )),
                        ),
                    );
                tab_content = tab_content.child(close_button);
                name_label = name_label.child(
                    div()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_family(theme.font_family.clone())
                        .line_height(ui_px(theme, 16.0))
                        .font_weight(FontWeight::NORMAL)
                        // Mute with a solid theme color, not glyph alpha:
                        // the title bar itself is already translucent.
                        .text_color(if is_active {
                            theme.foreground
                        } else {
                            tab_secondary_ink
                        })
                        .child(display_title),
                );

                tab_content = tab_content.child(name_label);

                // When this tab is being renamed, replace the title text
                // with an inline Input. Escape cancels; Enter confirms
                // (handled in begin_tab_rename's subscribe_in).
                let is_renaming = renaming_tab_index == Some(index);
                if is_renaming && let Some(input) = rename_input.clone() {
                    // Renaming uses the same allocated width as the resting tab.
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
                let tab_drag_preview_for_prepaint = self.tab_drag_preview.clone();
                let pane_title_drag_tab_bounds_for_prepaint =
                    self.pane_title_drag_tab_bounds.clone();
                let reveal_active = is_active
                    && !slots_animating
                    && !cx.has_active_drag()
                    && !tab_strip_preview_active
                    && !pane_origin_drag_active;
                let reveal_state = self.horizontal_tab_reveal_key.clone();
                let tab_scroll = self.horizontal_tab_scroll.clone();
                tab_el = tab_el.on_prepaint(move |bounds, window, _| {
                    if reveal_active {
                        schedule_tab_reveal(
                            (session_id, index, tab_count),
                            bounds,
                            &tab_scroll,
                            &reveal_state,
                            window,
                        );
                    }
                    if is_dragged_source
                        && let Ok(mut preview) = tab_drag_preview_for_prepaint.lock()
                        && let Some(preview) = preview.as_mut()
                    {
                        preview.source_visual_index = tab_visual_pos;
                    }
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
                            .min_w(px(HORIZONTAL_TAB_MIN_WIDTH))
                            .items_center()
                            .px(px(10.0))
                            .h(px(TOP_BAR_TAB_HEIGHT))
                            .text_size(ui_px(theme, 12.0))
                            .font_family(theme.font_family.clone())
                            .rounded(px(6.0))
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
                                            .size(mono_icon_px(theme, 12.0))
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
                    tab_el
                        .font_family(theme.font_family.clone())
                        .child(tab_content)
                        .context_menu(move |menu: gpui_component::menu::PopupMenu, _window, _cx| {
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
                        })
                });
                visual_pos += 1;

                if index + 1 == tab_count {
                    tabs_container = tabs_container.child(
                        tab_trailing_drop_target()
                            .id(ElementId::Name(format!("tab-trailing-drop-{index}").into()))
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
                                if dragged.origin == DraggedTabOrigin::Pane
                                    && let Some(pane_id) = dragged.pane_id
                                {
                                    this.detach_pane_to_new_tab_at_slot(pane_id, to, window, cx);
                                    return;
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
                                .min_w(px(HORIZONTAL_TAB_MIN_WIDTH))
                                .items_center()
                                .px(px(10.0))
                                .h(px(TOP_BAR_TAB_HEIGHT))
                                .text_size(ui_px(theme, 12.0))
                                .font_family(theme.font_family.clone())
                                .rounded(px(6.0))
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
                                                .size(mono_icon_px(theme, 12.0))
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
            for exit in &self.tab_slot_motion.departing {
                if self.tab_slot_motion.before(exit).is_none() {
                    tabs_container = tabs_container.child(departing_tab(exit, theme));
                }
            }
            rendered_tab_count = visual_pos;
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
        if !caption_docked && (tab_strip_progress > 0.01 || pane_title_drag_to_tab_active) {
            let opacity = if pane_title_drag_to_tab_active && tab_strip_progress < 0.01 {
                1.0
            } else {
                tab_strip_progress
            };
            leading_chrome = leading_chrome.child(
                horizontal_tab_strip(
                    rendered_tab_count,
                    tabs_container.into_any_element(),
                    &self.horizontal_tab_scroll,
                    &self.horizontal_tab_edges,
                    top_bar_surface_color,
                    theme,
                    cx.entity_id(),
                    cx,
                )
                .max_w(px(slot_units * HORIZONTAL_TAB_MAX_WIDTH))
                // Surplus width stays on the parent caption, outside tab hit targets.
                .mr(px(TAB_WINDOW_DRAG_GAP))
                .opacity(opacity),
            );
        }

        top_bar = top_bar.child(leading_chrome);

        // Right-side controls — compact row
        let mut tab_controls = chrome_controls_row(top_bar_height)
            .ml(px(if show_horizontal_tabs { 8.0 } else { 0.0 }));
        if show_horizontal_tabs && tab_count > 1 {
            let workspace = cx.entity().downgrade();
            tab_controls = tab_controls.child(
                // Occlude the whole popover trigger, not its inner button: the
                // wrapper must receive mouse-down while the caption must not.
                div().occlude().child(
                    chrome_control(
                        "tab-list",
                        "phosphor/caret-down.svg",
                        None,
                        "All tabs",
                        None,
                        cx,
                    )
                    .debug_selector(|| "horizontal-tab-menu".into())
                    .dropdown_menu(move |menu, _, cx| {
                        let mut menu = menu.min_w(px(248.0)).max_w(px(360.0)).scrollable(true);
                        let Some(entity) = workspace.upgrade() else {
                            return menu;
                        };
                        let this = entity.read(cx);
                        // Capture labels only when opening, not on every chrome repaint.
                        let entries = this
                            .tabs
                            .iter()
                            .enumerate()
                            .map(|(index, tab)| {
                                let name = this
                                    .sidebar
                                    .read(cx)
                                    .session(index)
                                    .map_or(tab.title.as_str(), |entry| entry.name.as_str());
                                (
                                    tab.summary_id,
                                    SharedString::from(name.to_owned()),
                                    index == this.active_tab,
                                    this.sidebar
                                        .read(cx)
                                        .session(index)
                                        .map(|entry| entry.identity_icons().to_vec())
                                        .unwrap_or_else(|| vec!["phosphor/terminal.svg"]),
                                )
                            })
                            .collect::<Vec<_>>();
                        for (session_id, title, active, icons) in &entries {
                            let session_id = *session_id;
                            let workspace = workspace.clone();
                            let title = title.clone();
                            let active = *active;
                            let icons = icons.clone();
                            menu = menu.item(
                                PopupMenuItem::element(move |_, cx| {
                                    tab_menu_row(
                                        session_id,
                                        title.clone(),
                                        &icons,
                                        active,
                                        cx.theme(),
                                    )
                                })
                                .on_click(move |_, window, cx| {
                                    let _ = workspace.update(cx, |this, cx| {
                                        // Resolve identity on click: tabs may close or reorder while the menu is open.
                                        if let Some(index) = this
                                            .tabs
                                            .iter()
                                            .position(|tab| tab.summary_id == session_id)
                                        {
                                            this.activate_tab(index, window, cx);
                                        }
                                    });
                                }),
                            );
                        }
                        menu
                    }),
                ),
            );
        }
        #[cfg(target_os = "linux")]
        {
            if linux_client_decorated(window) {
                tab_controls = tab_controls.mr(px(4.0));
            }
        }

        #[cfg(target_os = "macos")]
        if self.handoff_button_visible(cx) {
            tab_controls = tab_controls.child(
                chrome_button(
                    "handoff-agent",
                    "phosphor/handshake.svg",
                    None,
                    "Handoff Agent",
                    Some(Box::new(crate::HandoffAgent)),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_agent_handoff(&crate::HandoffAgent, window, cx);
                })),
            );
        }
        if !caption_docked {
            tab_controls = tab_controls.child(
                chrome_button(
                    "tab-new",
                    "phosphor/plus.svg",
                    None,
                    "New tab",
                    Some(Box::new(NewTab)),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_tab(&NewTab, window, cx);
                })),
            );
        }

        let left_sidebar_tooltip = if self.left_panel_open {
            "Hide left sidebar"
        } else {
            "Show left sidebar"
        };
        {
            tab_controls = tab_controls.child(
                chrome_button(
                    "toggle-left-sidebar",
                    "phosphor/sidebar.svg",
                    Some(self.left_panel_open),
                    left_sidebar_tooltip,
                    Some(Box::new(ToggleLeftPanel)),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_left_panel(&ToggleLeftPanel, window, cx);
                })),
            );
        }

        // Input bar toggle
        let input_bar_tooltip = if self.input_bar_visible {
            "Hide input bar"
        } else {
            "Show input bar"
        };
        tab_controls = tab_controls.child(
            chrome_button(
                "toggle-input-bar",
                "phosphor/square-half-bottom-fill.svg",
                Some(self.input_bar_visible),
                input_bar_tooltip,
                Some(Box::new(crate::ToggleInputBar)),
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_input_bar(&crate::ToggleInputBar, window, cx);
            })),
        );

        // Agent panel toggle
        let agent_panel_tooltip = if self.agent_panel_open {
            "Hide agent panel"
        } else {
            "Show agent panel"
        };
        tab_controls = tab_controls.child(
            chrome_button(
                "toggle-agent-panel",
                "phosphor/square-half-fill.svg",
                Some(self.agent_panel_open),
                agent_panel_tooltip,
                Some(Box::new(ToggleAgentPanel)),
                cx,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.toggle_agent_panel(&ToggleAgentPanel, window, cx);
            })),
        );

        // Settings button — only on platforms without a native menu
        // bar. macOS exposes Settings through `App → Settings…` (and
        // ⌘,) so a gear in the chrome would be redundant there. On
        // Windows and Linux it's the primary discovery surface for
        // Settings alongside the command palette.
        #[cfg(not(target_os = "macos"))]
        {
            tab_controls = tab_controls.child(
                chrome_button(
                    "toggle-settings",
                    "phosphor/gear.svg",
                    None,
                    "Settings",
                    Some(Box::new(settings_panel::ToggleSettings)),
                    cx,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_settings(&settings_panel::ToggleSettings, window, cx);
                })),
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

        top_bar.relative().child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .opacity(tab_strip_progress)
                .child(self.tab_activity.clone()),
        )
    }
}

#[cfg(test)]
mod tab_surface_tests {
    use super::{tab_hover_surface, tab_label_detail, tab_resting_surface, tab_selected_surface};
    use gpui::{Hsla, Rgba, rgb};

    fn luminance(color: Hsla) -> f32 {
        let color = Rgba::from(color);
        let linear = |channel: f32| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn tab_details_keep_full_unicode_names_and_distinct_context() {
        let name = "项目 / a deliberately long workspace name";
        assert_eq!(tab_label_detail(name, Some(name), None), name);
        assert_eq!(
            tab_label_detail(name, Some("~/dev/project"), Some("Waiting for input")),
            format!("{name}\n~/dev/project\nWaiting for input")
        );
        assert_eq!(tab_label_detail("shell", Some(""), Some("shell")), "shell");
    }

    #[test]
    fn default_theme_selection_stays_distinct_without_fading_its_label() {
        for terminal_theme in [
            con_terminal::TerminalTheme::flexoki_dark(),
            con_terminal::TerminalTheme::flexoki_light(),
        ] {
            let backdrop: Hsla = rgb(terminal_theme.background.to_u32()).into();
            let ink: Hsla = rgb(terminal_theme.foreground.to_u32()).into();
            for chrome_opacity in [0.2, 0.82, 1.0] {
                let capsule = tab_selected_surface(
                    backdrop.opacity(chrome_opacity),
                    ink,
                    terminal_theme.is_dark(),
                );
                let painted = backdrop.blend(capsule);
                let resting = backdrop.blend(tab_resting_surface(ink, terminal_theme.is_dark()));
                let hovered = resting.blend(tab_hover_surface(ink, terminal_theme.is_dark()));
                assert!(
                    contrast(painted, resting) >= 1.10,
                    "{} selected and resting capsules are indistinguishable",
                    terminal_theme.name
                );
                for surface in [resting, hovered] {
                    assert!(contrast(ink, surface) >= 4.5);
                }
                assert!(
                    contrast(ink, painted) >= 4.5,
                    "{} selection obscured its label",
                    terminal_theme.name
                );
            }
        }
    }
}
