use con_core::session::TabAccentColor;
use gpui::{App, Hsla};
use gpui_component::ActiveTheme;

pub(crate) const TAB_ACCENT_ACTIVE_ALPHA: f32 = 0.35;
pub(crate) const TAB_ACCENT_INACTIVE_ALPHA: f32 = 0.15;
pub(crate) const TAB_ACCENT_INACTIVE_HOVER_ALPHA: f32 = 0.22;

/// Map a `TabAccentColor` to an `Hsla` for rendering dots and tab backgrounds.
pub(crate) fn tab_accent_color_hsla(color: TabAccentColor, cx: &App) -> Hsla {
    let is_dark = cx.theme().is_dark();
    let (h, s, ll, ld) = match color {
        TabAccentColor::Red => (0.0, 0.82, 0.52, 0.60),
        TabAccentColor::Orange => (25.0, 0.88, 0.52, 0.60),
        TabAccentColor::Yellow => (46.0, 0.88, 0.48, 0.58),
        TabAccentColor::Green => (142.0, 0.60, 0.42, 0.52),
        TabAccentColor::Teal => (174.0, 0.60, 0.40, 0.50),
        TabAccentColor::Blue => (214.0, 0.80, 0.52, 0.62),
        TabAccentColor::Purple => (270.0, 0.70, 0.52, 0.62),
        TabAccentColor::Pink => (330.0, 0.78, 0.56, 0.64),
        // Unknown is a forward-compat catch-all; render as neutral green.
        TabAccentColor::Unknown => (142.0, 0.40, 0.45, 0.55),
    };
    gpui::hsla(h / 360.0, s, if is_dark { ld } else { ll }, 1.0)
}

/// Render a user-assigned tab accent as a low- or high-emphasis chrome surface.
pub(crate) fn tab_accent_surface_hsla(color: TabAccentColor, alpha: f32, cx: &App) -> Hsla {
    let mut h = tab_accent_color_hsla(color, cx);
    h.a = alpha;
    h
}
