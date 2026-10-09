//! One retained activity overlay per tab chrome group.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use con_core::terminal_status::{Activity, Status};
use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, ContentMask, Context, Hsla, IntoElement,
    ParentElement, PathBuilder, Pixels, Render, Styled, Window, canvas, div, point, px, svg,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivityVisual {
    None,
    Busy,
    Progress(u8),
    NeedsInput,
    Error,
    Paused,
    Done,
}

impl ActivityVisual {
    fn arc(self, phase: f32, reduced_motion: bool) -> (f32, f32) {
        use std::f32::consts::{FRAC_PI_2, TAU};
        match self {
            Self::Busy if reduced_motion => (0.0, TAU),
            Self::Busy => (phase * TAU, FRAC_PI_2),
            Self::Progress(percent) => (-FRAC_PI_2, TAU * f32::from(percent) / 100.0),
            Self::NeedsInput | Self::Error | Self::Paused | Self::Done | Self::None => (0.0, 0.0),
        }
    }

    fn glyph(self) -> Option<&'static str> {
        match self {
            Self::NeedsInput => Some("phosphor/warning.svg"),
            Self::Error => Some("phosphor/x.svg"),
            Self::Paused => Some("phosphor/pause.svg"),
            Self::Done => Some("phosphor/check.svg"),
            Self::None | Self::Busy | Self::Progress(_) => None,
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
            (Activity::Done, _) => Self::Done,
            (Activity::Unknown | Activity::Idle, _) => Self::None,
        }
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

    /// Keeps the caller's original icon slot. Compact rail slots retain the
    /// brand below a progress ring; stationary states use one centered glyph.
    pub(crate) fn icon(
        &self,
        icon: &'static str,
        size: Pixels,
        color: Hsla,
        status: Option<Status>,
        theme: &gpui_component::Theme,
        compact: bool,
    ) -> AnyElement {
        let visual = status
            .map(ActivityVisual::from)
            .unwrap_or(ActivityVisual::None);
        let active = visual != ActivityVisual::None;
        let slot_size = if compact { px(32.0) } else { size };
        let icon_size = if compact { size.min(px(14.0)) } else { size };
        let mut slot = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .size(slot_size);
        if !active || (compact && visual.glyph().is_none()) {
            slot = slot.child(
                svg()
                    .path(icon)
                    .size(icon_size)
                    .flex_shrink_0()
                    .text_color(color),
            );
        }
        if active {
            let activity_color = match visual {
                ActivityVisual::NeedsInput | ActivityVisual::Paused => theme.warning,
                ActivityVisual::Error => theme.danger,
                ActivityVisual::Done => theme.foreground,
                ActivityVisual::Progress(_) | ActivityVisual::Busy => theme.foreground,
                ActivityVisual::None => unreachable!(),
            };
            if let Some(glyph) = visual.glyph() {
                return slot
                    .child(
                        svg()
                            .path(glyph)
                            .size(if compact { px(18.0) } else { size })
                            .text_color(activity_color),
                    )
                    .into_any_element();
            }
            let ring_size = if compact {
                px(24.0)
            } else {
                slot_size.min(px(18.0))
            };
            slot = slot.child(
                self.register(activity_color, visual)
                    .absolute()
                    .size(ring_size),
            );
        }
        slot.into_any_element()
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
                if marker.visual == ActivityVisual::Busy
                    && was_visible != bounds.intersects(&mask.bounds)
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

fn paint_ring(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    color: Hsla,
    visual: ActivityVisual,
    phase: f32,
    reduced_motion: bool,
) {
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
        let mut path = PathBuilder::stroke(px(1.5));
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
                            endpoint - point(px(0.75), px(0.75)),
                            gpui::size(px(1.5), px(1.5)),
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
                marker.visual == ActivityVisual::Busy
                    && marker
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
                    Animation::new(Duration::from_secs(2)).repeat_synced(),
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
            ActivityVisual::Done.arc(0.8, false),
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
                assert_eq!(layer.markers.borrow().rows[0].visual, visual);
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
            ActivityVisual::Done
        );
    }
}
