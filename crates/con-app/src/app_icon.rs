use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use con_core::config::{DEFAULT_APP_ICON, app_icon_asset, sanitize_app_icon};

static APPLIED_APP_ICON: Mutex<String> = Mutex::new(String::new());
static SAVED_APP_ICON: Mutex<String> = Mutex::new(String::new());
static NEXT_PREVIEW_OWNER: AtomicU64 = AtomicU64::new(1);
static PREVIEW_OWNER: Mutex<Option<u64>> = Mutex::new(None);

#[cfg(target_os = "macos")]
pub fn current_asset() -> &'static str {
    app_icon_asset(&current_id())
}

#[cfg(target_os = "macos")]
fn current_id() -> String {
    applied_id()
}

fn mutex_id(slot: &Mutex<String>) -> String {
    slot.lock()
        .ok()
        .filter(|id| !id.is_empty())
        .map(|id| id.clone())
        .unwrap_or_else(|| DEFAULT_APP_ICON.to_string())
}

pub fn applied_id() -> String {
    mutex_id(&APPLIED_APP_ICON)
}

pub fn saved_id() -> String {
    mutex_id(&SAVED_APP_ICON)
}

pub fn remember_saved(id: &str) {
    let id = sanitize_app_icon(id);
    if let Ok(mut saved) = SAVED_APP_ICON.lock() {
        saved.clone_from(&id);
    }
}

/// Unique token for a Settings panel that can own the live Dock preview.
pub fn new_preview_owner() -> u64 {
    NEXT_PREVIEW_OWNER.fetch_add(1, Ordering::Relaxed)
}

/// Apply a live preview and record which panel last changed the Dock.
///
/// Updates ownership even when the icon id is unchanged, so a second
/// Settings window that previews the same id becomes the owner.
pub fn apply_preview(owner: u64, id: &str) {
    apply_app_icon(id);
    set_preview_owner(Some(owner));
}

/// Restore the saved Dock icon only if this panel still owns the preview.
///
/// Two Settings windows can preview the same unsaved id. Matching on that
/// id would let either Discard cancel the other panel's preview.
pub fn restore_saved_if_owner(owner: u64) {
    if preview_owner() != Some(owner) {
        return;
    }
    set_preview_owner(None);
    apply_app_icon(&saved_id());
}

/// Drop preview ownership after a successful save without touching the Dock.
pub fn clear_preview_owner(owner: u64) {
    if preview_owner() == Some(owner) {
        set_preview_owner(None);
    }
}

fn preview_owner() -> Option<u64> {
    PREVIEW_OWNER.lock().ok().and_then(|owner| *owner)
}

fn set_preview_owner(owner: Option<u64>) {
    if let Ok(mut current) = PREVIEW_OWNER.lock() {
        *current = owner;
    }
}

/// Apply a saved icon at process start or when config is reloaded with no
/// windows, and clear any live preview owner.
pub fn apply_persisted(id: &str) {
    remember_saved(id);
    apply_app_icon(id);
    set_preview_owner(None);
}

/// If this panel changed `app_icon` since its snapshot, that value wins.
/// Otherwise keep the last saved icon so a stale window cannot clobber it.
/// Does not update process-wide saved state; call [`remember_saved`] after
/// the config actually reaches disk.
pub fn take_for_save(panel_id: &str, snapshot_id: Option<&str>) -> String {
    let panel = sanitize_app_icon(panel_id);
    match snapshot_id {
        Some(snapshot) if sanitize_app_icon(snapshot) == panel => saved_id(),
        _ => panel,
    }
}

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
        if icon == nil {
            return;
        }
        // PNGs otherwise report their pixel size in points, so a 512px
        // asset fills the Dock tile like an Electron/SPA icon. 128pt is
        // the usual Dock representation size.
        let _: () = msg_send![icon, setSize: cocoa::foundation::NSSize::new(128.0, 128.0)];
        NSApp().setApplicationIconImage_(icon);
        // alloc/init is +1. NSApp retains the image, so drop our ownership
        // or each switch leaks the previous 512px NSImage until exit.
        let _: () = msg_send![icon, release];
    });
}

#[cfg(not(target_os = "macos"))]
fn set_application_icon_image(_png: &[u8]) {}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn reset() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap();
        *APPLIED_APP_ICON.lock().unwrap() = String::new();
        *SAVED_APP_ICON.lock().unwrap() = String::new();
        set_preview_owner(None);
        guard
    }

    #[test]
    fn take_for_save_merges_stale_panels_and_keeps_explicit_changes() {
        let _guard = reset();
        remember_saved("raccoon-a1");
        assert_eq!(take_for_save("default", Some("default")), "raccoon-a1");
        assert_eq!(saved_id(), "raccoon-a1");

        assert_eq!(take_for_save("raccoon-c1", Some("default")), "raccoon-c1");
        assert_eq!(saved_id(), "raccoon-a1");

        remember_saved("raccoon-c1");
        assert_eq!(saved_id(), "raccoon-c1");
    }

    #[test]
    fn restore_saved_only_when_this_owner_last_applied() {
        let _guard = reset();
        remember_saved("raccoon-a1");
        apply_preview(1, "raccoon-a2");
        apply_preview(2, "raccoon-a2");
        restore_saved_if_owner(1);
        assert_eq!(applied_id(), "raccoon-a2");
        restore_saved_if_owner(2);
        assert_eq!(applied_id(), "raccoon-a1");
    }
}
