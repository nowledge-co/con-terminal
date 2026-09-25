use gpui::{App, Bounds, Pixels, Window, px, size};

use super::HEADER_HEIGHT;

/// Body vertical padding around the scrollable content (pt(10) + pb(16)).
const BODY_PADDING_Y: Pixels = px(26.0);
/// Footer button bar: py(12) + py(12) around a default gpui-component Button
/// (Size::Medium → h_8, 32px at the default 16px rem).
const FOOTER_HEIGHT: Pixels = px(56.0);
/// Lower bound so the dialog never collapses around a nearly empty body.
const MIN_WINDOW_HEIGHT: Pixels = px(320.0);

/// Pure height computation behind `fit_window_to_content`: measured body
/// content plus header, body padding and (optional) footer, clamped to
/// [MIN_WINDOW_HEIGHT, 85% of the screen] so the body's scroll fallback
/// stays reachable.
fn fitted_height(
    content_height: Pixels,
    has_footer: bool,
    screen_height: Option<Pixels>,
) -> Pixels {
    let footer = if has_footer {
        FOOTER_HEIGHT
    } else {
        Pixels::ZERO
    };
    let mut height = f32::from(HEADER_HEIGHT + BODY_PADDING_Y + content_height + footer)
        .max(f32::from(MIN_WINDOW_HEIGHT));
    if let Some(screen) = screen_height {
        height = height.min(f32::from(screen) * 0.85);
    }
    px(height)
}

/// Fit the dialog height to the body content measured at prepaint time, so
/// every state that adds or removes rows (picker, model section, candidates,
/// job, error/busy lines) resizes the window without per-state
/// magic-number deltas. `children_bounds` are window-relative layout bounds
/// (from `window.layout_bounds`), not content coordinates, but the
/// differential `last.bottom − first.top` is invariant to scroll offset
/// because all children shift by the same amount when the scroll container
/// scrolls. Resizes are idempotent: a sub-pixel delta is ignored to avoid
/// a resize/layout feedback loop.
pub(super) fn fit_window_to_content(
    children: Vec<Bounds<Pixels>>,
    has_footer: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let (Some(first), Some(last)) = (children.first(), children.last()) else {
        return;
    };
    let height = fitted_height(
        last.bottom() - first.top(),
        has_footer,
        window
            .display(cx)
            .map(|display| display.bounds().size.height),
    );
    let current = window.window_bounds().get_bounds().size;
    if (f32::from(current.height) - f32::from(height)).abs() > 1.0 {
        window.resize(size(current.width, height));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitted_height_adds_chrome_and_footer() {
        // 44 header + 26 body padding + 200 content + 56 footer.
        assert_eq!(fitted_height(px(200.0), true, None), px(326.0));
        // Without a footer the same content is 56px shorter.
        assert_eq!(fitted_height(px(300.0), false, None), px(370.0));
    }

    #[test]
    fn fitted_height_respects_floor_and_screen_cap() {
        // Nearly empty content bottoms out at the minimum height.
        assert_eq!(fitted_height(px(0.0), false, None), MIN_WINDOW_HEIGHT);
        // Tall content is capped at 85% of the screen so the body keeps
        // its scroll fallback.
        assert_eq!(fitted_height(px(2000.0), true, Some(px(1000.0))), px(850.0));
    }
}
