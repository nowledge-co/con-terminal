use std::cell::RefCell;
use std::collections::HashMap;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
use x11rb::wrapper::ConnectionExt as _;

thread_local! {
    static ICONS: RefCell<HashMap<String, Vec<u32>>> = RefCell::new(HashMap::new());
}

pub(super) fn refresh_launcher_cache(path: &std::path::Path) {
    if let Some(theme_dir) = path.ancestors().nth(3) {
        match std::process::Command::new("gtk-update-icon-cache")
            .args(["-f", "-t", "-q"])
            .arg(theme_dir)
            .output()
        {
            Ok(output) if !output.status.success() => {
                log::debug!("desktop icon cache refresh failed: {}", output.status);
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                log::debug!("desktop icon cache refresh unavailable: {error}");
            }
            _ => {}
        }
    }
}

pub(super) fn apply(window: &gpui::Window, id: &str) -> anyhow::Result<()> {
    let xid = match window.window_handle()?.as_raw() {
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)?,
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        // Wayland identifies apps through their desktop file. Do not open an
        // unrelated XWayland connection or rewrite a launcher to fake support.
        _ => return Ok(()),
    };
    let (connection, _) = x11rb::connect(None)?;
    let atom = connection
        .intern_atom(false, b"_NET_WM_ICON")?
        .reply()?
        .atom;
    ICONS.with(|cache| -> anyhow::Result<()> {
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(id) {
            let image = super::window_icon_image(id)?;
            cache.insert(id.to_string(), super::x11_icon_data(&image));
        }
        connection
            .change_property32(PropMode::REPLACE, xid, atom, AtomEnum::CARDINAL, &cache[id])?
            .check()?;
        Ok(())
    })?;
    connection.flush()?;
    Ok(())
}
