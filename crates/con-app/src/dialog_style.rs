//! Shared action hierarchy for con-authored dialogs; retain component behavior.
use gpui::{App, FontWeight, Styled};
use gpui_component::{
    ActiveTheme, Sizable,
    button::{Button, ButtonCustomVariant, ButtonVariants},
};

pub(crate) trait DialogButtonStyle {
    fn dialog_primary(self, cx: &App) -> Self;
    fn dialog_secondary(self, cx: &App) -> Self;
    fn dialog_destructive(self, cx: &App) -> Self;
}

fn action(button: Button, cx: &App, destructive: bool, primary: bool) -> Button {
    let theme = cx.theme();
    let ink = if destructive {
        theme.danger
    } else if primary {
        theme.primary
    } else {
        theme.foreground
    };
    let fill = if primary {
        if theme.is_dark() { 0.18 } else { 0.10 }
    } else {
        if theme.is_dark() { 0.075 } else { 0.045 }
    };
    button
        .small()
        .custom(
            ButtonCustomVariant::new(cx)
                .color(ink.opacity(fill))
                .foreground(ink)
                .hover(ink.opacity(fill + 0.035))
                .active(ink.opacity(fill + 0.07))
                .shadow(false),
        )
        .h(crate::ui_scale::ui_px(theme, 32.0))
        .px(crate::ui_scale::ui_px(theme, 12.0))
        .rounded(crate::ui_scale::ui_px(theme, 8.0))
        .font_weight(if primary {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
}

impl DialogButtonStyle for Button {
    fn dialog_primary(self, cx: &App) -> Self {
        action(self, cx, false, true)
    }
    fn dialog_secondary(self, cx: &App) -> Self {
        action(self, cx, false, false)
    }
    fn dialog_destructive(self, cx: &App) -> Self {
        action(self, cx, true, false)
    }
}
