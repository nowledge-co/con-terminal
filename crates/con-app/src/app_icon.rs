use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use con_core::config::{DEFAULT_APP_ICON, app_icon_asset, sanitize_app_icon};

static APPLIED_APP_ICON: Mutex<String> = Mutex::new(String::new());
static SAVED_APP_ICON: Mutex<String> = Mutex::new(String::new());
#[cfg(any(target_os = "macos", target_os = "linux", test))]
static SYNCED_FILE_ICON: Mutex<String> = Mutex::new(String::new());
static NEXT_PREVIEW_OWNER: AtomicU64 = AtomicU64::new(1);
static PREVIEW_STACK: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());

#[cfg(target_os = "macos")]
pub fn current_asset() -> &'static str {
    app_icon_asset(&current_id())
}

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

fn applied_id() -> String {
    mutex_id(&APPLIED_APP_ICON)
}

pub fn saved_id() -> String {
    mutex_id(&SAVED_APP_ICON)
}

/// Called only after config reaches disk, or when loading persisted config.
/// File icons must never follow an unsaved Dock preview.
pub fn remember_saved(id: &str) {
    let id = sanitize_app_icon(id);
    if let Ok(mut saved) = SAVED_APP_ICON.lock() {
        saved.clone_from(&id);
    }
}

/// Sync the installed icon without blocking the UI on desktop integration I/O.
/// Call after recording a successfully saved (or loaded) configuration.
pub fn sync_saved_file_icon(_cx: &gpui::App) {
    #[cfg(target_os = "macos")]
    _cx.background_executor()
        .spawn(async { sync_file_icon(set_bundle_icon) })
        .detach();
    #[cfg(target_os = "linux")]
    if !con_paths::is_flatpak() {
        _cx.background_executor()
            .spawn(async {
                sync_file_icon(|id| {
                    let path = con_paths::launcher_icon_path()
                        .ok_or_else(|| anyhow::anyhow!("user icon directory is unavailable"))?;
                    write_launcher_icon(&path, id)?;
                    native::refresh_launcher_cache(&path);
                    Ok(())
                })
            })
            .detach();
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn sync_file_icon(apply: impl FnOnce(&str) -> anyhow::Result<()>) {
    // Separate from APPLIED_APP_ICON: saving the already-previewed image
    // still needs a file update. Serialize NSWorkspace calls and cache only
    // successes, so an unwritable bundle or icon directory can be retried.
    let Ok(mut synced) = SYNCED_FILE_ICON.lock() else {
        return;
    };
    // Read after acquiring the write lock, not when scheduling the task:
    // out-of-order background tasks must never restore an older selection.
    let id = saved_id();
    if *synced == id {
        return;
    }
    match apply(&id) {
        Ok(()) => *synced = id,
        Err(err) => log::warn!("could not update installed app icon: {err:#}"),
    }
}

/// Unique token for a Settings panel that can own the live Dock preview.
pub fn new_preview_owner() -> u64 {
    NEXT_PREVIEW_OWNER.fetch_add(1, Ordering::Relaxed)
}

/// Apply a live preview and record which panel last changed the Dock.
///
/// Re-applying from the same panel updates its stack entry and moves it
/// to the top, even when the icon id is unchanged.
pub fn apply_preview(owner: u64, id: &str) {
    let id = sanitize_app_icon(id);
    apply_app_icon(&id);
    if let Ok(mut stack) = PREVIEW_STACK.lock() {
        stack.retain(|(existing, _)| *existing != owner);
        stack.push((owner, id));
    }
}

/// Restore the Dock when this panel releases its preview.
///
/// If another panel still has a live preview, that icon is promoted.
/// Only an empty stack falls back to the saved icon.
pub fn restore_saved_if_owner(owner: u64) {
    let next = {
        let Ok(mut stack) = PREVIEW_STACK.lock() else {
            return;
        };
        let was_top = stack.last().is_some_and(|(existing, _)| *existing == owner);
        stack.retain(|(existing, _)| *existing != owner);
        if !was_top {
            return;
        }
        stack.last().map(|(_, id)| id.clone())
    };
    match next {
        Some(id) => apply_app_icon(&id),
        None => apply_app_icon(&saved_id()),
    }
}

/// Drop this panel's preview after a successful save without touching the Dock.
///
/// If this panel was driving the Dock, older buried previews are also
/// dropped. The save just committed that image; promoting a previous
/// unsaved choice later would jump the Dock off the icon that was written.
pub fn clear_preview_owner(owner: u64) {
    if let Ok(mut stack) = PREVIEW_STACK.lock() {
        let was_top = stack.last().is_some_and(|(existing, _)| *existing == owner);
        if was_top {
            stack.clear();
        } else {
            stack.retain(|(existing, _)| *existing != owner);
        }
    }
}

/// Keep the process-wide saved icon when a non-Settings path writes config.
pub fn merge_saved_into(app_icon: &mut String) {
    *app_icon = saved_id();
}

/// Apply a saved icon at process start or when config is reloaded with no
/// windows, and clear any live preview stack.
pub fn apply_persisted(id: &str) {
    remember_saved(id);
    apply_app_icon(id);
    if let Ok(mut stack) = PREVIEW_STACK.lock() {
        stack.clear();
    }
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

/// Apply the running Dock / Cmd-Tab image, including unsaved previews.
/// The installed `.app` icon is synchronized separately after saving.
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

/// Update existing native windows after a preview or rollback. New windows
/// receive the current choice through the Root observer installed at startup.
pub fn sync_window_icons(_cx: &mut gpui::App) {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    _cx.defer(|cx| {
        // A picker click can run inside a window update; defer until that
        // window is back in App so it receives the change as well.
        for handle in cx.windows() {
            let _ = handle.update(cx, |_, window, _| apply_window_icon(window));
        }
    });
}

pub fn init(_cx: &mut gpui::App) {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    _cx.observe_new::<gpui_component::Root>(|_, window, _| {
        if let Some(window) = window {
            apply_window_icon(window);
        }
    })
    .detach();
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn apply_window_icon(window: &gpui::Window) {
    if let Err(error) = native::apply(window, &current_id()) {
        log::warn!("could not update window icon: {error:#}");
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn window_icon_image(id: &str) -> anyhow::Result<image::RgbaImage> {
    let asset = app_icon_asset(id);
    let bytes = crate::assets::png_bytes(asset)
        .ok_or_else(|| anyhow::anyhow!("app icon asset missing: {asset}"))?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)?.to_rgba8();
    Ok(image::imageops::resize(
        &image,
        256,
        256,
        image::imageops::FilterType::Lanczos3,
    ))
}

#[cfg(target_os = "windows")]
#[path = "app_icon/windows.rs"]
mod native;

#[cfg(target_os = "linux")]
#[path = "app_icon/linux.rs"]
mod native;

#[cfg(any(target_os = "linux", test))]
fn x11_icon_data(image: &image::RgbaImage) -> Vec<u32> {
    let mut data = Vec::with_capacity(2 + image.as_raw().len() / 4);
    data.extend([image.width(), image.height()]);
    data.extend(image.pixels().map(|pixel| {
        let [r, g, b, a] = pixel.0;
        u32::from_be_bytes([a, r, g, b])
    }));
    data
}

#[cfg(any(target_os = "linux", test))]
fn write_launcher_icon(path: &std::path::Path, id: &str) -> anyhow::Result<()> {
    use image::ImageEncoder;
    use std::io::Write;

    let image = window_icon_image(id)?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        image.as_raw(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
    )?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("icon path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let staged = parent.join(format!(".con-icon-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged)?;
        file.write_all(&png)?;
        file.sync_all()?;
        std::fs::rename(&staged, path)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(staged);
    result
}

#[cfg(test)]
#[path = "app_icon/ico.rs"]
mod ico;

#[cfg(target_os = "macos")]
fn set_bundle_icon(id: &str) -> anyhow::Result<()> {
    use cocoa::base::id as ObjcId;
    use cocoa::foundation::NSString;
    use objc::{class, msg_send, sel, sel_impl};

    objc::rc::autoreleasepool(|| unsafe {
        let bundle: ObjcId = msg_send![class!(NSBundle), mainBundle];
        let path: ObjcId = msg_send![bundle, bundlePath];
        let path = std::ffi::CStr::from_ptr(path.UTF8String()).to_str()?;
        let path = std::path::Path::new(path);
        // `cargo run` is not an application bundle. Never customize the
        // containing directory or search for another installed copy.
        if path.extension().is_some_and(|ext| ext == "app") {
            set_file_icon(path, id)?;
        }
        Ok(())
    })
}

#[cfg(target_os = "macos")]
fn set_file_icon(path: &std::path::Path, id: &str) -> anyhow::Result<()> {
    use cocoa::appkit::NSImage;
    use cocoa::base::{BOOL, YES, id as ObjcId, nil};
    use cocoa::foundation::{NSData, NSString};
    use objc::rc::{StrongPtr, autoreleasepool};
    use objc::{class, msg_send, sel, sel_impl};

    autoreleasepool(|| unsafe {
        let path = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("invalid bundle path"))?;
        let path = StrongPtr::new(NSString::alloc(nil).init_str(path));
        let image = if id == DEFAULT_APP_ICON {
            // nil removes the file's custom icon, revealing the signed
            // bundle's original resource without changing its contents.
            StrongPtr::new(nil)
        } else {
            let asset = app_icon_asset(id);
            let png = crate::assets::png_bytes(asset)
                .ok_or_else(|| anyhow::anyhow!("app icon asset missing: {asset}"))?;
            let data = NSData::dataWithBytes_length_(nil, png.as_ptr().cast(), png.len() as u64);
            let image = StrongPtr::new(NSImage::initWithData_(NSImage::alloc(nil), data));
            anyhow::ensure!(*image != nil, "invalid app icon image: {asset}");
            image
        };
        let workspace: ObjcId = msg_send![class!(NSWorkspace), sharedWorkspace];
        let success: BOOL = msg_send![workspace, setIcon: *image forFile: *path options: 0_u64];
        anyhow::ensure!(
            success == YES,
            "NSWorkspace rejected the app icon (bundle must be writable)"
        );
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn distribution_sources_match_the_default_catalog_asset() {
        for source in [
            include_str!("../build.rs"),
            include_str!("../Cargo.toml"),
            include_str!("../../../scripts/macos/common.sh"),
            include_str!("../../../scripts/linux/release.sh"),
            include_str!("../../../packaging/flatpak/aetherpak.yaml"),
        ] {
            assert!(source.contains(con_core::config::DEFAULT_APP_ICON_ASSET));
        }
    }

    #[test]
    fn launcher_icon_replaces_only_con_and_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("icons/co.nowledge.con.png");
        write_launcher_icon(&path, "default").unwrap();
        let classic = std::fs::read(&path).unwrap();
        let unrelated = path.with_file_name("another-app.png");
        std::fs::write(&unrelated, b"untouched").unwrap();
        write_launcher_icon(&path, DEFAULT_APP_ICON).unwrap();
        let current = std::fs::read(&path).unwrap();
        assert_ne!(classic, current);
        assert_eq!(image::load_from_memory(&current).unwrap().width(), 256);
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"untouched");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            2
        );
        let blocked = dir.path().join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        assert!(write_launcher_icon(&blocked, DEFAULT_APP_ICON).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), current);
    }

    #[test]
    fn native_icon_assets_and_packaging_preserve_alpha() {
        for choice in con_core::config::APP_ICONS {
            let image = window_icon_image(choice.id).unwrap();
            assert_eq!(image.dimensions(), (256, 256));
            assert!(image.pixels().any(|pixel| pixel[3] > 0));
        }
        let png = crate::assets::png_bytes(con_core::config::DEFAULT_APP_ICON_ASSET).unwrap();
        let bytes = ico::encode(&png).unwrap();
        assert_eq!(&bytes[..6], &[0, 0, 1, 0, 7, 0]);
        for (entry, size) in bytes[6..118]
            .chunks_exact(16)
            .zip([16, 24, 32, 48, 64, 128, 256])
        {
            assert_eq!(entry[0] as u32, size % 256);
            assert_eq!(entry[1], entry[0]);
            let len = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
            let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
            let image = image::load_from_memory(&bytes[offset..offset + len])
                .unwrap()
                .to_rgba8();
            assert_eq!(image.dimensions(), (size, size));
            assert_eq!(image.get_pixel(0, 0)[3], 0);
        }
    }

    #[test]
    fn x11_icon_pixels_are_straight_argb_not_premultiplied() {
        let image =
            image::RgbaImage::from_raw(2, 1, vec![240, 120, 60, 128, 1, 2, 3, 255]).unwrap();
        assert_eq!(x11_icon_data(&image), vec![2, 1, 0x80f0783c, 0xff010203]);
    }

    fn reset() -> std::sync::MutexGuard<'static, ()> {
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *APPLIED_APP_ICON.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
        *SAVED_APP_ICON.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
        *SYNCED_FILE_ICON.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
        PREVIEW_STACK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        guard
    }

    #[test]
    fn file_icon_tracks_saved_choices_not_dock_previews() {
        let _guard = reset();
        apply_persisted("raccoon-a1");
        sync_file_icon(|id| {
            assert_eq!(id, "raccoon-a1");
            Ok(())
        });
        assert_eq!(*SYNCED_FILE_ICON.lock().unwrap(), "raccoon-a1");

        apply_preview(1, "raccoon-b1");
        sync_file_icon(|_| panic!("preview must not change the file icon"));
        assert_eq!(*SYNCED_FILE_ICON.lock().unwrap(), "raccoon-a1");
        restore_saved_if_owner(1);
        assert_eq!(*SYNCED_FILE_ICON.lock().unwrap(), "raccoon-a1");

        apply_preview(2, "raccoon-c1");
        remember_saved("raccoon-c1");
        sync_file_icon(|id| {
            assert_eq!(id, "raccoon-c1");
            Ok(())
        });
        assert_eq!(*SYNCED_FILE_ICON.lock().unwrap(), "raccoon-c1");
        remember_saved("default");
        sync_file_icon(|id| {
            assert_eq!(id, "default");
            Ok(())
        });
        assert_eq!(*SYNCED_FILE_ICON.lock().unwrap(), "default");
    }

    #[test]
    fn file_icon_retries_failures_but_skips_successful_duplicates() {
        let _guard = reset();
        remember_saved("raccoon-a1");
        sync_file_icon(|_| anyhow::bail!("read-only bundle"));
        assert!(SYNCED_FILE_ICON.lock().unwrap().is_empty());
        let mut calls = 0;
        sync_file_icon(|_| {
            calls += 1;
            Ok(())
        });
        sync_file_icon(|_| panic!("duplicate file write"));
        assert_eq!(calls, 1);
        remember_saved("default");
        sync_file_icon(|_| {
            calls += 1;
            Ok(())
        });
        assert_eq!(calls, 2);
    }

    #[test]
    fn queued_sync_reads_latest_saved_icon_not_preview_or_old_save() {
        let _guard = reset();
        remember_saved("raccoon-a1");
        remember_saved("raccoon-b1");
        apply_preview(1, "raccoon-c1");
        sync_file_icon(|id| {
            assert_eq!(id, "raccoon-b1");
            Ok(())
        });
        sync_file_icon(|_| panic!("older queued task must not write again"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_file_icon_sets_and_clears_custom_icon_flag() {
        let _guard = reset();
        struct TestBundle(std::path::PathBuf);
        impl Drop for TestBundle {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let bundle = TestBundle(std::env::temp_dir().join(format!(
            "con-icon-{}-{}.app",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
        std::fs::create_dir(&bundle.0).unwrap();
        let has_custom_icon = || {
            let info = std::process::Command::new("/usr/bin/xattr")
                .args(["-px", "com.apple.FinderInfo"])
                .arg(&bundle.0)
                .output()
                .unwrap();
            // Finder flags are big-endian at offset 8; kHasCustomIcon = 0x0400.
            info.status.success()
                && String::from_utf8(info.stdout)
                    .unwrap()
                    .split_whitespace()
                    .nth(8)
                    .is_some_and(|byte| u8::from_str_radix(byte, 16).unwrap() & 4 != 0)
        };
        assert!(!has_custom_icon());
        set_file_icon(&bundle.0, "raccoon-suit-a6").unwrap();
        assert!(has_custom_icon());
        set_file_icon(&bundle.0, DEFAULT_APP_ICON).unwrap();
        assert!(!has_custom_icon());
        assert!(set_file_icon(&bundle.0.join("missing.app"), "raccoon-a1").is_err());
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

    #[test]
    fn restore_promotes_the_preceding_live_preview() {
        let _guard = reset();
        remember_saved("raccoon-a1");
        apply_preview(1, "raccoon-a2");
        apply_preview(2, "raccoon-b1");
        restore_saved_if_owner(2);
        assert_eq!(applied_id(), "raccoon-a2");
        restore_saved_if_owner(1);
        assert_eq!(applied_id(), "raccoon-a1");
    }

    #[test]
    fn merge_saved_into_keeps_the_process_wide_icon() {
        let _guard = reset();
        remember_saved("raccoon-a2");
        let mut stale = "raccoon-a1".to_string();
        merge_saved_into(&mut stale);
        assert_eq!(stale, "raccoon-a2");
    }

    #[test]
    fn saving_the_top_preview_does_not_leave_buried_entries() {
        let _guard = reset();
        remember_saved("raccoon-a1");
        apply_preview(1, "raccoon-a2");
        apply_preview(2, "raccoon-b1");
        remember_saved("raccoon-b1");
        clear_preview_owner(2);
        assert_eq!(applied_id(), "raccoon-b1");

        apply_preview(3, "raccoon-c1");
        restore_saved_if_owner(3);
        assert_eq!(applied_id(), "raccoon-b1");
    }

    #[test]
    fn saving_a_buried_preview_keeps_the_live_top() {
        let _guard = reset();
        remember_saved("raccoon-a1");
        apply_preview(1, "raccoon-a2");
        apply_preview(2, "raccoon-b1");
        remember_saved("raccoon-a2");
        clear_preview_owner(1);
        assert_eq!(applied_id(), "raccoon-b1");
        restore_saved_if_owner(2);
        assert_eq!(applied_id(), "raccoon-a2");
    }
}
