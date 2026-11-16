//! Retained activity animation shared by tab chrome and chat status.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    time::{Duration, Instant},
};

use con_core::terminal_status::{Activity, Status};
use gpui::{
    Animation, AnimationExt, AnyElement, App, AppContext, Bounds, ContentMask, Context, Hsla,
    InteractiveElement, IntoElement, ParentElement, PathBuilder, Pixels, Render, Size,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, px, svg,
};
use gpui_component::ActiveTheme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivityVisual {
    None,
    Busy,
    BusyBars,
    Progress(u8),
    NeedsInput,
    Error,
    Paused,
}

impl ActivityVisual {
    fn arc(self, phase: f32, reduced_motion: bool) -> (f32, f32) {
        use std::f32::consts::{FRAC_PI_2, TAU};
        match self {
            Self::Busy | Self::BusyBars if reduced_motion => (0.0, TAU),
            Self::BusyBars => (0.0, 0.0),
            Self::Busy => (phase * TAU, FRAC_PI_2),
            Self::Progress(percent) => (-FRAC_PI_2, TAU * f32::from(percent) / 100.0),
            Self::NeedsInput | Self::Error | Self::Paused | Self::None => (0.0, 0.0),
        }
    }

    fn glyph(self) -> Option<&'static str> {
        match self {
            Self::NeedsInput => Some("phosphor/warning.svg"),
            Self::Error => Some("phosphor/x.svg"),
            Self::Paused => Some("phosphor/pause.svg"),
            Self::None | Self::Busy | Self::BusyBars | Self::Progress(_) => None,
        }
    }
}

impl From<Status> for ActivityVisual {
    fn from(status: Status) -> Self {
        match (status.activity, status.percent) {
            (Activity::Busy, Some(percent)) => Self::Progress(percent.min(100)),
            (Activity::Busy, None) => Self::Busy,
            (Activity::NeedsInput, _) => Self::NeedsInput,
            (Activity::Error, _) => Self::Error,
            (Activity::Paused, _) => Self::Paused,
            (Activity::Done, _) => Self::None,
            (Activity::Unknown | Activity::Idle, _) => Self::None,
        }
    }
}

/// Completion is an unread signal, never an application identity. Keep its
/// protocol acknowledgement on the owning surface; selecting a tab is not input.
pub(crate) fn notification_label(
    status: Option<Status>,
    unread: bool,
    active: bool,
) -> Option<&'static str> {
    match status.map(|status| status.activity) {
        Some(Activity::Done) => Some("Completed"),
        Some(Activity::Error | Activity::NeedsInput | Activity::Paused) => None,
        _ if unread && !active => Some("Unread activity"),
        _ => None,
    }
}

pub(crate) fn notification_dot(color: Hsla) -> gpui::Stateful<gpui::Div> {
    div()
        .id("tab-notification-dot")
        .debug_selector(|| "tab-notification-dot".into())
        .role(gpui::Role::Label)
        .aria_label("Unread activity")
        .size(px(5.0))
        .rounded_full()
        .bg(color)
}

/// Shared across orientations and keyed by tab identity, never row position.
#[derive(Default)]
pub(crate) struct TabNotifications(RefCell<HashMap<u64, NotificationMotion>>);

struct NotificationMotion {
    visible: bool,
    value: crate::motion::MotionValue,
}

impl NotificationMotion {
    fn sample(&mut self, visible: bool, now: Instant, reduced: bool, frame: impl FnOnce()) -> f32 {
        if self.visible != visible {
            self.visible = visible;
            self.value.set_target_at(
                if visible { 1.0 } else { 0.0 },
                Duration::from_millis(if visible { 180 } else { 120 }),
                now,
            );
        }
        self.value.update_at(now, reduced, frame)
    }
}

impl TabNotifications {
    pub(crate) fn retain(&self, keep: impl Fn(&u64) -> bool) {
        self.0.borrow_mut().retain(|id, _| keep(id));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dot(
        &self,
        id: u64,
        label: Option<&'static str>,
        color: Hsla,
        slot: Size<Pixels>,
        group: Size<Pixels>,
        window: &mut Window,
        cx: &App,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let mut motions = self.0.borrow_mut();
        // An absent tab needs no allocation and no animation callback.
        if label.is_none() && !motions.contains_key(&id) {
            return None;
        }
        let motion = motions.entry(id).or_insert_with(|| NotificationMotion {
            visible: false,
            value: crate::motion::MotionValue::new(0.0),
        });
        let progress = motion.sample(label.is_some(), Instant::now(), cx.reduce_motion(), || {
            window.request_animation_frame();
        });
        if label.is_none() && progress == 0.0 {
            return None;
        }
        // Animate the fill itself, not inherited opacity on cached brand artwork.
        // A fixed inset footprint keeps the marker attached to its identity.
        let diameter = px(4.0);
        let mut dot = div()
            .id("tab-notification-dot")
            .absolute()
            .left((slot.width + group.width) * 0.5 - diameter)
            .top((slot.height - group.height) * 0.5)
            .size(diameter)
            .rounded_full()
            .bg(color.opacity(0.78 * progress));
        if let Some(label) = label {
            dot = dot
                .debug_selector(|| "tab-notification-dot".into())
                .role(gpui::Role::Label)
                .aria_label(label);
        }
        Some(dot)
    }
}

struct Marker {
    bounds: Option<Bounds<Pixels>>,
    mask: Option<ContentMask<Pixels>>,
    color: Hsla,
    visual: ActivityVisual,
}

#[derive(Default)]
struct Markers {
    rows: Vec<Marker>,
    used: usize,
}

#[derive(Default)]
pub(crate) struct TabActivity {
    markers: Rc<RefCell<Markers>>,
}

/// A standalone marker sharing the tabs' retained animation renderer.
pub(crate) struct BusyIndicator {
    layer: gpui::Entity<TabActivity>,
}

impl BusyIndicator {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            layer: cx.new(|cx| TabActivity::new(window, cx)),
        }
    }
}

impl Render for BusyIndicator {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let size = crate::ui_scale::ui_icon_px(theme, 16.0);
        let layer = self.layer.read(cx);
        layer.clear();
        let marker = layer.register(theme.muted_foreground, ActivityVisual::BusyBars);
        div()
            .debug_selector(|| "busy-bars".into())
            .relative()
            .flex_none()
            .size(size)
            .child(marker.size_full())
            .child(self.layer.clone())
    }
}

impl TabActivity {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // A hidden-window render can retire the last animation callback.
        // Uncovering must restart it without activation or terminal output.
        cx.observe_window_visibility(window, |_, _, _, cx| cx.notify())
            .detach();
        Self::default()
    }

    pub(crate) fn clear(&self) {
        self.markers.borrow_mut().used = 0;
    }

    /// A small standalone status slot for hover details. Busy/progress/attention
    /// use exactly the tab painter; completion remains an unread dot.
    pub(crate) fn status_marker(
        &self,
        status: Status,
        size: Pixels,
        theme: &gpui_component::Theme,
    ) -> AnyElement {
        match status.activity {
            Activity::Done => div()
                .size(size)
                .flex()
                .items_center()
                .justify_center()
                .child(notification_dot(theme.primary).aria_label("Completed"))
                .into_any_element(),
            Activity::Idle | Activity::Unknown => svg()
                .path("phosphor/minus.svg")
                .size(size)
                .text_color(theme.muted_foreground)
                .into_any_element(),
            _ => self.icon_group(&[], size, theme.foreground, Some(status), theme, false),
        }
    }

    /// Keeps the caller's original icon slot. Compact working states retain the
    /// brand inside a ring. Stacks use a centered loader below the artwork.
    #[cfg(test)]
    pub(crate) fn icon(
        &self,
        icon: &'static str,
        size: Pixels,
        color: Hsla,
        status: Option<Status>,
        theme: &gpui_component::Theme,
        compact: bool,
    ) -> AnyElement {
        self.icon_group(&[icon], size, color, status, theme, compact)
    }

    pub(crate) fn icon_group(
        &self,
        icons: &[&'static str],
        size: Pixels,
        color: Hsla,
        status: Option<Status>,
        theme: &gpui_component::Theme,
        compact: bool,
    ) -> AnyElement {
        let visual = status
            .map(ActivityVisual::from)
            .unwrap_or(ActivityVisual::None);
        let visual = if visual == ActivityVisual::Busy && (!compact || icons.len() > 1) {
            ActivityVisual::BusyBars
        } else {
            visual
        };
        let active = visual != ActivityVisual::None;
        let slot_size = if compact { px(32.0) } else { size };
        let icon_size = if compact { size.min(px(16.0)) } else { size };
        let group_width = crate::assets::tab_identity_group_width(icons.len(), icon_size, compact);
        let group_height = crate::assets::tab_identity_group_height(icons.len(), icon_size);
        let mut slot = div()
            .debug_selector(|| "tab-status-slot".into())
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .w(if compact { slot_size } else { group_width })
            .h(if compact { slot_size } else { group_height });
        if !active
            || (compact
                && matches!(
                    visual,
                    ActivityVisual::Busy | ActivityVisual::BusyBars | ActivityVisual::Progress(_)
                ))
        {
            slot = slot.child(
                div()
                    .debug_selector(|| "tab-status-brand".into())
                    .w(group_width)
                    .h(group_height)
                    .flex_shrink_0()
                    .child(crate::assets::tab_identity_group(
                        icons, icon_size, color, compact, theme,
                    )),
            );
        }
        if active {
            let activity_color = match visual {
                ActivityVisual::NeedsInput | ActivityVisual::Paused => theme.warning,
                ActivityVisual::Error => theme.danger,
                ActivityVisual::Progress(_) | ActivityVisual::Busy | ActivityVisual::BusyBars => {
                    theme.foreground
                }
                ActivityVisual::None => unreachable!(),
            };
            if let Some(glyph) = visual.glyph() {
                return slot
                    .child(
                        svg()
                            .debug_selector(|| "tab-status-glyph".into())
                            .path(glyph)
                            .size(if compact { px(18.0) } else { size })
                            .text_color(activity_color),
                    )
                    .into_any_element();
            }
            let activity_color = if compact {
                activity_color.opacity(0.7)
            } else {
                activity_color
            };
            let marker = self.register(activity_color, visual).absolute();
            slot = if compact && icons.len() == 1 {
                // A single identity has a quiet, concentric silhouette. The
                // 16pt artwork leaves a clear gap inside the 24pt activity ring.
                slot.child(marker.size(px(24.0)))
            } else if compact {
                // A fixed lower lane keeps motion on the identity group’s axis,
                // inside the selection fill and clear of the unread corner.
                let width = if visual == ActivityVisual::BusyBars {
                    px(10.0)
                } else {
                    px(6.0)
                };
                slot.child(
                    marker
                        .left((slot_size - width) / 2.0)
                        .bottom(px(1.0))
                        .w(width)
                        .h(px(5.0)),
                )
            } else {
                slot.child(marker.size(slot_size.min(px(18.0))))
            };
        }
        slot.into_any_element()
    }

    #[cfg(test)]
    pub(crate) fn rendered_ring_bounds(&self) -> Vec<Bounds<Pixels>> {
        let markers = self.markers.borrow();
        markers.rows[..markers.used]
            .iter()
            .filter_map(|marker| marker.bounds)
            .collect()
    }

    fn register(&self, color: Hsla, visual: ActivityVisual) -> impl Styled + IntoElement {
        let mut markers = self.markers.borrow_mut();
        let index = markers.used;
        let previous = markers.rows.get(index);
        let marker = Marker {
            bounds: previous.and_then(|marker| marker.bounds),
            mask: previous.and_then(|marker| marker.mask),
            color,
            visual,
        };
        if index == markers.rows.len() {
            markers.rows.push(marker);
        } else {
            markers.rows[index] = marker;
        }
        markers.used += 1;
        drop(markers);
        let markers = self.markers.clone();
        canvas(
            move |bounds, window, _| {
                let mut markers = markers.borrow_mut();
                let marker = &mut markers.rows[index];
                let was_visible = marker
                    .bounds
                    .zip(marker.mask)
                    .is_some_and(|(bounds, mask)| bounds.intersects(&mask.bounds));
                let mask = window.content_mask();
                marker.bounds = Some(bounds);
                marker.mask = Some(mask);
                if matches!(
                    marker.visual,
                    ActivityVisual::Busy | ActivityVisual::BusyBars
                ) && was_visible != bounds.intersects(&mask.bounds)
                {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
    }

    fn paint(&self, phase: Rc<Cell<f32>>, reduced_motion: bool) -> AnyElement {
        let markers = self.markers.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _cx| {
                let markers = markers.borrow();
                for marker in &markers.rows[..markers.used] {
                    let Some(bounds) = marker.bounds else {
                        continue;
                    };
                    window.with_content_mask(marker.mask, |window| {
                        paint_ring(
                            window,
                            bounds,
                            marker.color,
                            marker.visual,
                            phase.get(),
                            reduced_motion,
                        );
                    });
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}

fn busy_bar_strength(phase: f32, index: usize, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return 0.65;
    }
    let angle = (phase - index as f32 * 0.2) * std::f32::consts::TAU;
    0.5 - 0.5 * angle.cos()
}

fn paint_ring(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    color: Hsla,
    visual: ActivityVisual,
    phase: f32,
    reduced_motion: bool,
) {
    if visual == ActivityVisual::BusyBars {
        for index in 0..3 {
            let strength = busy_bar_strength(phase, index, reduced_motion);
            let width = bounds.size.width.min(px(18.0));
            let unit = width / 9.0;
            let height = bounds.size.height.min(px(14.0)) * (0.35 + 0.65 * strength);
            let origin = point(
                bounds.center().x - unit * 3.5 + unit * 2.5 * index as f32,
                bounds.center().y - height / 2.0,
            );
            window.paint_quad(
                gpui::fill(
                    Bounds::new(origin, gpui::size(unit * 2.0, height)),
                    color.opacity(0.65 + 0.35 * strength),
                )
                .corner_radii(unit),
            );
        }
        return;
    }
    let center = bounds.center();
    let radius = (bounds.size.width.min(bounds.size.height) / 2.0) - px(1.0);
    let draw_arc = |window: &mut Window, start: f32, sweep: f32, tone: Hsla| {
        if sweep <= 0.0 {
            return;
        }
        let at = |angle: f32| {
            point(
                center.x + radius * angle.cos(),
                center.y + radius * angle.sin(),
            )
        };
        let stroke = px(1.5);
        let mut path = PathBuilder::stroke(stroke);
        path.move_to(at(start));
        // Two arcs also handle a full circle, whose endpoints coincide.
        path.arc_to(
            point(radius, radius),
            px(0.0),
            false,
            true,
            at(start + sweep / 2.0),
        );
        path.arc_to(
            point(radius, radius),
            px(0.0),
            false,
            true,
            at(start + sweep),
        );
        if let Ok(path) = path.build() {
            window.paint_path(path, tone)
        }
        if sweep < std::f32::consts::TAU {
            // GPUI exposes StrokeOptions but not its line-cap type. Endpoint
            // disks give the short arc round ends without another dependency.
            for angle in [start, start + sweep] {
                let endpoint = at(angle);
                window.paint_quad(
                    gpui::fill(
                        Bounds::new(
                            endpoint - point(stroke / 2.0, stroke / 2.0),
                            gpui::size(stroke, stroke),
                        ),
                        tone,
                    )
                    .corner_radii(px(0.75)),
                );
            }
        }
    };
    if matches!(visual, ActivityVisual::Busy | ActivityVisual::Progress(_)) {
        draw_arc(window, 0.0, std::f32::consts::TAU, color.opacity(0.08));
    }
    let (start, sweep) = visual.arc(phase, reduced_motion);
    draw_arc(window, start, sweep, color);
}

impl Render for TabActivity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        log::trace!(target: "con::activity", "activity_layer_render");
        let mut markers = self.markers.borrow_mut();
        let used = markers.used;
        markers.rows.truncate(used);
        let animated = window.is_visible()
            && !cx.reduce_motion()
            && markers.rows.iter().any(|marker| {
                matches!(
                    marker.visual,
                    ActivityVisual::Busy | ActivityVisual::BusyBars
                ) && marker
                    .bounds
                    .zip(marker.mask)
                    .is_none_or(|(bounds, mask)| bounds.intersects(&mask.bounds))
            });
        drop(markers);
        let phase = Rc::new(Cell::new(0.0));
        let layer = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(self.paint(phase.clone(), cx.reduce_motion()));
        let layer = if animated {
            layer
                .with_animation(
                    "tab-activity",
                    Animation::new(Duration::from_millis(1200)).repeat_synced(),
                    move |element, delta| {
                        phase.set(delta);
                        element
                    },
                )
                .into_any_element()
        } else {
            layer.into_any_element()
        };
        div().absolute().top_0().left_0().size_full().child(layer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use con_core::terminal_status::Evidence;

    #[test]
    fn notification_state_follows_identity_and_releases_closed_tabs() {
        let notifications = super::TabNotifications::default();
        for id in [17, 42] {
            notifications.0.borrow_mut().insert(
                id,
                super::NotificationMotion {
                    visible: true,
                    value: crate::motion::MotionValue::new(1.0),
                },
            );
        }
        // Reordering preserves the same retained transition; removing a tab
        // releases its state instead of transferring it to the next row.
        notifications.retain(|id| [42, 17].contains(id));
        assert_eq!(notifications.0.borrow().len(), 2);
        notifications.retain(|id| *id == 42);
        assert!(!notifications.0.borrow().contains_key(&17));
        assert_eq!(notifications.0.borrow()[&42].value.current(), 1.0);
    }

    #[test]
    fn notification_motion_is_finite_reversible_and_does_not_restart() {
        let start = std::time::Instant::now();
        let mut motion = super::NotificationMotion {
            visible: false,
            value: crate::motion::MotionValue::new(0.0),
        };
        let frames = std::cell::Cell::new(0);
        let frame = || frames.set(frames.get() + 1);
        assert_eq!(motion.sample(true, start, false, frame), 0.0);
        let middle = start + std::time::Duration::from_millis(60);
        let before = motion.sample(true, middle, false, frame);
        assert!(before > 0.0 && before < 1.0);
        assert_eq!(motion.sample(false, middle, false, frame), before);
        assert!(
            motion.sample(
                false,
                middle + std::time::Duration::from_millis(30),
                false,
                frame
            ) < before
        );
        assert_eq!(
            motion.sample(
                false,
                middle + std::time::Duration::from_millis(120),
                false,
                frame
            ),
            0.0
        );
        let settled_frames = frames.get();
        assert_eq!(
            motion.sample(
                false,
                middle + std::time::Duration::from_secs(1),
                false,
                frame
            ),
            0.0
        );
        assert_eq!(frames.get(), settled_frames);
        assert_eq!(
            motion.sample(
                true,
                middle + std::time::Duration::from_secs(2),
                true,
                frame
            ),
            1.0
        );
        assert_eq!(frames.get(), settled_frames);
        assert_eq!(
            motion.sample(
                false,
                middle + std::time::Duration::from_secs(3),
                true,
                frame
            ),
            0.0
        );
        assert_eq!(frames.get(), settled_frames);
    }

    #[test]
    fn completion_and_unread_share_a_signal_without_hiding_urgent_states() {
        for active in [false, true] {
            for unread in [false, true] {
                assert_eq!(
                    notification_label(Some(status(Activity::Done, None)), unread, active),
                    Some("Completed")
                );
                for activity in [Activity::NeedsInput, Activity::Error, Activity::Paused] {
                    assert_eq!(
                        notification_label(Some(status(activity, None)), unread, active),
                        None
                    );
                }
            }
        }
        for activity in [Activity::Unknown, Activity::Idle, Activity::Busy] {
            assert_eq!(
                notification_label(Some(status(activity, None)), true, false),
                Some("Unread activity")
            );
            assert_eq!(
                notification_label(Some(status(activity, None)), true, true),
                None
            );
            assert_eq!(
                notification_label(Some(status(activity, None)), false, false),
                None
            );
        }
        assert_eq!(
            notification_label(None, true, false),
            Some("Unread activity")
        );
        assert_eq!(notification_label(None, true, true), None);
    }

    #[gpui::test]
    fn completion_retains_the_whole_identity_stack_in_both_densities(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Fixture {
            activity: Activity,
            compact: bool,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let theme = cx.theme();
                div().child(TabActivity::default().icon_group(
                    &[
                        "phosphor/terminal.svg",
                        "phosphor/code.svg",
                        "phosphor/book-open.svg",
                    ],
                    px(18.0),
                    theme.foreground,
                    Some(status(self.activity, None)),
                    theme,
                    self.compact,
                ))
            }
        }
        cx.update(gpui_component::init);
        let (view, cx) = cx.add_window_view(|_, _| Fixture {
            activity: Activity::Idle,
            compact: false,
        });
        for compact in [false, true] {
            view.update(cx, |view, cx| {
                view.compact = compact;
                view.activity = Activity::Idle;
                cx.notify();
            });
            let idle = cx.debug_bounds("tab-status-brand").unwrap();
            let slot = cx.debug_bounds("tab-status-slot").unwrap();
            view.update(cx, |view, cx| {
                view.activity = Activity::Done;
                cx.notify();
            });
            assert_eq!(cx.debug_bounds("tab-status-brand").unwrap(), idle);
            assert_eq!(cx.debug_bounds("tab-status-slot").unwrap(), slot);
            assert!(cx.debug_bounds("tab-status-glyph").is_none());
        }
    }

    #[test]
    fn busy_bars_are_periodic_staggered_and_static_with_reduced_motion() {
        for index in 0..3 {
            for phase in [0.0, 0.2, 0.5, 0.9] {
                let value = super::busy_bar_strength(phase, index, false);
                assert!((0.0..=1.0).contains(&value));
                assert!(
                    (value - super::busy_bar_strength(phase + 1.0, index, false)).abs() < 0.00001
                );
                assert_eq!(super::busy_bar_strength(phase, index, true), 0.65);
            }
        }
        assert!(super::busy_bar_strength(0.0, 0, false) < super::busy_bar_strength(0.0, 1, false));
    }

    #[gpui::test]
    fn standalone_busy_bars_schedule_one_frame_per_tick(cx: &mut gpui::TestAppContext) {
        struct Host {
            panel: gpui::Entity<Parent>,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                // Match the fixed-size cached panel boundary in the workspace.
                div().size_full().child(
                    self.panel
                        .clone()
                        .cached(gpui::StyleRefinement::default().size_full()),
                )
            }
        }
        struct Parent {
            indicator: gpui::Entity<super::BusyIndicator>,
            renders: Rc<Cell<usize>>,
        }
        impl Render for Parent {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.renders.set(self.renders.get() + 1);
                div().size_full().child(self.indicator.clone())
            }
        }
        cx.update(gpui_component::init);
        let renders = Rc::new(Cell::new(0));
        let (_, view) = cx.add_window_view(|window, cx| Host {
            panel: cx.new(|cx| Parent {
                indicator: cx.new(|cx| super::BusyIndicator::new(window, cx)),
                renders: renders.clone(),
            }),
        });
        view.update(|window, _| window.activate_window());
        view.run_until_parked();
        assert!(view.debug_bounds("busy-bars").is_some());
        // Drain the initial visibility-change and animation notifications.
        view.update(|window, cx| window.simulate_next_frame(cx));
        for _ in 0..3 {
            let initial_renders = renders.get();
            let frames = view.update(|window, cx| window.simulate_next_frame(cx));
            assert_eq!(frames, 1, "busy bars must not schedule duplicate frames");
            assert!(renders.get() <= initial_renders + 1);
        }
    }

    #[gpui::test]
    fn busy_animation_follows_visibility_not_activation(cx: &mut gpui::TestAppContext) {
        let (layer, cx) = cx.add_window_view(|window, cx| {
            let layer = TabActivity::new(window, cx);
            let bounds = Bounds::new(point(px(8.0), px(8.0)), gpui::size(px(28.0), px(28.0)));
            *layer.markers.borrow_mut() = Markers {
                rows: vec![Marker {
                    bounds: Some(bounds),
                    mask: Some(ContentMask { bounds }),
                    color: gpui::black(),
                    visual: ActivityVisual::Busy,
                }],
                used: 1,
            };
            layer
        });
        let next_frame = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| window.simulate_next_frame(cx))
        };
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        cx.update(|window, cx| {
            assert!(window.is_window_active());
            layer.update(cx, |_, cx| cx.notify());
        });
        assert!(next_frame(cx) > 0, "visible Busy must schedule animation");

        cx.deactivate_window();
        cx.update(|window, cx| {
            assert!(!window.is_window_active());
            assert!(window.is_visible());
            layer.update(cx, |_, cx| cx.notify());
        });
        next_frame(cx); // Drain any frame queued before deactivation.
        assert!(
            next_frame(cx) > 0,
            "visible inactive Busy must keep animating"
        );

        cx.simulate_visibility_change(gpui::WindowVisibility::Hidden);
        // A status update can render while hidden and retire the animation.
        cx.update(|_, cx| layer.update(cx, |_, cx| cx.notify()));
        next_frame(cx);
        assert_eq!(next_frame(cx), 0, "hidden Busy must stop scheduling frames");

        cx.simulate_visibility_change(gpui::WindowVisibility::Visible);
        cx.run_until_parked();
        assert!(
            next_frame(cx) > 0,
            "uncovering without activation must resume Busy"
        );

        cx.update(|_, cx| cx.set_reduce_motion(true));
        next_frame(cx);
        assert_eq!(next_frame(cx), 0, "reduced motion must stop animation");
        cx.update(|_, cx| cx.set_reduce_motion(false));
        assert!(next_frame(cx) > 0);

        cx.update(|_, cx| {
            layer.update(cx, |layer, cx| {
                layer.markers.borrow_mut().rows[0].visual = ActivityVisual::Progress(37);
                cx.notify();
            });
        });
        next_frame(cx);
        assert_eq!(next_frame(cx), 0, "determinate progress must not spin");

        cx.update(|_, cx| {
            layer.update(cx, |layer, cx| {
                let mut markers = layer.markers.borrow_mut();
                markers.rows[0].visual = ActivityVisual::Busy;
                markers.rows[0].mask = Some(ContentMask {
                    bounds: Bounds::new(
                        point(px(100.0), px(100.0)),
                        gpui::size(px(28.0), px(28.0)),
                    ),
                });
                cx.notify();
            });
        });
        assert_eq!(next_frame(cx), 0, "clipped Busy must not schedule frames");
    }

    #[test]
    fn arc_geometry_preserves_progress_and_reduced_motion() {
        use std::f32::consts::{FRAC_PI_2, TAU};
        assert_eq!(
            ActivityVisual::Busy.arc(0.25, false),
            (FRAC_PI_2, FRAC_PI_2)
        );
        assert_eq!(ActivityVisual::Busy.arc(0.25, true), (0.0, TAU));
        assert_eq!(
            ActivityVisual::Progress(0).arc(0.8, false),
            (-FRAC_PI_2, 0.0)
        );
        assert_eq!(
            ActivityVisual::Progress(25).arc(0.8, false),
            (-FRAC_PI_2, FRAC_PI_2)
        );
        assert_eq!(
            ActivityVisual::Progress(100).arc(0.8, true),
            (-FRAC_PI_2, TAU)
        );
        assert_eq!(
            ActivityVisual::None.arc(0.8, false),
            ActivityVisual::Paused.arc(0.8, false)
        );
        assert_eq!(ActivityVisual::Paused.arc(0.8, false), (0.0, 0.0));
    }

    #[test]
    fn stationary_states_do_not_register_progress_overlays() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        for compact in [false, true] {
            for activity in [
                Activity::NeedsInput,
                Activity::Error,
                Activity::Paused,
                Activity::Done,
            ] {
                layer.icon(
                    "phosphor/terminal.svg",
                    px(16.0),
                    theme.foreground,
                    Some(status(activity, None)),
                    &theme,
                    compact,
                );
            }
        }
        assert_eq!(layer.markers.borrow().used, 0);
    }

    #[test]
    fn rebuilding_rows_retains_clipping_and_excludes_removed_rows() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        for _ in 0..2 {
            layer.icon(
                "phosphor/terminal.svg",
                px(16.0),
                theme.foreground,
                Some(status(Activity::Busy, None)),
                &theme,
                true,
            );
        }
        let bounds = Bounds::new(point(px(0.0), px(200.0)), gpui::size(px(28.0), px(28.0)));
        let mask = ContentMask {
            bounds: Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(80.0), px(100.0))),
        };
        {
            let mut markers = layer.markers.borrow_mut();
            markers.rows[0].bounds = Some(bounds);
            markers.rows[0].mask = Some(mask);
        }
        layer.clear();
        layer.icon(
            "phosphor/terminal.svg",
            px(16.0),
            theme.foreground,
            Some(status(Activity::Busy, None)),
            &theme,
            true,
        );
        let markers = layer.markers.borrow();
        assert_eq!(markers.used, 1);
        assert_eq!(markers.rows[0].bounds, Some(bounds));
        assert!(
            !markers.rows[0]
                .bounds
                .unwrap()
                .intersects(&markers.rows[0].mask.unwrap().bounds)
        );
    }

    #[test]
    fn expanded_attention_replaces_icon_without_registering_a_ring() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        for activity in [
            Activity::NeedsInput,
            Activity::Error,
            Activity::Paused,
            Activity::Done,
            Activity::Idle,
        ] {
            layer.icon(
                "phosphor/terminal.svg",
                px(16.0),
                theme.foreground,
                Some(status(activity, None)),
                &theme,
                false,
            );
        }
        assert_eq!(layer.markers.borrow().used, 0);
        layer.icon(
            "phosphor/terminal.svg",
            px(16.0),
            theme.foreground,
            Some(status(Activity::Busy, Some(37))),
            &theme,
            false,
        );
        let markers = layer.markers.borrow();
        assert_eq!(markers.used, 1);
        assert_eq!(markers.rows[0].visual, ActivityVisual::Progress(37));
        assert_eq!(markers.rows[0].color, theme.foreground.opacity(0.75));
    }

    fn status(activity: Activity, percent: Option<u8>) -> Status {
        Status {
            surface_id: 1,
            activity,
            evidence: Evidence::Progress,
            percent,
        }
    }

    #[test]
    fn compact_stacks_use_one_shared_loader_instead_of_surrounding_the_cards() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        for icons in [
            vec!["phosphor/terminal.svg"],
            vec!["phosphor/terminal.svg", "phosphor/code.svg"],
            vec![
                "phosphor/terminal.svg",
                "phosphor/code.svg",
                "phosphor/folder.svg",
            ],
        ] {
            layer.clear();
            layer.icon_group(
                &icons,
                px(18.0),
                theme.foreground,
                Some(status(Activity::Busy, None)),
                &theme,
                true,
            );
            let markers = layer.markers.borrow();
            assert_eq!(markers.used, 1);
            assert_eq!(
                markers.rows[0].visual,
                if icons.len() == 1 {
                    ActivityVisual::Busy
                } else {
                    ActivityVisual::BusyBars
                }
            );
        }
    }

    #[test]
    fn status_icons_register_rings_only_for_working_states_in_both_densities() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        for compact in [false, true] {
            layer.clear();
            for activity in [
                Activity::NeedsInput,
                Activity::Error,
                Activity::Paused,
                Activity::Done,
            ] {
                layer.icon(
                    "phosphor/terminal.svg",
                    px(24.0),
                    theme.foreground,
                    Some(status(activity, None)),
                    &theme,
                    compact,
                );
                assert_eq!(layer.markers.borrow().used, 0);
            }
            for (percent, visual) in [
                (None, ActivityVisual::Busy),
                (Some(40), ActivityVisual::Progress(40)),
            ] {
                layer.clear();
                layer.icon(
                    "phosphor/terminal.svg",
                    px(24.0),
                    theme.foreground,
                    Some(status(Activity::Busy, percent)),
                    &theme,
                    compact,
                );
                assert_eq!(layer.markers.borrow().used, 1);
                assert_eq!(
                    layer.markers.borrow().rows[0].visual,
                    if visual == ActivityVisual::Busy && !compact {
                        ActivityVisual::BusyBars
                    } else {
                        visual
                    }
                );
            }
        }
    }

    #[test]
    fn status_maps_to_compact_visuals() {
        assert_eq!(
            ActivityVisual::from(status(Activity::Idle, None)),
            ActivityVisual::None
        );
        assert_eq!(
            ActivityVisual::from(status(Activity::Busy, None)),
            ActivityVisual::Busy
        );
        assert_eq!(
            ActivityVisual::from(status(Activity::Busy, Some(120))),
            ActivityVisual::Progress(100)
        );
        assert_eq!(
            ActivityVisual::from(status(Activity::NeedsInput, None)),
            ActivityVisual::NeedsInput
        );
        assert_eq!(
            ActivityVisual::from(status(Activity::Error, None)),
            ActivityVisual::Error
        );
        assert_eq!(
            ActivityVisual::from(status(Activity::Paused, None)),
            ActivityVisual::Paused
        );
        assert_eq!(
            ActivityVisual::from(status(Activity::Done, None)),
            ActivityVisual::None
        );
    }
}
