use gpui::{
    Action, AnyElement, App, Context, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Window, div, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::menu::{PopupMenu, PopupMenuItem};

fn action_item(label: &'static str, action: Box<dyn Action>) -> PopupMenuItem {
    let render_action = action.boxed_clone();
    PopupMenuItem::element(move |window, cx| action_row(label, render_action.as_ref(), window, cx))
        .action(action)
}

fn action_row(
    label: &'static str,
    action: &dyn Action,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let shortcut = crate::keycaps::first_action_keystroke(action, window);

    div()
        .flex()
        .w_full()
        .min_w(px(188.0))
        .items_center()
        .justify_between()
        .gap(px(24.0))
        .font_family(theme.font_family.clone())
        .child(
            div()
                .min_w_0()
                .text_size(px(13.0))
                .line_height(px(18.0))
                .child(SharedString::from(label)),
        )
        .child(
            div().flex().min_w(px(54.0)).justify_end().children(
                shortcut.map(|stroke| crate::keycaps::keycaps_for_stroke(&stroke, theme)),
            ),
        )
        .into_any_element()
}

pub(crate) fn terminal_context_menu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    menu.min_w(px(220.0))
        .item(action_item("Paste", Box::new(crate::Paste)))
        .item(action_item("Copy", Box::new(crate::Copy)))
        .item(action_item("Ask AI", Box::new(crate::AskAi)))
        .item(action_item(
            "Clear Terminal",
            Box::new(crate::ClearTerminal),
        ))
        .separator()
        .submenu("Split Pane", window, cx, |menu, _window, _cx| {
            menu.min_w(px(164.0))
                .item(action_item("Right", Box::new(crate::SplitRight)))
                .item(action_item("Down", Box::new(crate::SplitDown)))
                .item(action_item("Left", Box::new(crate::SplitLeft)))
                .item(action_item("Up", Box::new(crate::SplitUp)))
        })
        .item(action_item(
            "Toggle Pane Zoom",
            Box::new(crate::TogglePaneZoom),
        ))
        .item(action_item("Close Pane", Box::new(crate::ClosePane)))
        .separator()
        .submenu("Surfaces", window, cx, |menu, _window, _cx| {
            menu.min_w(px(214.0))
                .item(action_item("New Surface Tab", Box::new(crate::NewSurface)))
                .item(action_item(
                    "New Surface Pane Right",
                    Box::new(crate::NewSurfaceSplitRight),
                ))
                .item(action_item(
                    "New Surface Pane Down",
                    Box::new(crate::NewSurfaceSplitDown),
                ))
                .separator()
                .item(action_item(
                    "Next Surface Tab",
                    Box::new(crate::NextSurface),
                ))
                .item(action_item(
                    "Previous Surface Tab",
                    Box::new(crate::PreviousSurface),
                ))
                .item(action_item(
                    "Rename Current",
                    Box::new(crate::RenameSurface),
                ))
                .item(action_item("Close Current", Box::new(crate::CloseSurface)))
        })
        .separator()
        .item(action_item("Focus Input", Box::new(crate::FocusInput)))
        .item(action_item(
            "Settings",
            Box::new(crate::settings_panel::ToggleSettings),
        ))
        .item(action_item(
            "Command Palette",
            Box::new(crate::command_palette::ToggleCommandPalette),
        ))
}
