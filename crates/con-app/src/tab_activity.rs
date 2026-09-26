//! One activity overlay per tab chrome group. Row markers supply clipped bounds;
//! this entity owns the clock, but GPUI also invalidates its rendered ancestors.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use con_core::terminal_status::{Activity, Status};
use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, ContentMask, Context, Hsla, IntoElement,
    ParentElement, Pixels, Render, Styled, TransformationMatrix, Window, canvas, div, fill, px,
    radians, svg,
};

struct Marker {
    bounds: Option<Bounds<Pixels>>,
    mask: Option<ContentMask<Pixels>>,
    color: Hsla,
    percent: Option<u8>,
    ring: bool,
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
    /// Called once when the owning chrome rebuilds its rows, not on animation ticks.
    pub(crate) fn clear(&self) {
        self.markers.borrow_mut().used = 0;
    }

    pub(crate) fn marker(
        &self,
        status: Status,
        theme: &gpui_component::Theme,
        active: bool,
        inset: f32,
    ) -> AnyElement {
        let color = match status.activity {
            Activity::Busy if status.percent.is_some() => theme.progress_bar,
            Activity::Error => theme.danger,
            Activity::Paused | Activity::NeedsInput => theme.warning,
            Activity::Busy | Activity::Unknown | Activity::Idle => return div().into_any_element(),
        };
        self.register(
            color.opacity(if active { 0.82 } else { 0.58 }),
            status.percent,
            false,
        )
        .absolute()
        .left(px(inset))
        .right(px(inset))
        .bottom(px(1.0))
        .h(px(2.0))
        .into_any_element()
    }

    /// Keep a fixed slot in every state so work starting/stopping cannot move labels.
    pub(crate) fn icon(
        &self,
        icon: &'static str,
        size: Pixels,
        color: Hsla,
        status: Option<Status>,
        theme: &gpui_component::Theme,
    ) -> AnyElement {
        let mut slot = div()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .size(size * 1.75)
            .child(
                svg()
                    .path(icon)
                    .size(size)
                    .flex_shrink_0()
                    .text_color(color),
            );
        if status
            .is_some_and(|status| status.activity == Activity::Busy && status.percent.is_none())
        {
            slot = slot.child(
                self.register(theme.progress_bar, None, true)
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            );
        }
        slot.into_any_element()
    }

    fn register(&self, color: Hsla, percent: Option<u8>, ring: bool) -> impl Styled + IntoElement {
        let mut markers = self.markers.borrow_mut();
        let index = markers.used;
        let previous = markers.rows.get(index);
        let marker = Marker {
            bounds: previous.and_then(|marker| marker.bounds),
            mask: previous.and_then(|marker| marker.mask),
            color,
            percent,
            ring,
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
                if marker.ring && was_visible != bounds.intersects(&mask.bounds) {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
    }

    fn paint(&self, angle: Rc<Cell<f32>>) -> AnyElement {
        let markers = self.markers.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let markers = markers.borrow();
                for marker in &markers.rows[..markers.used] {
                    let Some(mut bounds) = marker.bounds else {
                        continue;
                    };
                    window.with_content_mask(marker.mask, |window| {
                        if marker.ring {
                            let center = bounds.center().scale(window.scale_factor());
                            let transform = TransformationMatrix::unit()
                                .translate(center)
                                .rotate(radians(angle.get()))
                                .translate(bounds.center().scale(-window.scale_factor()));
                            if let Err(error) = window.paint_svg(
                                bounds,
                                "phosphor/circle-notch.svg".into(),
                                None,
                                transform,
                                marker.color,
                                cx,
                            ) {
                                log::error!("activity ring: {error}");
                            }
                            return;
                        }
                        if let Some(percent) = marker.percent {
                            window.paint_quad(fill(bounds, marker.color.opacity(0.2)));
                            bounds.size.width *= f32::from(percent.min(100)) / 100.0;
                        }
                        window.paint_quad(fill(bounds, marker.color));
                    });
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}

impl Render for TabActivity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut markers = self.markers.borrow_mut();
        let used = markers.used;
        markers.rows.truncate(used);
        let animated = window.is_visible()
            && !cx.reduce_motion()
            && markers.rows.iter().any(|marker| {
                marker.ring
                    && marker
                        .bounds
                        .zip(marker.mask)
                        .is_none_or(|(bounds, mask)| bounds.intersects(&mask.bounds))
            });
        log::trace!(target: "con::activity", "activity_layer_render markers={used} animated={animated}");
        drop(markers);
        let angle = Rc::new(Cell::new(0.0));
        let moving = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(self.paint(angle.clone()));
        let moving = if animated {
            moving
                .with_animation(
                    "tab-activity",
                    Animation::new(Duration::from_millis(1200)).repeat_synced(),
                    move |element, delta| {
                        angle.set(std::f32::consts::TAU * delta);
                        element
                    },
                )
                .into_any_element()
        } else {
            moving.into_any_element()
        };
        div().absolute().top_0().left_0().size_full().child(moving)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use con_core::terminal_status::Evidence;
    use gpui::{Point, point, size};

    #[test]
    fn rebuilding_rows_retains_clipping_and_excludes_removed_rows() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        let status = Status {
            surface_id: 1,
            activity: Activity::Busy,
            evidence: Evidence::Progress,
            percent: None,
        };
        layer.icon(
            "phosphor/terminal.svg",
            px(16.0),
            theme.foreground,
            Some(status),
            &theme,
        );
        layer.icon(
            "phosphor/terminal.svg",
            px(16.0),
            theme.foreground,
            Some(status),
            &theme,
        );
        let bounds = Bounds::new(point(px(0.0), px(200.0)), size(px(80.0), px(2.0)));
        let mask = ContentMask {
            bounds: Bounds::new(Point::default(), size(px(80.0), px(100.0))),
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
            Some(status),
            &theme,
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
    fn determinate_and_attention_markers_do_not_rotate() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        for (activity, percent) in [
            (Activity::Busy, Some(37)),
            (Activity::NeedsInput, None),
            (Activity::Error, None),
            (Activity::Idle, None),
        ] {
            layer.marker(
                Status {
                    surface_id: 1,
                    activity,
                    evidence: Evidence::Progress,
                    percent,
                },
                &theme,
                false,
                0.0,
            );
        }
        let markers = layer.markers.borrow();
        assert_eq!(markers.used, 3);
        assert!(markers.rows.iter().all(|marker| !marker.ring));
        assert_eq!(markers.rows[0].percent, Some(37));
    }

    #[test]
    fn busy_uses_full_contrast_ring_instead_of_an_underline() {
        let layer = TabActivity::default();
        let theme = gpui_component::Theme::default();
        let mut status = Status {
            surface_id: 1,
            activity: Activity::Busy,
            evidence: Evidence::Progress,
            percent: None,
        };
        layer.marker(status, &theme, false, 0.0);
        assert_eq!(layer.markers.borrow().used, 0);
        layer.icon(
            "phosphor/terminal.svg",
            px(16.0),
            theme.foreground.opacity(0.38),
            Some(status),
            &theme,
        );
        {
            let markers = layer.markers.borrow();
            assert_eq!(markers.used, 1);
            assert!(markers.rows[0].ring);
            assert_eq!(markers.rows[0].color, theme.progress_bar);
        }
        layer.clear();
        status.percent = Some(37);
        layer.icon(
            "phosphor/terminal.svg",
            px(16.0),
            theme.foreground,
            Some(status),
            &theme,
        );
        assert_eq!(layer.markers.borrow().used, 0);
        layer.marker(status, &theme, false, 0.0);
        let markers = layer.markers.borrow();
        assert_eq!(markers.used, 1);
        assert!(!markers.rows[0].ring);
        assert_eq!(markers.rows[0].percent, Some(37));
    }
}
