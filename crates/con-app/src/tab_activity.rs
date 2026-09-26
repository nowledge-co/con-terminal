//! One paint-only activity layer per tab chrome group. Row markers supply their
//! clipped bounds; only this small entity owns a repeating animation.

use std::{cell::RefCell, rc::Rc, time::Duration};

use con_core::terminal_status::{Activity, Status};
use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, ContentMask, Context, Hsla, IntoElement,
    ParentElement, Pixels, Render, Styled, Window, canvas, div, fill, px,
};

struct Marker {
    bounds: Option<Bounds<Pixels>>,
    mask: Option<ContentMask<Pixels>>,
    color: Hsla,
    percent: Option<u8>,
    pulse: bool,
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
    /// Called once when the owning chrome rebuilds its rows, not on pulse ticks.
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
            Activity::Busy => theme.progress_bar,
            Activity::Error => theme.danger,
            Activity::Paused | Activity::NeedsInput => theme.warning,
            Activity::Unknown | Activity::Idle => return div().into_any_element(),
        };
        let mut markers = self.markers.borrow_mut();
        let index = markers.used;
        let previous = markers.rows.get(index);
        let marker = Marker {
            bounds: previous.and_then(|marker| marker.bounds),
            mask: previous.and_then(|marker| marker.mask),
            color: color.opacity(if active { 0.82 } else { 0.58 }),
            percent: status.percent,
            pulse: status.activity == Activity::Busy && status.percent.is_none(),
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
                if marker.pulse && was_visible != bounds.intersects(&mask.bounds) {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .left(px(inset))
        .right(px(inset))
        .bottom(px(1.0))
        .h(px(2.0))
        .into_any_element()
    }

    fn paint(&self, pulse: bool) -> AnyElement {
        let markers = self.markers.clone();
        canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let markers = markers.borrow();
                for marker in markers.rows[..markers.used]
                    .iter()
                    .filter(|marker| marker.pulse == pulse)
                {
                    let Some(mut bounds) = marker.bounds else {
                        continue;
                    };
                    window.with_content_mask(marker.mask, |window| {
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
                marker.pulse
                    && marker
                        .bounds
                        .zip(marker.mask)
                        .is_none_or(|(bounds, mask)| bounds.intersects(&mask.bounds))
            });
        log::trace!(target: "con::activity", "activity_layer_render markers={used} animated={animated}");
        drop(markers);
        let moving = div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(self.paint(true));
        let moving = if animated {
            moving
                .with_animation(
                    "tab-activity",
                    Animation::new(Duration::from_secs(2))
                        .repeat_synced()
                        .with_max_fps(24.0),
                    |element, delta| {
                        element.opacity(0.75 + 0.25 * (std::f32::consts::TAU * delta).cos())
                    },
                )
                .into_any_element()
        } else {
            moving.into_any_element()
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(self.paint(false))
            .child(moving)
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
        layer.marker(status, &theme, true, 0.0);
        layer.marker(status, &theme, false, 0.0);
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
        layer.marker(status, &theme, true, 0.0);
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
    fn determinate_and_attention_markers_do_not_pulse() {
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
        assert!(markers.rows.iter().all(|marker| !marker.pulse));
        assert_eq!(markers.rows[0].percent, Some(37));
    }
}
