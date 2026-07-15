// Release builds on Windows use the GUI subsystem so double-clicking
// the exe (or launching it from Explorer / a shortcut) doesn't spawn a
// stray console window alongside the GPUI window. Debug builds keep
// the console subsystem so `cargo wrun` in a terminal still prints
// env_logger output and panic traces inline.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
// Suppress warnings from objc 0.2's `sel_impl!` and `class!` macros
// checking `cfg(feature = "cargo-clippy")`.
#![allow(unexpected_cfgs)]

mod agent_panel;
mod assets;
mod chat_markdown;
#[cfg(target_os = "macos")]
mod cli_shim;
mod command_palette;
#[cfg(target_os = "macos")]
mod global_hotkey;
#[cfg(target_os = "macos")]
mod macos_windowing;
#[cfg(target_os = "macos")]
mod quick_terminal;

// The terminal-view module is selected per platform:
//   macOS   -> ghostty_view.rs (libghostty + child NSView)
//   Windows -> windows_view.rs (libghostty-vt + ConPTY + D3D11 child HWND)
//   Linux   -> linux_view.rs (Unix PTY + libghostty-vt + GPUI paint path)
//   other   -> stub_view.rs
//
// All three expose the same public type names (`GhosttyView`,
// `GhosttyTitleChanged`, `GhosttyProcessExited`, `GhosttyFocusChanged`,
// `GhosttySplitRequested`, `init`) so downstream modules
// (`terminal_pane`, `workspace`) compile on every target without
// per-callsite cfg gates. See `docs/impl/windows-port.md` and
// `docs/impl/linux-port.md`.
#[cfg(target_os = "macos")]
mod ghostty_view;
#[cfg(target_os = "windows")]
#[path = "windows_view.rs"]
mod ghostty_view;
#[cfg(target_os = "linux")]
#[path = "linux_view.rs"]
mod ghostty_view;
#[cfg(all(
    not(target_os = "macos"),
    not(target_os = "windows"),
    not(target_os = "linux")
))]
#[path = "stub_view.rs"]
mod ghostty_view;

mod activity_bar;
mod editor_buffer;
mod editor_lsp;
mod editor_syntax;
mod editor_view;
mod file_tree_view;
mod input_bar;
mod keycaps;
mod model_registry;
mod motion;
mod pane_tree;
mod settings_panel;
mod sidebar;
mod sidebar_search_view;
mod tab_colors;
mod tab_context_menu;
mod terminal_context_menu;
mod terminal_env;
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod terminal_ime;
mod terminal_keys;
mod terminal_links;
mod terminal_pane;
mod terminal_paste;
mod terminal_restore;
mod terminal_shortcuts;
mod theme;
mod ui_scale;
mod updater;
mod workspace;

use std::any::TypeId;

use con_core::config::KeybindingConfig;
use con_core::session::{
    GlobalHistoryState, PaneLayoutState, Session, SurfaceState, WORKSPACE_ERROR_SURFACE_OWNER,
};
use con_core::workspace_layout::WorkspaceLayout;
use gpui::*;
use gpui_component::ActiveTheme;
#[cfg(target_os = "macos")]
use gpui_component::link::Link;
use workspace::ConWorkspace;

actions!(
    con,
    [
        ShowAbout,
        Quit,
        NewWindow,
        NewTab,
        NextTab,
        PreviousTab,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectTab9,
        NextWindow,
        PreviousWindow,
        ToggleSummon,
        ToggleQuickTerminal,
        ToggleAgentPanel,
        ToggleInputBar,
        CollapseSidebar,
        CloseTab,
        ClosePane,
        TogglePaneZoom,
        SplitRight,
        SplitDown,
        SplitLeft,
        SplitUp,
        ClearTerminal,
        ClearRestoredTerminalHistory,
        ExportWorkspaceLayout,
        AddWorkspaceLayoutTabs,
        OpenWorkspaceLayoutWindow,
        NewSurface,
        NewSurfaceSplitRight,
        NewSurfaceSplitDown,
        NextSurface,
        PreviousSurface,
        RenameSurface,
        CloseSurface,
        FocusInput,
        AskAi,
        CycleInputMode,
        TogglePaneScopePicker,
        ToggleLeftPanel,
        FocusFiles,
        SearchFiles,
        CheckForUpdates,
        EditorMoveLeft,
        EditorMoveRight,
        EditorMoveUp,
        EditorMoveDown,
        EditorMoveHome,
        EditorMoveEnd,
        EditorMoveLineStart,
        EditorMoveLineEnd,
        EditorSelectLeft,
        EditorSelectRight,
        EditorSelectUp,
        EditorSelectDown,
        EditorSelectHome,
        EditorSelectEnd,
        EditorDeleteBackward,
        EditorDeleteForward,
        EditorInsertNewline,
        EditorSave,
        HideApp,
        HideOtherApps,
        ShowAllApps,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        SelectAll,
        Minimize
    ]
);

/// Set the macOS dock icon at runtime (for `cargo run` — bundled apps use Info.plist).
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use cocoa::appkit::{NSApp, NSApplication, NSImage};
    use cocoa::base::nil;
    use cocoa::foundation::NSData;
    use objc::rc::autoreleasepool;

    const ICON_PNG: &[u8] = include_bytes!("../../../assets/Con-macOS-Dark-256x256@2x.png");

    autoreleasepool(|| unsafe {
        let data = NSData::dataWithBytes_length_(
            nil,
            ICON_PNG.as_ptr() as *const std::ffi::c_void,
            ICON_PNG.len() as u64,
        );
        let icon = NSImage::initWithData_(NSImage::alloc(nil), data);
        NSApp().setApplicationIconImage_(icon);
    });
}

#[cfg(not(target_os = "macos"))]
fn set_dock_icon() {}

#[cfg(target_os = "linux")]
fn linux_uses_client_side_decorations() -> bool {
    true
}

#[cfg(target_os = "macos")]
fn supports_transparent_main_window() -> bool {
    use cocoa::base::nil;
    use objc::{class, msg_send, sel, sel_impl};

    #[repr(C)]
    struct NSOperatingSystemVersion {
        major_version: isize,
        minor_version: isize,
        patch_version: isize,
    }

    unsafe {
        let process_info: cocoa::base::id = msg_send![class!(NSProcessInfo), processInfo];
        if process_info == nil {
            return true;
        }
        let version: NSOperatingSystemVersion = msg_send![process_info, operatingSystemVersion];
        version.major_version >= 13
    }
}

#[cfg(not(target_os = "macos"))]
#[cfg(target_os = "windows")]
fn supports_transparent_main_window() -> bool {
    true
}

#[cfg(target_os = "linux")]
fn supports_transparent_main_window() -> bool {
    // Linux keeps Con's integrated client-side chrome so titlebar
    // actions match macOS/Windows. Wayland uses the transparent GPUI
    // surface directly; X11 additionally receives a SHAPE mask.
    linux_uses_client_side_decorations()
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn supports_transparent_main_window() -> bool {
    false
}

/// Enable a Windows 11 DWM backdrop on the top-level window.
///
/// `blur=true` selects `DWMSBT_TRANSIENTWINDOW` (Acrylic, real Gaussian
/// blur of what's behind the window). `blur=false` selects
/// `DWMSBT_MAINWINDOW` (Mica, a static desktop-image-tinted fill — no
/// blur, much cheaper). Both render only when the GPUI Root above is
/// transparent.
///
/// Returns `true` when the DWM attribute was accepted — we use that
/// signal to keep the GPUI Root transparent so the backdrop shows
/// through. On Windows 10 / older Win11 / RDP / Arm32 / any host where
/// DwmSetWindowAttribute rejects `DWMWA_SYSTEMBACKDROP_TYPE`, we fall
/// back to an opaque theme fill in `open_con_window` (otherwise the
/// compositor shows the desktop through the window, which is what the
/// user observed when a modal hid the pane HWND).
#[cfg(target_os = "windows")]
fn apply_windows_backdrop(window: &mut Window, blur: bool) -> bool {
    set_windows_backdrop(window, blur).is_some()
}

/// Live re-apply of the DWM backdrop type. Called from the workspace
/// theme-update path so toggling "background blur" in settings switches
/// between Acrylic and Mica without restarting the window.
#[cfg(target_os = "windows")]
pub fn set_windows_backdrop_blur(window: &mut Window, blur: bool) {
    let _ = set_windows_backdrop(window, blur);
}

/// Live re-apply of the Linux background appearance. Mirrors
/// `set_windows_backdrop_blur` so the workspace theme-update path
/// can toggle "background blur" in settings without restarting the
/// window.
///
/// Linux uses a transparent top-level surface so Con's integrated
/// client-side chrome can define the visible rounded shape.
#[cfg(target_os = "linux")]
pub fn set_linux_window_blur(window: &mut Window, _blur: bool) {
    use gpui::WindowBackgroundAppearance;
    window.set_background_appearance(WindowBackgroundAppearance::Transparent);
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LinuxWindowShapeRadii {
    pub top_left: u32,
    pub top_right: u32,
    pub bottom_right: u32,
    pub bottom_left: u32,
}

#[cfg(target_os = "linux")]
pub(crate) fn sync_linux_x11_window_shape(
    window: &mut Window,
    width_px: u32,
    height_px: u32,
    radii: LinuxWindowShapeRadii,
) -> bool {
    let Some(window_id) = linux_x11_window_id(window) else {
        return false;
    };
    let Some(rectangles) = linux_window_shape_rectangles(width_px, height_px, radii) else {
        return false;
    };

    if let Err(err) = apply_linux_x11_shape(window_id, &rectangles) {
        log::debug!("failed to apply Linux X11 window shape: {err}");
        return false;
    }

    true
}

#[cfg(target_os = "linux")]
fn linux_x11_window_id(window: &Window) -> Option<u32> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok().filter(|id| *id != 0),
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn linux_window_shape_rectangles(
    width_px: u32,
    height_px: u32,
    radii: LinuxWindowShapeRadii,
) -> Option<Vec<x11rb::protocol::xproto::Rectangle>> {
    use x11rb::protocol::xproto::Rectangle;

    let width = u16::try_from(width_px).ok()?;
    let height = u16::try_from(height_px).ok()?;
    if width == 0 || height == 0 {
        return None;
    }

    let full_rect = || {
        vec![Rectangle {
            x: 0,
            y: 0,
            width,
            height,
        }]
    };

    let max_dimension = i16::MAX as u16;
    if width > max_dimension || height > max_dimension {
        return Some(full_rect());
    }

    let max_radius = u32::from(width / 2).min(u32::from(height / 2));
    let radii = LinuxWindowShapeRadii {
        top_left: radii.top_left.min(max_radius),
        top_right: radii.top_right.min(max_radius),
        bottom_right: radii.bottom_right.min(max_radius),
        bottom_left: radii.bottom_left.min(max_radius),
    };
    if radii == LinuxWindowShapeRadii::default() {
        return Some(full_rect());
    }

    let mut rows: Vec<(u16, u16, u16)> = Vec::with_capacity(usize::from(height));
    for y in 0..height {
        let y_u32 = u32::from(y);
        let left_inset =
            corner_row_inset(y_u32, u32::from(height), radii.top_left, radii.bottom_left)
                .min(width / 2);
        let right_inset = corner_row_inset(
            y_u32,
            u32::from(height),
            radii.top_right,
            radii.bottom_right,
        )
        .min(width / 2);
        let row_width = width.saturating_sub(left_inset.saturating_add(right_inset));
        if row_width > 0 {
            rows.push((y, left_inset, row_width));
        }
    }

    let mut rectangles: Vec<Rectangle> = Vec::with_capacity(rows.len());
    for (y, inset, row_width) in rows {
        if let Some(last) = rectangles.last_mut() {
            let same_x = last.x == inset as i16;
            let same_width = last.width == row_width;
            let contiguous = last.y as u16 + last.height == y;
            if same_x && same_width && contiguous {
                last.height = last.height.saturating_add(1);
                continue;
            }
        }
        rectangles.push(Rectangle {
            x: inset as i16,
            y: y as i16,
            width: row_width,
            height: 1,
        });
    }

    Some(rectangles)
}

#[cfg(target_os = "linux")]
fn corner_row_inset(y: u32, height: u32, top_radius: u32, bottom_radius: u32) -> u16 {
    if top_radius > 0 && y < top_radius {
        let radius = top_radius as f32;
        rounded_corner_inset(radius, radius - y as f32 - 0.5)
    } else if bottom_radius > 0 && y >= height.saturating_sub(bottom_radius) {
        let radius = bottom_radius as f32;
        rounded_corner_inset(radius, y as f32 - (height - bottom_radius) as f32 + 0.5)
    } else {
        0
    }
}

#[cfg(target_os = "linux")]
fn rounded_corner_inset(radius: f32, distance_from_center: f32) -> u16 {
    let inside = (radius * radius - distance_from_center * distance_from_center).max(0.0);
    (radius - inside.sqrt()).ceil().max(0.0) as u16
}

#[cfg(target_os = "linux")]
fn apply_linux_x11_shape(
    window_id: u32,
    rectangles: &[x11rb::protocol::xproto::Rectangle],
) -> Result<(), Box<dyn std::error::Error>> {
    use std::cell::RefCell;

    use x11rb::connection::Connection;
    use x11rb::protocol::shape::{ConnectionExt as _, SK, SO};
    use x11rb::protocol::xproto::ClipOrdering;
    use x11rb::rust_connection::RustConnection;

    thread_local! {
        static X11_SHAPE_CONNECTION: RefCell<Option<RustConnection>> = const { RefCell::new(None) };
    }

    X11_SHAPE_CONNECTION.with(|connection| {
        let mut connection = connection.borrow_mut();
        if connection.is_none() {
            let (new_connection, _) = x11rb::connect(None)?;
            *connection = Some(new_connection);
        }

        let conn = connection.as_ref().expect("shape connection initialized");
        conn.shape_rectangles(
            SO::SET,
            SK::BOUNDING,
            ClipOrdering::UNSORTED,
            window_id,
            0,
            0,
            rectangles,
        )?;
        conn.shape_rectangles(
            SO::SET,
            SK::CLIP,
            ClipOrdering::UNSORTED,
            window_id,
            0,
            0,
            rectangles,
        )?;
        conn.flush()?;
        Ok(())
    })
}

#[cfg(target_os = "macos")]
fn should_use_macos_window_glass_backdrop(blur: bool, effective_opacity: f32) -> bool {
    blur && effective_opacity < 0.999
}

#[cfg(target_os = "macos")]
pub fn set_macos_window_glass_backdrop(window: &mut Window, blur: bool, effective_opacity: f32) {
    if !supports_transparent_main_window() {
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
        return;
    }

    window.set_background_appearance(
        if should_use_macos_window_glass_backdrop(blur, effective_opacity) {
            WindowBackgroundAppearance::Blurred
        } else {
            WindowBackgroundAppearance::Transparent
        },
    );
}

#[cfg(target_os = "windows")]
fn set_windows_backdrop(window: &mut Window, blur: bool) -> Option<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DWMSBT_MAINWINDOW, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DwmSetWindowAttribute,
    };

    let handle = HasWindowHandle::window_handle(window).ok()?;
    let hwnd = match handle.as_raw() {
        RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut std::ffi::c_void),
        _ => return None,
    };

    // NB: we deliberately do NOT call DwmExtendFrameIntoClientArea
    // here. That call is the documented path for legacy GDI / WPF
    // windows and it works well on a Win32-composed window, but GPUI's
    // top-level window is composited through DirectComposition. Mixing
    // the two made the whole window render opaque (see
    // Screenshot 2026-04-22 at 13.14.09). Acrylic/Mica with a DComp
    // client requires a `DCompositionCreateBackdropBrush` visual in
    // GPUI's tree; until that lands upstream, we keep the attribute
    // set so the non-client strip reflects the user's choice and the
    // terminal stays transparent.
    // SAFETY: hwnd is a live top-level window (we just received it
    // from GPUI's window-handle surface); DWMWA_SYSTEMBACKDROP_TYPE
    // takes a 4-byte integer by pointer.
    let try_apply = |value: i32, label: &'static str| -> windows::core::Result<()> {
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &value as *const i32 as *const std::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            )
        }
        .map(|()| log::info!("Windows: applied DWM {label} backdrop on main window"))
    };

    if blur {
        // Try Acrylic first; if the OS rejects it (older Win11 builds,
        // disabled effects), fall back to Mica so the window stays
        // transparent instead of going opaque.
        if try_apply(DWMSBT_TRANSIENTWINDOW.0, "Acrylic").is_ok() {
            return Some(());
        }
        log::info!("Windows: Acrylic backdrop unavailable; trying Mica fallback");
    }
    match try_apply(DWMSBT_MAINWINDOW.0, "Mica") {
        Ok(()) => Some(()),
        Err(err) => {
            log::info!(
                "Windows: Mica backdrop unavailable ({err:?}); \
                 falling back to opaque theme fill"
            );
            None
        }
    }
}

// On non-macOS targets, con now branches by platform: Windows has the
// shipped local backend, Linux has an in-progress local PTY + VT path,
// and any remaining targets still fall back to `stub_view.rs`. See the
// platform docs under `docs/impl/`.

fn default_window_options(config: &con_core::Config, cx: &mut App) -> WindowOptions {
    let transparent = supports_transparent_main_window();
    let window_decorations = default_window_decorations();
    let window_background = default_window_background(config, transparent);

    WindowOptions {
        window_bounds: Some(default_workspace_window_bounds(cx)),
        titlebar: default_titlebar_options(transparent),
        window_decorations,
        window_background,
        // macOS: disable NSWindow's native `isMovable` drag so that
        // GPUI elements (tabs, sidebar) can start their own drags
        // without the window also moving. The top_bar renders an
        // explicit `start_window_move()` on mouse-down in the empty
        // titlebar area to restore the drag-to-move gesture.
        #[cfg(target_os = "macos")]
        is_movable: false,
        #[cfg(target_os = "linux")]
        app_id: Some("co.nowledge.con".to_owned()),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
fn quick_terminal_bounds(cx: &mut App) -> WindowBounds {
    let fallback_display = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
    let fallback_bounds = Bounds::new(
        fallback_display.origin,
        size(
            fallback_display.size.width,
            fallback_display.size.height / 2.0,
        ),
    );

    let Some(visible) = quick_terminal::active_display_visible_bounds()
        .or_else(|| cx.primary_display().map(|display| display.visible_bounds()))
    else {
        return WindowBounds::Windowed(fallback_bounds);
    };

    let bounds = Bounds::new(
        visible.origin,
        size(visible.size.width, visible.size.height / 2.0),
    );
    WindowBounds::Windowed(bounds)
}

#[cfg(target_os = "macos")]
fn quick_terminal_options(config: &con_core::Config, cx: &mut App) -> WindowOptions {
    let mut options = default_window_options(config, cx);
    options.window_bounds = Some(quick_terminal_bounds(cx));
    // Avoid a transient GPUI titlebar before the AppKit trampoline
    // normalizes the window to borderless.
    options.titlebar = None;
    options
}

fn default_window_background(
    config: &con_core::Config,
    transparent: bool,
) -> WindowBackgroundAppearance {
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = config;

    if !transparent {
        return WindowBackgroundAppearance::Opaque;
    }

    #[cfg(target_os = "macos")]
    {
        let effective_opacity =
            ConWorkspace::effective_terminal_opacity(config.appearance.terminal_opacity);
        if should_use_macos_window_glass_backdrop(
            config.appearance.terminal_blur,
            effective_opacity,
        ) {
            return WindowBackgroundAppearance::Blurred;
        }
    }

    #[cfg(target_os = "linux")]
    {
        let _ = config;
        WindowBackgroundAppearance::Transparent
    }

    #[cfg(not(target_os = "linux"))]
    {
        WindowBackgroundAppearance::Transparent
    }
}

fn default_workspace_window_bounds(cx: &mut App) -> WindowBounds {
    let fallback_size = size(px(1200.0), px(800.0));
    let fallback_bounds = Bounds::centered(None, fallback_size, cx);
    let fallback_display_bounds = cx.primary_display().map(|display| display.visible_bounds());

    let active_bounds = cx.active_window().and_then(|handle| {
        handle
            .update(cx, |_, window, cx| {
                let bounds = match window.window_bounds() {
                    WindowBounds::Windowed(bounds) => bounds,
                    WindowBounds::Maximized(_) | WindowBounds::Fullscreen(_) => {
                        return None;
                    }
                };
                let display_bounds = window
                    .display(cx)
                    .map(|display| display.visible_bounds())
                    .or(fallback_display_bounds);
                Some((bounds, display_bounds))
            })
            .ok()
            .flatten()
    });

    let Some((base_bounds, display_bounds)) = active_bounds else {
        return WindowBounds::Windowed(fallback_bounds);
    };

    const CASCADE_OFFSET: f32 = 28.0;

    let proposed_origin = base_bounds.origin + point(px(CASCADE_OFFSET), px(CASCADE_OFFSET));
    let proposed_bounds = Bounds::new(proposed_origin, base_bounds.size);
    let Some(display_bounds) = display_bounds else {
        return WindowBounds::Windowed(proposed_bounds);
    };

    let display_right = display_bounds.origin.x + display_bounds.size.width;
    let display_bottom = display_bounds.origin.y + display_bounds.size.height;
    let window_right = proposed_bounds.origin.x + proposed_bounds.size.width;
    let window_bottom = proposed_bounds.origin.y + proposed_bounds.size.height;

    let fits_horizontally = window_right <= display_right;
    let fits_vertically = window_bottom <= display_bottom;
    let final_origin = match (fits_horizontally, fits_vertically) {
        (true, true) => proposed_origin,
        (false, true) => point(display_bounds.origin.x, proposed_origin.y),
        (true, false) => point(proposed_origin.x, display_bounds.origin.y),
        (false, false) => display_bounds.origin,
    };

    WindowBounds::Windowed(Bounds::new(final_origin, base_bounds.size))
}

fn default_titlebar_options(transparent: bool) -> Option<TitlebarOptions> {
    Some(TitlebarOptions {
        title: Some("con".into()),
        appears_transparent: transparent,
        ..Default::default()
    })
}

fn default_window_decorations() -> Option<WindowDecorations> {
    #[cfg(target_os = "linux")]
    {
        if linux_uses_client_side_decorations() {
            Some(WindowDecorations::Client)
        } else {
            Some(WindowDecorations::Server)
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

pub(crate) fn open_con_window(
    config: con_core::Config,
    session: Session,
    exit_on_error: bool,
    cx: &mut App,
) {
    let window_options = default_window_options(&config, cx);
    cx.spawn(async move |cx| {
        if let Err(err) = cx.open_window(window_options, |window, cx| {
            let restored_session = session.clone();
            let view = cx
                .new(|cx| ConWorkspace::from_session(config.clone(), restored_session, window, cx));
            #[cfg(target_os = "windows")]
            let mica_applied = apply_windows_backdrop(window, config.appearance.terminal_blur);

            #[cfg(target_os = "linux")]
            set_linux_window_blur(window, config.appearance.terminal_blur);
            cx.new(|cx| {
                #[cfg(target_os = "windows")]
                let background = if mica_applied {
                    cx.theme().transparent
                } else {
                    cx.theme().background
                };
                #[cfg(target_os = "macos")]
                let background = cx.theme().transparent;
                #[cfg(target_os = "linux")]
                let background = {
                    if matches!(window.window_decorations(), Decorations::Client { .. }) {
                        cx.theme().transparent
                    } else {
                        cx.theme().background
                    }
                };
                #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
                let background = cx.theme().background;
                gpui_component::Root::new(view, window, cx).bg(background)
            })
        }) {
            if exit_on_error {
                eprintln!("Fatal: failed to open window: {err}");
                std::process::exit(1);
            } else {
                log::error!("Failed to open window: {err}");
            }
        }
    })
    .detach();
}

#[cfg(target_os = "macos")]
pub(crate) fn open_quick_terminal(config: con_core::Config, session: Session, cx: &mut App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let window_options = quick_terminal_options(&config, cx);
    cx.spawn(async move |cx| {
        if let Err(err) = cx.open_window(window_options, |window, cx| {
            let restored_session = session.clone();
            let view = cx.new(|cx| {
                let mut workspace =
                    ConWorkspace::from_session(config.clone(), restored_session, window, cx);
                workspace.mark_as_quick_terminal();
                workspace
            });

            let raw_ptr = HasWindowHandle::window_handle(window)
                .ok()
                .and_then(|handle| match handle.as_raw() {
                    RawWindowHandle::AppKit(handle) => Some(handle.ns_view.as_ptr().cast()),
                    _ => None,
                })
                .and_then(crate::quick_terminal::window_from_view_ptr);
            if let Some(raw_ptr) = raw_ptr {
                crate::quick_terminal::store_window_ptr(raw_ptr);
            } else {
                crate::quick_terminal::opening_failed();
                log::error!("Failed to resolve quick terminal native window");
            }

            cx.new(|cx| gpui_component::Root::new(view, window, cx).bg(cx.theme().transparent))
        }) {
            crate::quick_terminal::opening_failed();
            log::error!("Failed to open quick terminal: {err}");
        }
    })
    .detach();
}

pub(crate) fn fresh_window_session_with_history() -> Session {
    fresh_window_session_with_history_for_cwd(None)
}

pub(crate) fn fresh_window_session_with_history_for_cwd(
    cwd: Option<std::path::PathBuf>,
) -> Session {
    let persisted = Session::load().unwrap_or_default();
    let persisted_history = GlobalHistoryState::load().unwrap_or_default();
    let mut session = Session::default();

    session.global_shell_history = if persisted_history.global_shell_history.is_empty() {
        persisted.global_shell_history
    } else {
        persisted_history.global_shell_history
    };
    session.input_history = if !persisted_history.input_history.is_empty() {
        persisted_history
            .input_history
            .into_iter()
            .filter_map(|entry| {
                let command = entry.trim();
                (!command.is_empty()).then(|| command.to_string())
            })
            .collect()
    } else if persisted.input_history.is_empty() {
        session
            .global_shell_history
            .iter()
            .filter_map(|entry| {
                let command = entry.command.trim();
                (!command.is_empty()).then(|| command.to_string())
            })
            .collect()
    } else {
        persisted
            .input_history
            .into_iter()
            .filter_map(|entry| {
                let command = entry.trim();
                (!command.is_empty()).then(|| command.to_string())
            })
            .collect()
    };

    if let Some(cwd) = cwd {
        let cwd = cwd.to_string_lossy().to_string();
        if let Some(tab) = session.tabs.first_mut() {
            tab.cwd = Some(cwd.clone());
            if let Some(pane) = tab.panes.first_mut() {
                pane.cwd = Some(cwd);
            }
        }
    }

    session
}

fn fallback_cwd_for_workspace_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    if path.is_dir() {
        return Some(path.to_path_buf());
    }
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(|parent| {
            parent
                .ancestors()
                .find(|ancestor| !ancestor.as_os_str().is_empty() && ancestor.is_dir())
                .map(std::path::Path::to_path_buf)
        })
}

fn workspace_path_error_session(path: &std::path::Path, err: &anyhow::Error) -> Session {
    let mut session =
        fresh_window_session_with_history_for_cwd(fallback_cwd_for_workspace_path(path));
    let screen_text = vec![
        "Con could not open the requested workspace profile.".to_string(),
        format!("Path: {}", path.display()),
        format!("Reason: {err}"),
        "Opened a fresh shell instead. Fix the profile and run con <path> again.".to_string(),
        String::new(),
    ];

    if let Some(tab) = session.tabs.first_mut() {
        tab.title = "Workspace Error".to_string();
        tab.user_label = Some("Workspace Error".to_string());
        tab.focused_pane_id = Some(0);
        let cwd = tab
            .cwd
            .clone()
            .or_else(|| tab.panes.first().and_then(|pane| pane.cwd.clone()));
        tab.layout = Some(PaneLayoutState::Leaf {
            pane_id: 0,
            cwd: cwd.clone(),
            active_surface_id: Some(0),
            surfaces: vec![SurfaceState {
                surface_id: 0,
                title: Some("Shell".to_string()),
                // Synthetic Con-owned diagnostic surface. Keep it out of
                // human/subagent owner namespaces used by the orchestrator.
                owner: Some(WORKSPACE_ERROR_SURFACE_OWNER.to_string()),
                cwd,
                close_pane_when_last: false,
                screen_text,
            }],
        });
    }

    session
}

fn startup_path_argument() -> Option<std::path::PathBuf> {
    std::env::args_os().skip(1).find_map(|arg| {
        let text = arg.to_string_lossy();
        (!text.starts_with('-')).then(|| std::path::PathBuf::from(arg))
    })
}

pub(crate) fn session_from_workspace_layout_path(
    path: impl AsRef<std::path::Path>,
) -> anyhow::Result<Session> {
    let path = path.as_ref();
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    if path.is_file() {
        return session_from_workspace_layout_file(path);
    }

    if path.is_dir() {
        let layout_path = WorkspaceLayout::default_path_for_root(&path);
        if layout_path.exists() {
            return session_from_workspace_layout_file(layout_path);
        }
        return Ok(fresh_window_session_with_history_for_cwd(Some(path)));
    }

    anyhow::bail!("workspace path does not exist: {}", path.display())
}

fn startup_session() -> Session {
    if let Some(path) = startup_path_argument() {
        match session_from_workspace_layout_path(&path) {
            Ok(session) => {
                log::info!("opening workspace path {}", path.display());
                return session;
            }
            Err(err) => {
                log::warn!("failed to open workspace path {}: {err}", path.display());
                return workspace_path_error_session(&path, &err);
            }
        }
    }

    if live_control_endpoint_exists() {
        log::info!(
            "existing con control endpoint detected; opening a fresh window session with shared history"
        );
        fresh_window_session_with_history()
    } else {
        Session::load().unwrap_or_default()
    }
}

pub(crate) fn workspace_layout_root_for_file(path: &std::path::Path) -> std::path::PathBuf {
    let Some(parent) = path.parent() else {
        return std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    };

    if parent.file_name().and_then(|name| name.to_str()) == Some(".con")
        && let Some(project_root) = parent.parent()
    {
        return project_root.to_path_buf();
    }

    parent.to_path_buf()
}

pub(crate) fn session_from_workspace_layout_file(
    path: impl AsRef<std::path::Path>,
) -> anyhow::Result<Session> {
    let path = path.as_ref();
    let layout = WorkspaceLayout::load(path)?;
    let profile_root = workspace_layout_root_for_file(path);
    let layout_root = if layout.root.trim().is_empty() || layout.root == "." {
        profile_root
    } else {
        let root = std::path::Path::new(&layout.root);
        if root.is_absolute() {
            root.to_path_buf()
        } else {
            profile_root.join(root)
        }
    };
    let history = GlobalHistoryState::load().unwrap_or_default();
    layout.to_session(layout_root, Some(&history))
}

#[cfg(unix)]
fn live_control_endpoint_exists() -> bool {
    let path = con_core::control_socket_path();
    let addr = match socket2::SockAddr::unix(&path) {
        Ok(addr) => addr,
        Err(err) => {
            log::debug!(
                "skipping con control endpoint probe for invalid socket path {}: {err}",
                path.display()
            );
            return false;
        }
    };

    let socket = match socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None) {
        Ok(socket) => socket,
        Err(err) => {
            log::debug!("failed to create con control endpoint probe socket: {err}");
            return false;
        }
    };

    match socket.connect_timeout(&addr, std::time::Duration::from_millis(75)) {
        Ok(()) => true,
        Err(err) => {
            log::debug!(
                "con control endpoint probe did not find a live server at {}: {err}",
                path.display()
            );
            false
        }
    }
}

#[cfg(windows)]
fn live_control_endpoint_exists() -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::core::PCWSTR;

    let path = con_core::control_socket_path();
    let pipe_name: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    if unsafe { WaitNamedPipeW(PCWSTR(pipe_name.as_ptr()), 75).as_bool() } {
        true
    } else {
        log::debug!(
            "con control endpoint probe did not find a live pipe at {} within 75ms",
            path.display()
        );
        false
    }
}

#[cfg(not(any(unix, windows)))]
fn live_control_endpoint_exists() -> bool {
    false
}

pub(crate) fn toggle_global_summon(cx: &mut App) {
    let frontmost_window = cx
        .window_stack()
        .and_then(|windows| windows.first().cloned());
    let has_windows = frontmost_window.is_some();

    #[cfg(target_os = "macos")]
    let app_is_active = global_hotkey::is_app_active();
    #[cfg(not(target_os = "macos"))]
    let app_is_active = cx.active_window().is_some();

    if !has_windows {
        let config = con_core::Config::load().unwrap_or_default();
        open_con_window(config, fresh_window_session_with_history(), false, cx);
        cx.activate(true);
        return;
    }

    if app_is_active {
        cx.hide();
    } else {
        cx.activate(true);
        if let Some(window_handle) = frontmost_window {
            let _ = cx.update_window(window_handle, |_, window, _| {
                window.activate_window();
            });
        }
    }
}

fn minimize_frontmost_window(cx: &mut App) {
    let frontmost_window = cx.active_window().or_else(|| {
        cx.window_stack()
            .and_then(|windows| windows.first().cloned())
    });

    if let Some(window_handle) = frontmost_window {
        let _ = cx.update_window(window_handle, |_, window, _| {
            window.minimize_window();
        });
    }
}

#[cfg(target_os = "macos")]
fn bundle_info_value(key: &'static [u8]) -> Option<String> {
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CStr;

    unsafe {
        let bundle: *mut objc::runtime::Object = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() {
            return None;
        }
        let info: *mut objc::runtime::Object = msg_send![bundle, infoDictionary];
        if info.is_null() {
            return None;
        }
        let key_ns: *mut objc::runtime::Object =
            msg_send![class!(NSString), stringWithUTF8String: key.as_ptr()];
        if key_ns.is_null() {
            return None;
        }
        let value: *mut objc::runtime::Object = msg_send![info, objectForKey: key_ns];
        if value.is_null() {
            return None;
        }
        let utf8: *const std::os::raw::c_char = msg_send![value, UTF8String];
        if utf8.is_null() {
            return None;
        }
        CStr::from_ptr(utf8).to_str().ok().map(ToOwned::to_owned)
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn app_display_version() -> String {
    #[cfg(target_os = "macos")]
    let version = bundle_info_value(b"CFBundleShortVersionString\0")
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    // Non-macOS builds pick up the full tag-derived version (e.g.
    // `0.1.0-beta.31`) from `CON_RELEASE_VERSION`, which the release
    // pipeline exports before `cargo build`. Cargo.toml is frozen at
    // `0.1.0` so `CARGO_PKG_VERSION` alone loses the prerelease suffix.
    #[cfg(not(target_os = "macos"))]
    let version = option_env!("CON_RELEASE_VERSION")
        .map(str::to_owned)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    version
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn app_build_number() -> String {
    #[cfg(target_os = "macos")]
    let build = bundle_info_value(b"CFBundleVersion\0")
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    #[cfg(not(target_os = "macos"))]
    let build = option_env!("CON_RELEASE_VERSION")
        .map(str::to_owned)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    build
}

#[cfg(target_os = "linux")]
fn apply_linux_platform_override() {
    let Some(requested) = std::env::var("CON_LINUX_BACKEND")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    match requested.as_str() {
        "x11" => {
            if std::env::var_os("DISPLAY").is_some() {
                unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
                log::info!("Linux compositor override: forcing X11 backend");
            } else {
                log::warn!(
                    "CON_LINUX_BACKEND=x11 requested, but DISPLAY is not set; keeping default compositor detection"
                );
            }
        }
        "wayland" => {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                unsafe { std::env::remove_var("DISPLAY") };
                log::info!("Linux compositor override: forcing Wayland backend");
            } else {
                log::warn!(
                    "CON_LINUX_BACKEND=wayland requested, but WAYLAND_DISPLAY is not set; keeping default compositor detection"
                );
            }
        }
        other => {
            log::warn!(
                "Ignoring unsupported CON_LINUX_BACKEND={other:?}; expected \"x11\" or \"wayland\""
            );
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_linux_platform_override() {}

// About-panel helpers and window: only wired in to the menu on macOS
// (Sparkle lives in the same menu, and the Cocoa "About con" gesture
// hits this path). The functions stay platform-neutral so they're
// trivially reusable when a cross-platform About window lands.
#[cfg(target_os = "macos")]
fn about_panel_name() -> String {
    #[cfg(target_os = "macos")]
    {
        bundle_info_value(b"CFBundleDisplayName\0")
            .or_else(|| bundle_info_value(b"CFBundleName\0"))
            .unwrap_or_else(|| "con".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        "con".to_string()
    }
}

#[cfg(target_os = "macos")]
fn about_panel_version_detail() -> String {
    let version = app_display_version();
    let build = app_build_number();
    let channel = con_core::release_channel::current().display_name();

    if build.is_empty() || build == version {
        match con_core::release_channel::current() {
            con_core::release_channel::ReleaseChannel::Dev => "Development Build".to_string(),
            _ => channel.to_string(),
        }
    } else {
        format!("Build {build} • {channel}")
    }
}

#[cfg(target_os = "macos")]
struct AboutView {
    app_name: String,
    version: String,
    version_detail: String,
}

#[cfg(target_os = "macos")]
impl Render for AboutView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let repo_url = "https://github.com/nowledge-co/con-terminal";
        let detail_surface = theme.secondary_active.opacity(0.34);
        let quiet_text = theme.foreground.opacity(0.64);
        let muted_text = theme.foreground.opacity(0.48);

        div()
            .size_full()
            .bg(theme.background)
            .px(px(28.0))
            .py(px(28.0))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(14.0))
                    .child(
                        div().size(px(88.0)).p(px(2.0)).child(
                            img("Con-macOS-Dark-256x256@2x.png")
                                .size_full()
                                .object_fit(ObjectFit::Contain),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .max_w(px(310.0))
                            .gap(px(5.0))
                            .child(
                                div()
                                    .text_size(px(26.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(self.app_name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(14.0))
                                    .line_height(relative(1.35))
                                    .text_align(TextAlign::Center)
                                    .text_color(quiet_text)
                                    .child("A GPU-accelerated terminal with AI harness."),
                            ),
                    )
                    .child(
                        div()
                            .px(px(12.0))
                            .py(px(7.0))
                            .rounded(px(999.0))
                            .bg(detail_surface)
                            .font_family(theme.mono_font_family.clone())
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground.opacity(0.8))
                            .child(format!("{} • {}", self.version, self.version_detail)),
                    )
                    .child(
                        div().flex().flex_col().items_center().gap(px(6.0)).child(
                            Link::new("about-repo")
                                .href(repo_url)
                                .text_size(px(12.5))
                                .font_family(theme.mono_font_family.clone())
                                .child(repo_url),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(muted_text)
                            .child("Copyright © 2026 Nowledge Labs, LLC"),
                    ),
            )
    }
}

#[cfg(target_os = "macos")]
fn show_about_window(cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(432.0), px(388.0)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("About {}", about_panel_name()).into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };

    let app_name = about_panel_name();
    let version = app_display_version();
    let version_detail = about_panel_version_detail();

    if let Err(err) = cx.open_window(options, move |window, cx| {
        let about = cx.new(|_| AboutView {
            app_name: app_name.clone(),
            version: version.clone(),
            version_detail: version_detail.clone(),
        });
        cx.new(|cx| gpui_component::Root::new(about, window, cx).bg(cx.theme().background))
    }) {
        log::error!("Failed to open About window: {err}");
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[cfg_attr(not(test), allow(dead_code))]
enum BindingScope {
    Global,
    Input,
    EditorView,
}

impl BindingScope {
    fn name(self) -> Option<&'static str> {
        match self {
            BindingScope::Global => None,
            BindingScope::Input => Some("Input"),
            BindingScope::EditorView => Some("EditorView"),
        }
    }
}

#[derive(Clone)]
struct BindingSpec {
    key: SharedString,
    action: TypeId,
    scope: BindingScope,
    to_key_binding: fn(&str, Option<&str>) -> KeyBinding,
    action_name: fn() -> SharedString,
}

impl std::fmt::Debug for BindingSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BindingSpec")
            .field("key", &self.key)
            .field("action", &self.action)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl BindingSpec {
    fn new<A: Action + Default + Clone + 'static>(
        key: impl Into<SharedString>,
        scope: BindingScope,
    ) -> Self {
        fn make<A: Action + Default + Clone + 'static>(
            key: &str,
            scope: Option<&str>,
        ) -> KeyBinding {
            KeyBinding::new(key, A::default(), scope)
        }
        fn action_name<A: Action + Default + Clone + 'static>() -> SharedString {
            gpui::Action::name(&A::default()).into()
        }

        Self {
            key: key.into(),
            action: TypeId::of::<A>(),
            scope,
            to_key_binding: make::<A>,
            action_name: action_name::<A>,
        }
    }

    fn to_key_binding(&self) -> KeyBinding {
        (self.to_key_binding)(self.key.as_ref(), self.scope.name())
    }

    fn action_name(&self) -> SharedString {
        (self.action_name)()
    }

    fn to_unbind_key_binding(&self) -> KeyBinding {
        KeyBinding::new(
            self.key.as_ref(),
            gpui::Unbind(self.action_name()),
            self.scope.name(),
        )
    }
}

const GLOBAL_SCOPES: &[BindingScope] = &[BindingScope::Global];
const EDITOR_SCOPES: &[BindingScope] = &[BindingScope::EditorView];
#[cfg_attr(not(test), allow(dead_code))]
const APP_OVERRIDE_SCOPES: &[BindingScope] = &[
    BindingScope::Global,
    BindingScope::Input,
    BindingScope::EditorView,
];

fn push_scoped<A: Action + Default + Clone + 'static>(
    specs: &mut Vec<BindingSpec>,
    key: &str,
    scopes: &[BindingScope],
) {
    if key.trim().is_empty() {
        return;
    }

    for scope in scopes {
        specs.push(BindingSpec::new::<A>(key.to_string(), *scope));
    }
}

fn push_global<A: Action + Default + Clone + 'static>(specs: &mut Vec<BindingSpec>, key: &str) {
    push_scoped::<A>(specs, key, GLOBAL_SCOPES);
}

fn push_editor<A: Action + Default + Clone + 'static>(specs: &mut Vec<BindingSpec>, key: &str) {
    push_scoped::<A>(specs, key, EDITOR_SCOPES);
}

#[cfg_attr(not(test), allow(dead_code))]
fn push_app_override<A: Action + Default + Clone + 'static>(
    specs: &mut Vec<BindingSpec>,
    key: &str,
) {
    push_scoped::<A>(specs, key, APP_OVERRIDE_SCOPES);
}

fn configurable_app_binding_specs(kb: &KeybindingConfig) -> Vec<BindingSpec> {
    let mut specs = Vec::new();

    // App-level shortcuts are global by default. GPUI `scope = None` is the
    // broad binding; add explicit per-context overrides only for proven cases.
    push_global::<Quit>(&mut specs, &kb.quit);
    push_global::<NewWindow>(&mut specs, &kb.new_window);
    push_global::<NewTab>(&mut specs, &kb.new_tab);
    push_global::<NextTab>(&mut specs, &kb.next_tab);
    push_global::<PreviousTab>(&mut specs, &kb.previous_tab);
    push_global::<ToggleAgentPanel>(&mut specs, &kb.toggle_agent);
    push_global::<CloseTab>(&mut specs, &kb.close_tab);
    push_global::<ClosePane>(&mut specs, &kb.close_pane);
    push_global::<TogglePaneZoom>(&mut specs, &kb.toggle_pane_zoom);
    push_global::<settings_panel::ToggleSettings>(&mut specs, &kb.settings);
    push_global::<command_palette::ToggleCommandPalette>(&mut specs, &kb.command_palette);
    push_global::<SplitRight>(&mut specs, &kb.split_right);
    push_global::<SplitDown>(&mut specs, &kb.split_down);
    push_global::<NewSurface>(&mut specs, &kb.new_surface);
    push_global::<NewSurfaceSplitRight>(&mut specs, &kb.new_surface_split_right);
    push_global::<NewSurfaceSplitDown>(&mut specs, &kb.new_surface_split_down);
    push_global::<NextSurface>(&mut specs, &kb.next_surface);
    push_global::<PreviousSurface>(&mut specs, &kb.previous_surface);
    push_global::<RenameSurface>(&mut specs, &kb.rename_surface);
    push_global::<CloseSurface>(&mut specs, &kb.close_surface);
    push_global::<FocusInput>(&mut specs, &kb.focus_input);
    push_global::<AskAi>(&mut specs, &kb.ask_ai);
    push_global::<CycleInputMode>(&mut specs, &kb.cycle_input_mode);
    push_global::<ToggleInputBar>(&mut specs, &kb.toggle_input_bar);
    push_global::<TogglePaneScopePicker>(&mut specs, &kb.toggle_pane_scope);
    push_global::<ToggleLeftPanel>(&mut specs, &kb.toggle_left_panel);
    // Text inputs/editors may consume file/search navigation chords for
    // editing/search behavior before a global binding wins. Files/Search are
    // app navigation, so bind them explicitly in those focused contexts too.
    push_app_override::<FocusFiles>(&mut specs, &kb.focus_files);
    push_app_override::<SearchFiles>(&mut specs, &kb.search_files);
    push_global::<CollapseSidebar>(&mut specs, &kb.collapse_sidebar);

    specs
}

fn fixed_app_binding_specs() -> Vec<BindingSpec> {
    let mut specs = Vec::new();

    push_global::<NextTab>(&mut specs, "secondary-shift-]");
    push_global::<PreviousTab>(&mut specs, "secondary-shift-[");
    push_global::<SelectTab1>(&mut specs, "secondary-1");
    push_global::<SelectTab2>(&mut specs, "secondary-2");
    push_global::<SelectTab3>(&mut specs, "secondary-3");
    push_global::<SelectTab4>(&mut specs, "secondary-4");
    push_global::<SelectTab5>(&mut specs, "secondary-5");
    push_global::<SelectTab6>(&mut specs, "secondary-6");
    push_global::<SelectTab7>(&mut specs, "secondary-7");
    push_global::<SelectTab8>(&mut specs, "secondary-8");
    push_global::<SelectTab9>(&mut specs, "secondary-9");

    #[cfg(target_os = "macos")]
    {
        push_global::<HideApp>(&mut specs, "cmd-h");
        push_global::<HideOtherApps>(&mut specs, "cmd-alt-h");
        push_global::<ShowAllApps>(&mut specs, "cmd-alt-shift-h");
        push_global::<NextWindow>(&mut specs, "cmd-`");
        push_global::<NextWindow>(&mut specs, "cmd->");
        push_global::<PreviousWindow>(&mut specs, "cmd-shift-`");
        push_global::<PreviousWindow>(&mut specs, "cmd-~");
        push_global::<PreviousWindow>(&mut specs, "cmd-<");
        push_global::<Minimize>(&mut specs, "cmd-m");
    }

    specs
}

fn editor_binding_specs() -> Vec<BindingSpec> {
    let mut specs = Vec::new();

    // Editor-only editing shortcuts must not bind to Global/ConWorkspace/Input,
    // otherwise terminal input such as Enter can be intercepted.
    push_editor::<EditorMoveLeft>(&mut specs, "left");
    push_editor::<EditorMoveRight>(&mut specs, "right");
    push_editor::<EditorMoveUp>(&mut specs, "up");
    push_editor::<EditorMoveDown>(&mut specs, "down");
    push_editor::<EditorMoveHome>(&mut specs, "home");
    push_editor::<EditorMoveEnd>(&mut specs, "end");
    #[cfg(target_os = "macos")]
    push_editor::<EditorMoveLineStart>(&mut specs, "ctrl-a");
    #[cfg(target_os = "macos")]
    push_editor::<EditorMoveLineEnd>(&mut specs, "ctrl-e");
    push_editor::<EditorSelectLeft>(&mut specs, "shift-left");
    push_editor::<EditorSelectRight>(&mut specs, "shift-right");
    push_editor::<EditorSelectUp>(&mut specs, "shift-up");
    push_editor::<EditorSelectDown>(&mut specs, "shift-down");
    push_editor::<EditorSelectHome>(&mut specs, "shift-home");
    push_editor::<EditorSelectEnd>(&mut specs, "shift-end");
    push_editor::<EditorDeleteBackward>(&mut specs, "backspace");
    push_editor::<EditorDeleteForward>(&mut specs, "delete");
    push_editor::<EditorInsertNewline>(&mut specs, "enter");
    push_editor::<EditorSave>(&mut specs, "secondary-s");
    push_editor::<SelectAll>(&mut specs, "secondary-a");
    push_editor::<Copy>(&mut specs, "secondary-c");
    push_editor::<Cut>(&mut specs, "secondary-x");
    push_editor::<Paste>(&mut specs, "secondary-v");
    push_editor::<Undo>(&mut specs, "secondary-z");

    specs
}

fn binding_specs(kb: &KeybindingConfig) -> Vec<BindingSpec> {
    let mut specs = configurable_app_binding_specs(kb);
    specs.extend(fixed_app_binding_specs());
    specs.extend(editor_binding_specs());
    specs
}

fn bind_specs(cx: &mut App, specs: Vec<BindingSpec>) {
    cx.bind_keys(
        specs
            .into_iter()
            .map(|spec| spec.to_key_binding())
            .collect::<Vec<_>>(),
    );
}

#[derive(Clone)]
struct FileSidebarShortcutBindings {
    focus_files: Option<String>,
    search_files: Option<String>,
}

impl Global for FileSidebarShortcutBindings {}

impl FileSidebarShortcutBindings {
    fn from_keybindings(kb: &KeybindingConfig) -> Self {
        Self {
            focus_files: validated_file_sidebar_shortcut("Focus Files", &kb.focus_files),
            search_files: validated_file_sidebar_shortcut("Search Files", &kb.search_files),
        }
    }
}

fn validated_file_sidebar_shortcut(label: &str, binding: &str) -> Option<String> {
    let trimmed = binding.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut strokes = trimmed.split_whitespace();
    let Some(stroke) = strokes.next() else {
        return None;
    };
    if strokes.next().is_some() || Keystroke::parse(stroke).is_err() {
        log::warn!("{label}: expected a single parseable shortcut, ignoring {binding:?}");
        return None;
    }

    Some(stroke.to_string())
}

fn update_file_sidebar_shortcut_bindings(cx: &mut App, kb: &KeybindingConfig) {
    let bindings = FileSidebarShortcutBindings::from_keybindings(kb);
    if cx.has_global::<FileSidebarShortcutBindings>() {
        cx.update_global::<FileSidebarShortcutBindings, _>(move |state, _| {
            *state = bindings;
        });
    } else {
        cx.set_global(bindings);
    }
}

fn keystroke_matches_single_binding(keystroke: &Keystroke, binding: &str) -> bool {
    let mut strokes = binding.split_whitespace();
    let Some(stroke) = strokes.next() else {
        return false;
    };
    if strokes.next().is_some() {
        return false;
    }

    let Ok(target) = Keystroke::parse(stroke) else {
        return false;
    };
    let target = KeybindingKeystroke::from_keystroke(target);
    if keystroke.should_match(&target) {
        return true;
    }

    let typed_key = keystroke.key.to_ascii_lowercase();
    let target_key = target.key().to_ascii_lowercase();
    let mut typed_modifiers = keystroke.modifiers;
    if keystroke.key.chars().all(|ch| ch.is_ascii_uppercase()) {
        typed_modifiers.shift = true;
    }

    typed_key == target_key && typed_modifiers == *target.modifiers()
}

fn install_file_sidebar_shortcut_interceptor(cx: &mut App, kb: &KeybindingConfig) {
    update_file_sidebar_shortcut_bindings(cx, kb);
    cx.intercept_keystrokes(|event, window, cx| {
        if !event
            .context_stack
            .iter()
            .any(|context| context.contains("ConWorkspace"))
        {
            return;
        }

        let bindings = cx.global::<FileSidebarShortcutBindings>().clone();
        if bindings
            .focus_files
            .as_deref()
            .is_some_and(|binding| keystroke_matches_single_binding(&event.keystroke, binding))
        {
            window.dispatch_action(Box::new(FocusFiles), cx);
            cx.stop_propagation();
        } else if bindings
            .search_files
            .as_deref()
            .is_some_and(|binding| keystroke_matches_single_binding(&event.keystroke, binding))
        {
            window.dispatch_action(Box::new(SearchFiles), cx);
            cx.stop_propagation();
        }
    })
    .detach();
}

pub(crate) fn bind_app_keybindings(cx: &mut App, kb: &KeybindingConfig) {
    bind_specs(cx, binding_specs(kb));
}

pub(crate) fn fixed_app_keybinding_shortcuts() -> Vec<(&'static str, &'static str)> {
    let shortcuts = vec![
        ("Next Tab", "secondary-shift-]"),
        ("Previous Tab", "secondary-shift-["),
        ("Select Tab 1", "secondary-1"),
        ("Select Tab 2", "secondary-2"),
        ("Select Tab 3", "secondary-3"),
        ("Select Tab 4", "secondary-4"),
        ("Select Tab 5", "secondary-5"),
        ("Select Tab 6", "secondary-6"),
        ("Select Tab 7", "secondary-7"),
        ("Select Tab 8", "secondary-8"),
        ("Select Tab 9", "secondary-9"),
    ];

    #[cfg(target_os = "macos")]
    let shortcuts = {
        let mut shortcuts = shortcuts;
        shortcuts.extend([
            ("Hide Con", "cmd-h"),
            ("Hide Other Apps", "cmd-alt-h"),
            ("Show All Apps", "cmd-alt-shift-h"),
            ("Next Window", "cmd-`"),
            ("Next Window", "cmd->"),
            ("Previous Window", "cmd-shift-`"),
            ("Previous Window", "cmd-~"),
            ("Previous Window", "cmd-<"),
            ("Minimize Window", "cmd-m"),
        ]);
        shortcuts
    };

    shortcuts
}

fn is_fixed_configurable_app_keybinding(key: &str, action_name: &SharedString) -> bool {
    let Some(key) = con_core::config::canonical_keybinding(key) else {
        return false;
    };
    let fixed_next_tab = con_core::config::canonical_keybinding("secondary-shift-]")
        .expect("fixed shortcut is valid");
    let fixed_previous_tab = con_core::config::canonical_keybinding("secondary-shift-[")
        .expect("fixed shortcut is valid");
    let next_tab: SharedString = gpui::Action::name(&NextTab).into();
    let previous_tab: SharedString = gpui::Action::name(&PreviousTab).into();

    (key == fixed_next_tab && action_name == &next_tab)
        || (key == fixed_previous_tab && action_name == &previous_tab)
}

fn bind_configurable_app_keybindings(cx: &mut App, kb: &KeybindingConfig) {
    bind_specs(cx, configurable_app_binding_specs(kb));
}

pub(crate) fn rebind_app_keybindings(cx: &mut App, old: &KeybindingConfig, new: &KeybindingConfig) {
    let unbinds = configurable_app_binding_specs(old)
        .into_iter()
        .filter_map(|spec| {
            if is_fixed_configurable_app_keybinding(spec.key.as_ref(), &spec.action_name()) {
                None
            } else {
                Some(spec.to_unbind_key_binding())
            }
        })
        .collect::<Vec<_>>();
    cx.bind_keys(unbinds);
    bind_configurable_app_keybindings(cx, new);
    update_file_sidebar_shortcut_bindings(cx, new);
}

/// Install a panic hook that writes every panic (including from
/// background threads) to both stderr and a log file in the platform
/// temp dir. On Windows specifically, a GUI-launched exe has no
/// console by default and the default panic output is lost — this
/// guarantees we always have a record to read after a silent exit.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".into());
        let payload = payload_as_str(info.payload()).unwrap_or("<non-string payload>");

        let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let record = format!(
            "[{timestamp}] PANIC on thread '{thread_name}' at {location}:\n  \
             {payload}\n\nBacktrace:\n{backtrace}\n\n"
        );

        // Always try stderr first.
        let _ = {
            use std::io::Write;
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(record.as_bytes());
            stderr.flush()
        };

        // Then persist to a file — this is the one the user can actually
        // read after the process exits.
        let log_path = std::env::temp_dir().join("con-panic.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            use std::io::Write;
            let _ = f.write_all(record.as_bytes());
            let _ = f.flush();
            eprintln!("[panic log] {}", log_path.display());
        }

        // Chain to the previous hook so cargo / test harnesses still
        // see what they expect.
        previous(info);
    }));
}

fn payload_as_str(payload: &(dyn std::any::Any + Send)) -> Option<&str> {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        Some(*s)
    } else if let Some(s) = payload.downcast_ref::<String>() {
        Some(s.as_str())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BindingSpec, EditorDeleteBackward, EditorInsertNewline, EditorMoveLineEnd,
        FileSidebarShortcutBindings, FocusFiles, FocusInput, NewTab, SearchFiles, SelectTab1,
        ToggleAgentPanel, Undo, binding_specs, command_palette, keystroke_matches_single_binding,
        push_app_override,
    };
    use con_core::config::KeybindingConfig;
    use gpui::Keystroke;
    use std::any::TypeId;

    fn default_specs() -> Vec<BindingSpec> {
        binding_specs(&KeybindingConfig::default())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_window_shape_uses_compact_rounded_rectangles() {
        let rectangles = super::linux_window_shape_rectangles(
            120,
            80,
            super::LinuxWindowShapeRadii {
                top_left: 14,
                top_right: 14,
                bottom_right: 14,
                bottom_left: 14,
            },
        )
        .unwrap();

        assert!(rectangles.len() > 1);
        assert!(rectangles.first().unwrap().x > 0);
        assert!(rectangles.first().unwrap().width < 120);
        assert_eq!(rectangles.first().unwrap().x, rectangles.last().unwrap().x);
        assert_eq!(
            rectangles.first().unwrap().width,
            rectangles.last().unwrap().width
        );
        assert!(
            rectangles
                .iter()
                .any(|rect| rect.x == 0 && rect.width == 120)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_window_shape_can_reset_to_full_rectangle() {
        let rectangles =
            super::linux_window_shape_rectangles(120, 80, super::LinuxWindowShapeRadii::default())
                .unwrap();

        assert_eq!(rectangles.len(), 1);
        assert_eq!(rectangles[0].x, 0);
        assert_eq!(rectangles[0].y, 0);
        assert_eq!(rectangles[0].width, 120);
        assert_eq!(rectangles[0].height, 80);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_window_shape_supports_independent_corners() {
        let rectangles = super::linux_window_shape_rectangles(
            120,
            80,
            super::LinuxWindowShapeRadii {
                top_left: 14,
                top_right: 0,
                bottom_right: 14,
                bottom_left: 0,
            },
        )
        .unwrap();

        assert!(rectangles.first().unwrap().x > 0);
        assert_eq!(
            rectangles.first().unwrap().width,
            120 - rectangles.first().unwrap().x as u16
        );
        assert!(rectangles.last().unwrap().x == 0);
        assert!(rectangles.last().unwrap().width < 120);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_window_shape_large_dimensions_fall_back_to_full_rectangle() {
        let rectangles = super::linux_window_shape_rectangles(
            120,
            i16::MAX as u32 + 1,
            super::LinuxWindowShapeRadii {
                top_left: 14,
                top_right: 14,
                bottom_right: 14,
                bottom_left: 14,
            },
        )
        .unwrap();

        assert_eq!(rectangles.len(), 1);
        assert_eq!(rectangles[0].x, 0);
        assert_eq!(rectangles[0].y, 0);
        assert_eq!(rectangles[0].width, 120);
        assert_eq!(rectangles[0].height, i16::MAX as u16 + 1);
    }

    fn specs_for_action<A: 'static>(specs: &[BindingSpec]) -> Vec<&BindingSpec> {
        let action = TypeId::of::<A>();
        specs.iter().filter(|spec| spec.action == action).collect()
    }

    fn scope_names(specs: &[&BindingSpec]) -> Vec<Option<&'static str>> {
        let mut scopes = specs
            .iter()
            .map(|spec| spec.scope.name())
            .collect::<Vec<_>>();
        scopes.sort();
        scopes.dedup();
        scopes
    }

    #[test]
    fn app_shortcuts_are_global_by_default() {
        let specs = default_specs();

        for (name, scopes) in [
            ("NewTab", scope_names(&specs_for_action::<NewTab>(&specs))),
            (
                "ToggleCommandPalette",
                scope_names(&specs_for_action::<command_palette::ToggleCommandPalette>(
                    &specs,
                )),
            ),
            (
                "SelectTab1",
                scope_names(&specs_for_action::<SelectTab1>(&specs)),
            ),
            (
                "FocusInput",
                scope_names(&specs_for_action::<FocusInput>(&specs)),
            ),
        ] {
            assert_eq!(
                scopes,
                vec![None],
                "{name} should be a single global binding"
            );
        }
    }

    #[test]
    fn file_sidebar_shortcuts_override_text_inputs_and_editor_views() {
        let specs = default_specs();

        for (name, scopes) in [
            (
                "FocusFiles",
                scope_names(&specs_for_action::<FocusFiles>(&specs)),
            ),
            (
                "SearchFiles",
                scope_names(&specs_for_action::<SearchFiles>(&specs)),
            ),
        ] {
            assert_eq!(
                scopes,
                vec![None, Some("EditorView"), Some("Input")],
                "{name} must work from terminals, inputs, and editor panes"
            );
        }
    }

    #[test]
    fn file_sidebar_shortcut_matcher_accepts_configured_chords() {
        let focus_binding = if cfg!(target_os = "macos") {
            "secondary-alt-e"
        } else {
            "secondary-shift-e"
        };
        let focus = Keystroke::parse(focus_binding).unwrap();
        let search = Keystroke::parse("secondary-shift-f").unwrap();

        assert!(keystroke_matches_single_binding(&focus, focus_binding));
        assert!(keystroke_matches_single_binding(
            &search,
            "secondary-shift-f"
        ));
        assert!(!keystroke_matches_single_binding(
            &focus,
            "secondary-shift-f"
        ));
    }

    #[test]
    fn file_sidebar_shortcut_matcher_ignores_multi_stroke_bindings() {
        let focus_binding = if cfg!(target_os = "macos") {
            "secondary-alt-e"
        } else {
            "secondary-shift-e"
        };
        let focus = Keystroke::parse(focus_binding).unwrap();
        let chord = format!("secondary-k {focus_binding}");

        assert!(!keystroke_matches_single_binding(&focus, &chord));
    }

    #[test]
    fn file_sidebar_shortcut_bindings_ignore_invalid_config_values() {
        let mut kb = KeybindingConfig::default();
        kb.focus_files = "secondary-k secondary-e".to_string();
        kb.search_files = "not-a-valid binding".to_string();

        let bindings = FileSidebarShortcutBindings::from_keybindings(&kb);

        assert!(bindings.focus_files.is_none());
        assert!(bindings.search_files.is_none());
    }

    #[test]
    fn editor_shortcuts_stay_editor_only() {
        let specs = default_specs();
        for (name, scopes) in [
            (
                "EditorInsertNewline",
                scope_names(&specs_for_action::<EditorInsertNewline>(&specs)),
            ),
            (
                "EditorMoveLineEnd",
                scope_names(&specs_for_action::<EditorMoveLineEnd>(&specs)),
            ),
            (
                "EditorDeleteBackward",
                scope_names(&specs_for_action::<EditorDeleteBackward>(&specs)),
            ),
            ("Undo", scope_names(&specs_for_action::<Undo>(&specs))),
        ] {
            assert_eq!(
                scopes,
                vec![Some("EditorView")],
                "{name} leaked outside EditorView"
            );
        }
    }

    #[test]
    fn terminal_reserved_enter_is_not_bound_outside_editor_view() {
        let specs = default_specs();
        let enter_specs = specs
            .iter()
            .filter(|spec| spec.key == "enter")
            .collect::<Vec<_>>();

        assert_eq!(enter_specs.len(), 1);
        assert_eq!(enter_specs[0].scope.name(), Some("EditorView"));
        assert_eq!(enter_specs[0].action, TypeId::of::<EditorInsertNewline>());
    }

    #[test]
    fn explicit_multi_context_override_can_be_represented() {
        let mut specs = Vec::new();
        push_app_override::<ToggleAgentPanel>(&mut specs, "secondary-test");

        let refs = specs.iter().collect::<Vec<_>>();
        assert_eq!(
            scope_names(&refs),
            vec![None, Some("EditorView"), Some("Input")]
        );
    }

    #[test]
    fn generated_specs_do_not_duplicate_same_key_action_scope() {
        let specs = default_specs();
        let mut seen = std::collections::HashMap::new();

        for spec in &specs {
            let canonical = con_core::config::canonical_keybinding(spec.key.as_ref())
                .unwrap_or_else(|| spec.key.to_string());
            let key = (canonical, spec.scope.name());
            if let Some(existing_action) = seen.insert(key.clone(), spec.action) {
                assert_eq!(
                    existing_action, spec.action,
                    "duplicate keybinding spec for canonical_key={:?} scope={:?} actions={:?}/{:?}",
                    key.0, key.1, existing_action, spec.action
                );
            }
        }
    }

    #[test]
    fn default_configurable_shortcuts_do_not_conflict_with_fixed_app_shortcuts() {
        let kb = KeybindingConfig::default();
        let conflicts = kb.shortcut_conflicts(&super::fixed_app_keybinding_shortcuts());

        assert!(
            conflicts.is_empty(),
            "default shortcuts should not conflict with fixed app shortcuts: {conflicts:?}"
        );
    }

    #[test]
    fn terminal_text_keys_are_not_bound_in_global_input_or_workspace_scopes() {
        let specs = default_specs();

        for reserved_key in [
            "enter",
            "backspace",
            "delete",
            "left",
            "right",
            "up",
            "down",
        ] {
            let offenders = specs
                .iter()
                .filter(|spec| spec.key == reserved_key)
                .filter(|spec| spec.scope.name() != Some("EditorView"))
                .collect::<Vec<_>>();

            assert!(
                offenders.is_empty(),
                "terminal key {reserved_key:?} leaked outside editor scope: {offenders:?}"
            );
        }
    }

    #[test]
    fn explicit_app_override_is_not_used_by_default_specs() {
        let specs = default_specs();

        let non_global_app_specs = specs
            .iter()
            .filter(|spec| {
                let action = spec.action;
                action == TypeId::of::<NewTab>()
                    || action == TypeId::of::<SelectTab1>()
                    || action == TypeId::of::<command_palette::ToggleCommandPalette>()
                    || action == TypeId::of::<FocusInput>()
            })
            .filter(|spec| spec.scope.name().is_some())
            .collect::<Vec<_>>();

        assert!(
            non_global_app_specs.is_empty(),
            "default app shortcuts should be global-only unless explicitly proven otherwise: {non_global_app_specs:?}"
        );
    }
}

/// Install a Win32 vectored exception handler. Catches C-level faults
/// (EXCEPTION_ACCESS_VIOLATION, EXCEPTION_STACK_OVERFLOW, illegal
/// instruction, etc.) that bypass Rust's panic machinery — common when
/// an FFI library (libghostty-vt, a GPU driver) crashes inside its
/// own code. We log the exception code + address and the last breadcrumb
/// visible in stderr, then let the exception propagate so the process
/// still terminates.
///
/// Vectored handlers run *before* structured exception handling (SEH),
/// so ours fires even if the FFI library has its own `__try`/`__except`.
#[cfg(target_os = "windows")]
fn install_seh_filter() {
    use windows::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_POINTERS,
    };

    extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: `info` is provided by the OS during an exception
        // dispatch; it points at a valid EXCEPTION_POINTERS structure.
        // We only read a few scalar fields and write to stderr /
        // a file; no heap allocation is done from this handler.
        let (code, address) = unsafe {
            if info.is_null() {
                (0u32, std::ptr::null_mut())
            } else {
                let er = (*info).ExceptionRecord;
                if er.is_null() {
                    (0, std::ptr::null_mut())
                } else {
                    ((*er).ExceptionCode.0 as u32, (*er).ExceptionAddress)
                }
            }
        };

        // Only log "interesting" exceptions — many Windows internals
        // throw soft exceptions (DLL not found probes, debugger
        // breaks) that we shouldn't report. The ones below are the
        // real crashes.
        let interesting = matches!(
            code,
            0xC0000005  // ACCESS_VIOLATION
            | 0xC00000FD  // STACK_OVERFLOW
            | 0xC000001D  // ILLEGAL_INSTRUCTION
            | 0xC0000094  // INT_DIVIDE_BY_ZERO
            | 0xC0000096  // PRIV_INSTRUCTION
            | 0xC000013A // CONTROL_C_EXIT
        );

        if interesting {
            let record = format!(
                "[SEH] Exception {:#x} at {:p} — likely C/FFI crash. \
                 The most recent log line above is the last thing that \
                 ran before the fault.\n",
                code, address
            );
            use std::io::Write;
            let _ = std::io::stderr().lock().write_all(record.as_bytes());
            let log_path = std::env::temp_dir().join("con-panic.log");
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                let _ = f.write_all(record.as_bytes());
            }
        }

        // EXCEPTION_CONTINUE_SEARCH = 0: let normal SEH unwinding run.
        0
    }

    // SAFETY: registering a handler is always safe; the handler fn is
    // `extern "system"` with the correct signature.
    unsafe {
        let _ = AddVectoredExceptionHandler(1, Some(handler));
    }
}

/// On macOS and Linux, GUI apps launched from the Dock / Spotlight / a
/// desktop launcher do not inherit the user's shell environment. Variables
/// like `PATH`, `GOPATH`, `NVM_DIR`, `HTTP_PROXY`, etc. that are set in
/// shell rc files are absent from the process environment.
///
/// Runs `<login-shell> -l -c 'env'` and full-overrides the process env with
/// the shell's values, matching Zed's `load_login_shell_environment` strategy.
/// `SHLVL` is skipped so terminal children start at level 1.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn inherit_shell_env() {
    use std::io::Read;
    use std::process::Command;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    // Run `<shell> -l -c 'env'`.
    // -l : login shell — sources /etc/profile, ~/.zprofile, ~/.bash_profile …
    //      (no -i: avoids interactive-only rc noise and TTY hangs)
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
    let mut child = match Command::new(&shell)
        .args(["-l", "-c", "env"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            log::debug!("inherit_shell_env: could not run {shell}: {e}");
            return;
        }
    };

    // Read stdout incrementally on a background thread, sending chunks as they
    // arrive. The main thread signals "child exited" via a second channel so
    // the reader can stop without waiting for EOF (which may never come if a
    // background process inherited stdout).
    let mut stdout_pipe = match child.stdout.take() {
        Some(p) => p,
        None => {
            log::debug!("inherit_shell_env: no stdout pipe");
            return;
        }
    };

    // Set the pipe to non-blocking so the reader thread can poll and check
    // the stop signal without blocking forever on a single read call.
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        unsafe extern "C" {
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        #[cfg(target_os = "macos")]
        const O_NONBLOCK: i32 = 0x0004;
        #[cfg(target_os = "linux")]
        const O_NONBLOCK: i32 = 0x0800;
        const F_GETFL: i32 = 3;
        const F_SETFL: i32 = 4;
        let fd = stdout_pipe.as_raw_fd();
        unsafe {
            let flags = fcntl(fd, F_GETFL);
            if flags >= 0 {
                fcntl(fd, F_SETFL, flags | O_NONBLOCK);
            }
        }
    }

    let (data_tx, data_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let reader_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            // Check if the main thread signalled stop.
            if stop_rx.try_recv().is_ok() {
                break;
            }
            match stdout_pipe.read(&mut tmp) {
                Ok(0) => break, // EOF
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = data_tx.send(buf);
    });

    // Wait for the child with a deadline.
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    log::debug!(
                        "inherit_shell_env: shell timed out after {}s, killing",
                        TIMEOUT.as_secs()
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stop_tx.send(());
                    let _ = reader_handle.join();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => {
                log::debug!("inherit_shell_env: try_wait error: {e}");
                let _ = stop_tx.send(());
                let _ = reader_handle.join();
                return;
            }
        }
    }

    // Child exited — signal the reader to stop (it may still be draining
    // buffered bytes) and collect whatever was read so far.
    let _ = stop_tx.send(());
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    let raw = data_rx.recv_timeout(remaining).unwrap_or_default();
    let _ = reader_handle.join();
    let stdout = match std::str::from_utf8(&raw) {
        Ok(s) => s,
        Err(_) => return,
    };

    // Parse KEY=VALUE lines. Values may contain `=` so split on the first `=` only.
    // Full override — shell values win, matching Zed's load_login_shell_environment.
    // Skip SHLVL: the login shell increments it, and terminals spawned by con
    // would inherit it and increment again (starting at 2 instead of 1).
    let mut inherited = 0usize;
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "SHLVL" {
            continue;
        }
        // SAFETY: single-threaded at this point in startup; no other
        // threads have been spawned yet.
        unsafe { std::env::set_var(key, value) };
        inherited += 1;
    }

    if inherited > 0 {
        log::info!("inherit_shell_env: inherited {inherited} variables from login shell");
    } else {
        log::debug!("inherit_shell_env: no new variables found in login shell env");
    }
}

fn main() {
    // Install a panic hook before anything else so every panic —
    // including ones from background threads that would otherwise be
    // invisible — gets written to both stderr and a dated log file.
    // Critical on Windows where double-clicking the exe detaches
    // stderr and a panic produces a silent exit.
    install_panic_hook();
    terminal_env::sanitize_inherited_terminal_environment();

    // Catch C-level access violations / stack overflows / etc. from
    // FFI libraries (libghostty-vt, GPU driver shims) that bypass
    // Rust's panic infrastructure entirely.
    #[cfg(target_os = "windows")]
    install_seh_filter();

    // Always capture backtraces unless the user already set a
    // preference. `full` prints symbols + line numbers when debug info
    // is present; harmless in release.
    if std::env::var_os("RUST_BACKTRACE").is_none() {
        // SAFETY: single-threaded during startup, standard env-var set.
        unsafe { std::env::set_var("RUST_BACKTRACE", "full") };
    }

    // Init the logger before inherit_shell_env so its log::info!/debug!
    // calls are visible when debugging proxy issues.
    let mut builder = env_logger::Builder::from_default_env();
    if std::env::var_os("RUST_LOG").is_none() {
        builder.filter_level(log::LevelFilter::Info);
    }
    let mut log_file_path = None;
    if let Some(path) = std::env::var_os("CON_LOG_FILE") {
        match std::fs::File::create(&path) {
            Ok(file) => {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
                builder.write_style(env_logger::WriteStyle::Never);
                log_file_path = Some(std::path::PathBuf::from(&path));
            }
            Err(err) => {
                eprintln!(
                    "con: failed to open CON_LOG_FILE={}: {err}",
                    std::path::Path::new(&path).display()
                );
            }
        }
    }
    builder.init();

    // Inherit the full login shell environment before any network client is
    // constructed. GUI apps on macOS/Linux launched from Dock/Spotlight don't
    // get PATH, GOPATH, HTTP_PROXY, etc. from shell rc files.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    inherit_shell_env();
    terminal_env::sanitize_inherited_terminal_environment();

    log::info!("con starting (pid {})", std::process::id());
    if let Some(path) = log_file_path.as_ref() {
        log::info!("logging to {}", path.display());
    }
    #[cfg(target_os = "macos")]
    cli_shim::ensure_cli_shim();
    apply_linux_platform_override();

    let config = con_core::Config::load().unwrap_or_default();
    log::info!("config loaded");
    // Apply [network] proxy config — overrides any shell-inherited env vars.
    // SAFETY: single-threaded at this point in startup; no other threads spawned yet.
    unsafe { config.network.apply_to_env() };

    let app = gpui_platform::application()
        .with_quit_mode(QuitMode::Explicit)
        .with_assets(assets::ConAssets);
    log::info!("gpui application created");
    app.on_reopen(|cx| {
        let has_windows = cx
            .window_stack()
            .map(|windows| !windows.is_empty())
            .unwrap_or(false);
        if has_windows {
            cx.activate(true);
            return;
        }

        let config = con_core::Config::load().unwrap_or_default();
        open_con_window(config, fresh_window_session_with_history(), false, cx);
        cx.activate(true);
    });
    app.run(move |cx: &mut App| {
        // Detect release channel (stable/beta/dev) from bundle Info.plist
        let channel = con_core::release_channel::init();
        log::info!("Release channel: {}", channel.display_name());

        // Set dock icon for development (cargo run)
        set_dock_icon();

        // Initialize gpui-component subsystems (theme, input, dialog, etc.)
        gpui_component::init(cx);
        input_bar::InputBar::init(cx);

        // Load and activate con's design theme (synced to terminal theme)
        theme::init_theme(
            cx,
            &config.terminal.theme,
            &config.terminal.font_family,
            &config.appearance.ui_font_family,
            config.appearance.ui_font_size,
        );

        // Register ghostty terminal key bindings (Tab interception, etc.)
        // The stub impl is a no-op but we call it unconditionally so the
        // code path is shape-identical across platforms.
        ghostty_view::init(cx);

        // Register global keybindings from user config
        bind_app_keybindings(cx, &config.keybindings);
        install_file_sidebar_shortcut_interceptor(cx, &config.keybindings);
        #[cfg(target_os = "macos")]
        global_hotkey::init(cx, &config.keybindings);
        #[cfg(target_os = "macos")]
        quick_terminal::init(cx);
        #[cfg(target_os = "macos")]
        macos_windowing::install_window_cycle_shortcuts();

        cx.on_action(|_: &NewWindow, cx: &mut App| {
            let config = con_core::Config::load().unwrap_or_default();
            open_con_window(config, fresh_window_session_with_history(), false, cx);
        });
        cx.on_action(|_: &ToggleSummon, cx: &mut App| {
            toggle_global_summon(cx);
        });
        #[cfg(target_os = "macos")]
        cx.on_action(|_: &ToggleQuickTerminal, cx: &mut App| {
            quick_terminal::toggle(cx);
        });
        #[cfg(target_os = "macos")]
        cx.on_action(|_: &NextWindow, _cx: &mut App| {
            macos_windowing::cycle_app_window(false);
        });
        #[cfg(target_os = "macos")]
        cx.on_action(|_: &PreviousWindow, _cx: &mut App| {
            macos_windowing::cycle_app_window(true);
        });
        cx.on_action(|_: &Minimize, cx: &mut App| {
            minimize_frontmost_window(cx);
        });
        cx.on_action(|_: &NewTab, cx: &mut App| {
            if cx.active_window().is_none() {
                let config = con_core::Config::load().unwrap_or_default();
                open_con_window(config, fresh_window_session_with_history(), false, cx);
            }
        });

        #[cfg(target_os = "macos")]
        cx.on_action(|_: &ShowAbout, cx: &mut App| {
            show_about_window(cx);
        });

        cx.on_action(|_: &CheckForUpdates, _cx: &mut App| {
            updater::check_for_updates();
        });
        cx.on_action(|_: &HideApp, cx: &mut App| {
            cx.hide();
        });
        cx.on_action(|_: &HideOtherApps, cx: &mut App| {
            cx.hide_other_apps();
        });
        cx.on_action(|_: &ShowAllApps, cx: &mut App| {
            cx.unhide_other_apps();
        });

        cx.set_menus(vec![
            Menu {
                name: "con".into(),
                items: vec![
                    MenuItem::action("About con", ShowAbout),
                    MenuItem::separator(),
                    MenuItem::action("Check for Updates…", CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Settings…", settings_panel::ToggleSettings),
                    MenuItem::separator(),
                    MenuItem::action("Hide con", HideApp),
                    MenuItem::action("Hide Others", HideOtherApps),
                    MenuItem::action("Show All", ShowAllApps),
                    MenuItem::separator(),
                    MenuItem::action("Quit con", Quit),
                ],
                disabled: false,
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Window", NewWindow),
                    MenuItem::action("New Tab", NewTab),
                    MenuItem::separator(),
                    MenuItem::action("Close Tab", CloseTab),
                    MenuItem::action("Close Pane", ClosePane),
                    MenuItem::action("Toggle Pane Zoom", TogglePaneZoom),
                    MenuItem::separator(),
                    MenuItem::action("Split Right", SplitRight),
                    MenuItem::action("Split Down", SplitDown),
                    MenuItem::action("Split Left", SplitLeft),
                    MenuItem::action("Split Up", SplitUp),
                    MenuItem::separator(),
                    MenuItem::action("Clear Terminal", ClearTerminal),
                    MenuItem::action(
                        "Clear Restored Terminal History",
                        ClearRestoredTerminalHistory,
                    ),
                    MenuItem::separator(),
                    MenuItem::action("New Surface Tab", NewSurface),
                    MenuItem::action("New Surface Pane Right", NewSurfaceSplitRight),
                    MenuItem::action("New Surface Pane Down", NewSurfaceSplitDown),
                    MenuItem::action("Next Surface Tab", NextSurface),
                    MenuItem::action("Previous Surface Tab", PreviousSurface),
                    MenuItem::action("Rename Current Surface", RenameSurface),
                    MenuItem::action("Close Current Surface", CloseSurface),
                ],
                disabled: false,
            },
            Menu {
                name: "Workspace".into(),
                items: vec![
                    MenuItem::action("Save Layout Profile…", ExportWorkspaceLayout),
                    MenuItem::separator(),
                    MenuItem::action("Add Tabs from Layout Profile…", AddWorkspaceLayoutTabs),
                    MenuItem::action(
                        "Open Layout Profile in New Window…",
                        OpenWorkspaceLayoutWindow,
                    ),
                ],
                disabled: false,
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Undo", Undo, OsAction::Undo),
                    MenuItem::os_action("Redo", Redo, OsAction::Redo),
                    MenuItem::separator(),
                    MenuItem::os_action("Cut", Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", Paste, OsAction::Paste),
                    MenuItem::separator(),
                    MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
                ],
                disabled: false,
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Toggle Agent Panel", ToggleAgentPanel),
                    MenuItem::action("Toggle Input Bar", ToggleInputBar),
                    #[cfg(target_os = "macos")]
                    MenuItem::action("Quick Terminal", ToggleQuickTerminal),
                    MenuItem::action("Command Palette", command_palette::ToggleCommandPalette),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Input / Terminal", FocusInput),
                    MenuItem::action("Cycle Input Mode", CycleInputMode),
                    MenuItem::separator(),
                    MenuItem::action("Focus Files", FocusFiles),
                    MenuItem::action("Search Files", SearchFiles),
                ],
                disabled: false,
            },
            #[cfg(target_os = "macos")]
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Minimize", Minimize),
                    MenuItem::separator(),
                    MenuItem::action("Next Window", NextWindow),
                    MenuItem::action("Previous Window", PreviousWindow),
                ],
                disabled: false,
            },
        ]);

        cx.set_dock_menu(vec![MenuItem::action("New Window", NewWindow)]);

        open_con_window(config.clone(), startup_session(), true, cx);

        // Initialize the auto-updater. On macOS this loads Sparkle
        // from the app bundle; on Windows it kicks off a notify-only
        // background check against the release appcast.
        updater::init();
    });
}
