use std::cell::RefCell;
use std::collections::HashMap;

use image::ImageEncoder;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, HICON, ICON_BIG, ICON_SMALL, IMAGE_FLAGS, SendMessageW,
    WM_SETICON,
};

struct Icon(HICON);

impl Drop for Icon {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

// Windows does not copy WM_SETICON handles. Retain one pair per catalog entry
// until the UI thread exits, including icons referenced by other open windows.
thread_local! {
    static ICONS: RefCell<HashMap<String, (Icon, Icon)>> = RefCell::new(HashMap::new());
}

fn create(image: &image::RgbaImage, size: u32) -> anyhow::Result<Icon> {
    let image = image::imageops::resize(image, size, size, image::imageops::FilterType::Lanczos3);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png).write_image(
        image.as_raw(),
        size,
        size,
        image::ExtendedColorType::Rgba8,
    )?;
    // Vista and later accept PNG icon resources. No bitmap/mask conversion
    // or premultiplication is needed, so translucent edges retain their alpha.
    Ok(Icon(unsafe {
        CreateIconFromResourceEx(
            &png,
            true,
            0x0003_0000,
            size as i32,
            size as i32,
            IMAGE_FLAGS(0),
        )?
    }))
}

pub(super) fn apply(window: &gpui::Window, id: &str) -> anyhow::Result<()> {
    let RawWindowHandle::Win32(handle) = window.window_handle()?.as_raw() else {
        return Ok(());
    };
    let hwnd = HWND(handle.hwnd.get() as *mut std::ffi::c_void);
    let (small, large) = ICONS.with(|cache| -> anyhow::Result<(HICON, HICON)> {
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(id) {
            let image = super::window_icon_image(id)?;
            let icons = (create(&image, 32)?, create(&image, 256)?);
            cache.insert(id.to_string(), icons);
        }
        let (small, large) = &cache[id];
        Ok((small.0, large.0))
    })?;
    // Native messages can re-enter window code. Release the cache borrow
    // first; the cache still owns the handles for every open window.
    unsafe {
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(small.0 as isize)),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(large.0 as isize)),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_accepts_every_catalog_png_as_small_and_large_icon_resources() {
        for choice in con_core::config::APP_ICONS {
            let image = crate::app_icon::window_icon_image(choice.id).unwrap();
            for size in [32, 256] {
                let icon = create(&image, size).unwrap();
                assert!(!icon.0.is_invalid(), "{} at {size}px", choice.id);
            }
        }
    }
}
