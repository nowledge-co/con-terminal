use gpui::{Bounds, Pixels, point, px};

/// Match the pinned GPUI renderer's endpoint snapping (half ties toward zero).
/// Native hosts must occupy the same device pixels as GPUI's clips and dividers.
#[cfg(any(target_os = "macos", test))]
pub fn snap_terminal_bounds(bounds: Bounds<Pixels>, scale: f32) -> Bounds<Pixels> {
    let scale = scale.max(f32::EPSILON);
    let snap = |value: Pixels| {
        let scaled = value.as_f32() * scale;
        px((scaled.abs() - 0.5).ceil().copysign(scaled) / scale)
    };
    let left = snap(bounds.left());
    let top = snap(bounds.top());
    Bounds::from_corners(
        point(left, top),
        point(
            snap(bounds.right()).max(left),
            snap(bounds.bottom()).max(top),
        ),
    )
}

/// Retain terminal pixels without stretching them. Only uncovered pixels get
/// the terminal fill, measured at paint time rather than from last frame's size.
#[cfg(any(target_os = "windows", test))]
pub fn retained_frame_gaps(
    frame_size: gpui::Size<Pixels>,
    background: Option<gpui::Hsla>,
) -> gpui::Canvas<()> {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            if let Some(background) = background {
                for gap in frame_gaps(bounds, frame_size).into_iter().flatten() {
                    window.paint_quad(gpui::fill(gap, background));
                }
            }
        },
    )
}

#[cfg(any(target_os = "windows", test))]
fn frame_gaps(
    bounds: Bounds<Pixels>,
    frame_size: gpui::Size<Pixels>,
) -> [Option<Bounds<Pixels>>; 2] {
    let width = frame_size.width.max(px(0.0)).min(bounds.size.width);
    let height = frame_size.height.max(px(0.0)).min(bounds.size.height);
    [
        (width < bounds.size.width).then(|| {
            Bounds::from_corners(
                point(bounds.left() + width, bounds.top()),
                bounds.bottom_right(),
            )
        }),
        (height < bounds.size.height && width > px(0.0)).then(|| {
            Bounds::from_corners(
                point(bounds.left(), bounds.top() + height),
                point(bounds.left() + width, bounds.bottom()),
            )
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{IntoElement, ParentElement, Render, Styled, div, size};

    #[test]
    fn gaps_cover_only_uncovered_pixels_including_subpixel_growth() {
        let frame = size(px(100.0), px(80.0));
        for (width, height) in [(100.1, 80.2), (130.0, 90.0), (90.0, 90.0), (130.0, 70.0)] {
            let bounds = Bounds::new(point(px(17.25), px(32.5)), size(px(width), px(height)));
            let gaps = frame_gaps(bounds, frame);
            let painted_area: f32 = gaps
                .iter()
                .flatten()
                .map(|gap| {
                    assert!(gap.left() >= bounds.left() && gap.right() <= bounds.right());
                    assert!(gap.top() >= bounds.top() && gap.bottom() <= bounds.bottom());
                    gap.size.width.as_f32() * gap.size.height.as_f32()
                })
                .sum();
            let expected = width * height - width.min(100.0) * height.min(80.0);
            assert!((painted_area - expected).abs() < 0.01);
        }
        assert!(
            frame_gaps(
                Bounds::new(point(px(0.0), px(0.0)), size(px(90.0), px(70.0))),
                frame,
            )
            .iter()
            .all(Option::is_none)
        );
    }

    struct GapView {
        width: f32,
        height: f32,
        background: Option<gpui::Hsla>,
    }

    impl Render for GapView {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .w(px(self.width))
                .h(px(self.height))
                .child(retained_frame_gaps(size(px(100.0), px(80.0)), self.background).size_full())
        }
    }

    #[gpui::test]
    fn resize_gaps_follow_current_layout_without_a_second_render(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| GapView {
            width: 100.0,
            height: 80.0,
            background: Some(gpui::hsla(0.1, 0.2, 0.3, 0.65)),
        });
        for (width, height, expected_quads) in [(100.0, 80.0, 0), (130.0, 90.0, 2), (90.0, 70.0, 0)]
        {
            view.update(cx, |view, cx| {
                view.width = width;
                view.height = height;
                cx.notify();
            });
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                let quads = window.painted_quads();
                assert_eq!(quads.len(), expected_quads);
                for quad in quads {
                    assert_eq!(
                        quad.background,
                        gpui::Background::from(gpui::hsla(0.1, 0.2, 0.3, 0.65))
                    );
                }
            });
        }
        view.update(cx, |view, cx| {
            view.width = 150.0;
            view.height = 120.0;
            view.background = None;
            cx.notify();
        });
        cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(window.painted_quads().is_empty());
        });
    }

    struct SnappingView;

    impl Render for SnappingView {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            gpui::canvas(
                |_, _, _| {},
                |_, (), window, _| {
                    let bounds = Bounds::from_corners(
                        point(px(-0.5), px(17.25)),
                        point(px(100.5), px(130.75)),
                    );
                    window.paint_quad(gpui::fill(bounds, gpui::rgb(0x112233)));
                    window.paint_quad(gpui::fill(
                        snap_terminal_bounds(bounds, window.scale_factor()),
                        gpui::rgb(0x445566),
                    ));
                },
            )
            .size_full()
        }
    }

    #[gpui::test]
    fn native_snapping_matches_the_pinned_gpui_paint_path(cx: &mut gpui::TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| SnappingView);
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            cx.simulate_scale_factor_change(scale);
            cx.update(|window, cx| {
                let _ = window.draw(cx);
                let quads = window.painted_quads();
                assert_eq!(quads.len(), 2);
                assert_eq!(quads[0].bounds, quads[1].bounds, "scale {scale}");
            });
        }
    }

    #[test]
    fn native_endpoints_share_divider_pixels_at_fractional_scales() {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            for origin in [-10.5, 0.0, 17.25] {
                for edge in [100.25, 100.5, 100.75] {
                    let first =
                        Bounds::from_corners(point(px(origin), px(0.0)), point(px(edge), px(80.0)));
                    let divider = Bounds::from_corners(
                        point(px(edge), px(0.0)),
                        point(px(edge + 1.0), px(80.0)),
                    );
                    let second = Bounds::from_corners(
                        point(px(edge + 1.0), px(0.0)),
                        point(px(250.75), px(80.0)),
                    );
                    let a = snap_terminal_bounds(first, scale);
                    let line = snap_terminal_bounds(divider, scale);
                    let b = snap_terminal_bounds(second, scale);
                    assert_eq!(a.right(), line.left());
                    assert_eq!(line.right(), b.left());
                    assert!((a.right().as_f32() * scale).fract().abs() < 0.0001);
                }
            }
        }
        let tie = snap_terminal_bounds(
            Bounds::from_corners(point(px(-0.5), px(0.5)), point(px(3.5), px(4.5))),
            1.0,
        );
        assert_eq!(
            tie,
            Bounds::from_corners(point(px(0.0), px(0.0)), point(px(3.0), px(4.0)))
        );
    }
}
