use gpui::{App, ParentElement, Styled, svg};
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_component::{Disableable, Theme};

use crate::ui_scale::mono_space_px;

/// The bottom bar and agent composer share one submit affordance.
pub(crate) fn send_button(id: &'static str, enabled: bool, theme: &Theme, cx: &App) -> Button {
    let fill = if enabled {
        theme.foreground
    } else {
        theme.foreground.opacity(0.055)
    };
    let size = mono_space_px(theme, 28.0);
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(fill)
                .hover(if enabled {
                    theme.foreground.opacity(0.88)
                } else {
                    theme.foreground.opacity(0.055)
                })
                .active(theme.foreground.opacity(0.76)),
        )
        .tab_stop(false)
        .disabled(!enabled)
        .size(size)
        .w(size)
        .h(size)
        .p_0()
        // Custom variants soften their resting color; keep this action explicit.
        .bg(fill)
        .rounded_full()
        .flex_shrink_0()
        .child(
            svg()
                .path("phosphor/arrow-up.svg")
                .size(mono_space_px(theme, 14.0))
                .text_color(if enabled {
                    theme.background
                } else {
                    theme.muted_foreground
                }),
        )
}
