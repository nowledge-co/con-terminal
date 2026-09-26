use gpui::{Pixels, SharedString, TitlebarOptions, px};
#[cfg(target_os = "macos")]
use gpui_component::{TITLE_BAR_HEIGHT, TitleBar};

pub(super) const SETTINGS_HEADER_HEIGHT: Pixels = px(44.0);

pub(crate) fn settings_titlebar_options() -> TitlebarOptions {
    floating_titlebar_options("Settings".into())
}

/// Transparent-titlebar chrome for standalone secondary windows (Settings,
/// Agent Handoff): native traffic lights aligned to the 44px custom header.
pub(crate) fn floating_titlebar_options(title: SharedString) -> TitlebarOptions {
    TitlebarOptions {
        title: Some(title),
        appears_transparent: cfg!(target_os = "macos"),
        traffic_light_position: {
            #[cfg(target_os = "macos")]
            {
                TitleBar::title_bar_options()
                    .traffic_light_position
                    .map(|mut position| {
                        // GPUI expects the button frame's top inset, not its
                        // center or visible circle. It reapplies this on resize
                        // and when leaving fullscreen.
                        let height =
                            native_button_height().unwrap_or(TITLE_BAR_HEIGHT - position.y * 2.0);
                        position.y = (SETTINGS_HEADER_HEIGHT - height) / 2.0;
                        position
                    })
            }
            #[cfg(not(target_os = "macos"))]
            {
                None
            }
        },
    }
}

#[cfg(target_os = "macos")]
fn native_button_height() -> Option<Pixels> {
    use cocoa::appkit::{NSWindowButton, NSWindowStyleMask};
    use cocoa::base::id;
    use cocoa::foundation::NSRect;
    use objc::{class, msg_send, sel, sel_impl};

    // Called on the UI thread while opening Settings. AppKit supplies an
    // autoreleased button with the same style as the Settings window.
    unsafe {
        let style = NSWindowStyleMask::NSTitledWindowMask
            | NSWindowStyleMask::NSClosableWindowMask
            | NSWindowStyleMask::NSMiniaturizableWindowMask
            | NSWindowStyleMask::NSResizableWindowMask
            | NSWindowStyleMask::NSFullSizeContentViewWindowMask;
        let button: id = msg_send![class!(NSWindow),
            standardWindowButton: NSWindowButton::NSWindowCloseButton
            forStyleMask: style
        ];
        if button.is_null() {
            return None;
        }
        let frame: NSRect = msg_send![button, frame];
        (frame.size.height.is_finite() && frame.size.height > 0.0)
            .then(|| px(frame.size.height as f32))
    }
}
