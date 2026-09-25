use super::*;

pub(super) fn terminal_separator_over_backdrop(backdrop: Hsla, theme: &Theme) -> Hsla {
    let overlay_alpha = if theme.is_dark() { 0.14 } else { 0.11 };
    backdrop
        .blend(theme.foreground.opacity(overlay_alpha))
        .alpha(1.0)
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)] // Keep existing production item ordering.
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn sidebar_max_width_reserves_main_content_and_caps_panel() {
        let window_width = TERMINAL_MIN_CONTENT_WIDTH + PANEL_MAX_WIDTH + 120.0;

        assert_eq!(max_sidebar_panel_width(window_width, 0.0), PANEL_MAX_WIDTH);

        let constrained_width = TERMINAL_MIN_CONTENT_WIDTH + 260.0;
        assert_eq!(max_sidebar_panel_width(constrained_width, 0.0), 260.0);

        let tiny_width = TERMINAL_MIN_CONTENT_WIDTH - 20.0;
        assert_eq!(max_sidebar_panel_width(tiny_width, 0.0), PANEL_MIN_WIDTH);
    }
}

pub(super) fn perf_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CON_GHOSTTY_PROFILE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

pub(super) fn chrome_tooltip(
    label: &str,
    stroke: Option<Keystroke>,
    window: &mut Window,
    cx: &mut App,
) -> AnyView {
    let label = label.to_string();
    Tooltip::element(move |_, cx| {
        let theme = cx.theme();
        let mut content = div().flex().items_center().gap(px(7.0)).child(
            div()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .text_color(theme.popover_foreground)
                .child(label.clone()),
        );

        if let Some(stroke) = stroke.as_ref() {
            content = content.child(crate::keycaps::keycaps_for_stroke(stroke, theme));
        }

        content
    })
    .build(window, cx)
}

pub(super) fn max_agent_panel_width(window_width: f32) -> f32 {
    (window_width - TERMINAL_MIN_CONTENT_WIDTH).max(AGENT_PANEL_MIN_WIDTH)
}

pub(super) fn max_sidebar_panel_width(window_width: f32, agent_panel_outer_width: f32) -> f32 {
    (window_width - agent_panel_outer_width - TERMINAL_MIN_CONTENT_WIDTH)
        .clamp(PANEL_MIN_WIDTH, PANEL_MAX_WIDTH)
}

/// Windows / Linux caption buttons (Min / Max+Restore / Close).
///
/// Each button is marked with `.window_control_area(..)` so GPUI's
/// platform layer hit-tests it during `WM_NCHITTEST` on Windows and
/// dispatches the OS-level action automatically. The X11 backend in
/// `gpui_linux` doesn't currently dispatch through that hit-test
/// path (`on_hit_test_window_control` is a no-op there), so on Linux
/// we additionally wire explicit `start_window_move` / `zoom_window`
/// / `minimize_window` / `remove_window` calls on click. The marker
/// is still set for future-proofing once the Linux backend grows
/// `_NET_WM_MOVERESIZE`-style server hit testing.
///
/// Uses Phosphor SVGs instead of Segoe Fluent Icons so the bar
/// renders identically on hosts where Segoe Fluent Icons isn't
/// installed (Win10 without the 2022 optional feature, Linux,
/// tests). Size and hover colors mirror Windows 11's native caption
/// buttons: 36px wide, 45px min height doesn't apply here (we honour
/// the shared `top_bar_height` instead), red hover on Close.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(super) fn caption_buttons(
    window: &Window,
    theme: &Theme,
    height: f32,
    // Linux Close needs a workspace handle so it can call
    // `prepare_window_close` (cancel sessions, flush state, drop
    // pending control responses) before yanking the window — same
    // shutdown path the macOS / Windows X-button hits via
    // `on_window_should_close`. Windows has its own caption-area
    // hit-test that runs through the workspace cleanup; on Linux
    // GPUI's X11 backend doesn't fire that path so we have to
    // route it explicitly.
    #[cfg(target_os = "linux")] workspace: gpui::WeakEntity<ConWorkspace>,
) -> impl IntoElement {
    #[cfg(target_os = "linux")]
    use gpui::MouseButton;
    use gpui::{Hsla, ParentElement, Rgba, Styled, WindowControlArea, div, px, svg};

    let close_red: Hsla = Rgba {
        r: 232.0 / 255.0,
        g: 17.0 / 255.0,
        b: 32.0 / 255.0,
        a: 1.0,
    }
    .into();
    let fg = theme.muted_foreground.opacity(0.9);
    let is_dark = theme.background.l < 0.5;
    let hover_bg = if is_dark {
        theme.foreground.opacity(0.11)
    } else {
        theme.foreground.opacity(0.075)
    };
    let hover_fg = theme.foreground.opacity(0.96);

    let button = |id: &'static str, icon: &'static str, area: WindowControlArea, close: bool| {
        let hover = if close { close_red } else { hover_bg };
        // All three glyphs rest at the same theme-muted foreground so
        // min/max/close read as one visual row. Only on hover does the
        // close glyph switch to white, paired with the red chip bg —
        // matches Windows 11 convention. Parent div declares itself as
        // a `group(id)` so the svg's `.group_hover(id, ...)` fires when
        // the 36px hit-target is hovered, not just the 10px icon ink.
        let hover_fg = if close { gpui::white() } else { hover_fg };
        let el = div()
            .id(id)
            .group(id)
            .flex()
            .items_center()
            .justify_center()
            // `.occlude()` is required so the parent top_bar's
            // `WindowControlArea::Drag` hit-test doesn't swallow these
            // child buttons on Windows (HTCLOSE/HTMAXBUTTON/HTMINBUTTON
            // won't fire without it). Matches Zed's platform_windows
            // caption-button implementation.
            .occlude()
            .w(px(36.0))
            .h(px(height))
            .flex_shrink_0()
            .window_control_area(area)
            .hover(move |s| s.bg(hover))
            .child(
                svg()
                    .path(icon)
                    .size(ui_icon_px(theme, 10.0))
                    .text_color(fg)
                    .group_hover(id, move |s| s.text_color(hover_fg)),
            );

        // Linux: GPUI's X11 hit-test doesn't fire `WindowControlArea`
        // dispatchers, so wire each button to its `Window` action by
        // hand. `on_mouse_down` matches macOS / Windows feel: the
        // action fires on the click-down edge rather than after the
        // up edge, which keeps the cluster snappy. Windows already
        // dispatches via the WindowControlArea hit-test set above —
        // no extra handler needed there.
        #[cfg(target_os = "linux")]
        let workspace_for_close = workspace.clone();
        #[cfg(target_os = "linux")]
        let el = el.on_mouse_down(MouseButton::Left, move |_, window, cx| match area {
            WindowControlArea::Min => window.minimize_window(),
            WindowControlArea::Max => window.zoom_window(),
            WindowControlArea::Close => {
                // Mirror the macOS / Windows close path: run the
                // workspace cleanup (cancel agent sessions, flush
                // session save, drop pending control responses,
                // shut down terminal surfaces) *before* the window
                // goes away. Without this, clicking the Linux CSD
                // close button bypasses agent cancellation and
                // pending control-request responses entirely.
                let _ = workspace_for_close.update(cx, |workspace, cx| {
                    workspace.prepare_window_close(cx);
                });
                window.remove_window();
            }
            _ => {}
        });

        el
    };

    let (max_icon, max_area) = if window.is_maximized() {
        ("phosphor/copy.svg", WindowControlArea::Max)
    } else {
        ("phosphor/square.svg", WindowControlArea::Max)
    };

    div()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .h(px(height))
        .child(button(
            "win-min",
            "phosphor/minus.svg",
            WindowControlArea::Min,
            false,
        ))
        .child(button("win-max", max_icon, max_area, false))
        .child(button(
            "win-close",
            "phosphor/x.svg",
            WindowControlArea::Close,
            true,
        ))
}
