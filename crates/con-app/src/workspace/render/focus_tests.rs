use super::workspace_focus_container;
use gpui::{
    Context, FocusHandle, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window,
    div, px,
};

struct FocusProbe {
    workspace: FocusHandle,
    terminal: FocusHandle,
    input: FocusHandle,
}

impl Render for FocusProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        workspace_focus_container(&self.workspace)
            .flex_col()
            .w(px(200.0))
            .child(
                div()
                    .id("chrome")
                    .debug_selector(|| "chrome".into())
                    .h(px(28.0)),
            )
            .child(
                div()
                    .id("terminal")
                    .debug_selector(|| "terminal".into())
                    .h(px(80.0))
                    .track_focus(&self.terminal),
            )
            .child(
                div()
                    .id("divider")
                    .debug_selector(|| "divider".into())
                    .h(px(6.0)),
            )
            .child(
                div()
                    .id("input")
                    .debug_selector(|| "input".into())
                    .h(px(40.0))
                    .track_focus(&self.input),
            )
    }
}

#[gpui::test]
fn layout_chrome_preserves_input_focus_and_children_can_still_take_focus(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|window, cx| {
        let terminal = cx.focus_handle();
        terminal.focus(window, cx);
        FocusProbe {
            workspace: cx.focus_handle(),
            terminal,
            input: cx.focus_handle(),
        }
    });
    for input in [false, true] {
        cx.update(|window, cx| {
            let probe = view.read(cx);
            let focus = if input { &probe.input } else { &probe.terminal }.clone();
            focus.focus(window, cx);
        });
        for selector in ["chrome", "divider"] {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_click(bounds.center(), gpui::Modifiers::default());
            cx.update(|window, cx| {
                let probe = view.read(cx);
                assert!(
                    (if input { &probe.input } else { &probe.terminal }).is_focused(window),
                    "{selector} stole the input focus"
                );
            });
        }
    }
    for selector in ["terminal", "input"] {
        let bounds = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(bounds.center(), gpui::Modifiers::default());
        cx.update(|window, cx| {
            let probe = view.read(cx);
            assert!(
                (if selector == "input" {
                    &probe.input
                } else {
                    &probe.terminal
                })
                .is_focused(window)
            );
        });
    }
    cx.update(|window, cx| {
        let focus = view.read(cx).workspace.clone();
        focus.focus(window, cx);
        assert!(
            focus.is_focused(window),
            "editor-only workspace focus remains available"
        );
    });
}

#[gpui::test]
fn column_resize_targets_cover_gutters_outside_clipped_panels(cx: &mut gpui::TestAppContext) {
    use gpui::{Modifiers, point};
    struct Gutters {
        hits: [usize; 2],
    }
    impl Render for Gutters {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div().pl(px(30.0)).pt(px(20.0)).child(
                div()
                    .relative()
                    .w(px(600.0))
                    .h(px(200.0))
                    .child(
                        div()
                            .absolute()
                            .left(px(400.0))
                            .w(px(200.0))
                            .h_full()
                            .overflow_hidden()
                            .occlude(),
                    )
                    .children(
                        [
                            ("left-sidebar-resize-handle", 202.5),
                            ("agent-panel-divider", 398.5),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(i, (id, center))| {
                            super::column_resize_handle(id, center, false, gpui::black(), false)
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(move |this, _, _, _| {
                                        this.hits[i] += 1;
                                    }),
                                )
                        }),
                    ),
            )
        }
    }
    let (view, cx) = cx.add_window_view(|_, _| Gutters { hits: [0; 2] });
    for (i, (id, center)) in [
        ("left-sidebar-resize-handle", 202.5),
        ("agent-panel-divider", 398.5),
    ]
    .into_iter()
    .enumerate()
    {
        let bounds = cx.debug_bounds(id).unwrap();
        assert_eq!(bounds.size.width, px(7.0));
        assert!((bounds.center().x.as_f32() - (30.0 + center)).abs() <= 0.5);
        // Both edges must deliver mouse-down even where the target overlaps
        // the adjacent clipped/occluded panel. Do not test only its centre.
        for x in [bounds.left() + px(0.5), bounds.right() - px(0.5)] {
            cx.simulate_click(point(x, bounds.center().y), Modifiers::default());
        }
        view.read_with(cx, |probe, _| assert_eq!(probe.hits[i], 2));
    }
}
