use std::sync::Mutex;

use con_core::config::{app_icon_asset, sanitize_app_icon};

static APPLIED_APP_ICON: Mutex<String> = Mutex::new(String::new());

/// Apply the selected app icon. On macOS this updates the Dock and Cmd-Tab
/// image for the running process. Finder / Launchpad still use the bundled
/// `.app` icon until alternate bundle icons are added.
pub fn apply_app_icon(id: &str) {
    let id = sanitize_app_icon(id);
    if let Ok(mut current) = APPLIED_APP_ICON.lock() {
        if *current == id {
            return;
        }
        current.clone_from(&id);
    }

    let asset = app_icon_asset(&id);
    let Some(bytes) = crate::assets::png_bytes(asset) else {
        log::warn!("app icon asset missing: {asset}");
        return;
    };
    set_application_icon_image(bytes.as_ref());
}

#[cfg(target_os = "macos")]
fn set_application_icon_image(png: &[u8]) {
    use cocoa::appkit::{NSApp, NSApplication, NSImage};
    use cocoa::base::nil;
    use cocoa::foundation::NSData;
    use objc::rc::autoreleasepool;
    use objc::{msg_send, sel, sel_impl};

    autoreleasepool(|| unsafe {
        let data = NSData::dataWithBytes_length_(
            nil,
            png.as_ptr() as *const std::ffi::c_void,
            png.len() as u64,
        );
        let icon = NSImage::initWithData_(NSImage::alloc(nil), data);
        // PNGs otherwise report their pixel size in points, so a 512px
        // asset fills the Dock tile like an Electron/SPA icon. 128pt is
        // the usual Dock representation size.
        let _: () = msg_send![icon, setSize: cocoa::foundation::NSSize::new(128.0, 128.0)];
        NSApp().setApplicationIconImage_(icon);
    });
}

#[cfg(not(target_os = "macos"))]
fn set_application_icon_image(_png: &[u8]) {}
