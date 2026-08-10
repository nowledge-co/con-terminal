//! GhosttyView — GPUI element wrapping ghostty's GPU-accelerated Metal terminal.
//!
//! Architecture:
//! - Creates an NSView child within GPUI's window view
//! - Passes the NSView to ghostty (macOS embedded platform)
//! - Ghostty renders via Metal into that view (zero-copy, GPU-accelerated)
//! - GPUI input events forwarded via `ghostty_surface_key` / `ghostty_surface_text`
//! - Terminal state (title, pwd) arrives via ghostty's per-surface action callbacks
//!
//! Key input goes through ghostty's key processing pipeline (not raw escape
//! sequences), so application cursor mode, kitty keyboard protocol, and
//! ghostty key bindings all work correctly.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
#[cfg(target_os = "macos")]
use std::os::raw::c_void;
use std::sync::Arc;
use std::sync::OnceLock;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use con_ghostty::ffi;
use con_ghostty::{
    GhosttyApp, GhosttySplitDirection, GhosttySurfaceEvent, GhosttyTerminal, MouseButton,
};
use gpui::{prelude::FluentBuilder as _, *};
use gpui_component::ActiveTheme;
use gpui_component::menu::ContextMenuExt;

use crate::terminal_paste::{
    TerminalPastePayload, payload_from_clipboard, payload_from_external_paths,
};
use crate::terminal_restore::key_down_may_write_terminal;

// Actions to intercept Tab/Shift-Tab before Root's focus-cycling handler.
actions!(ghostty, [ConsumeTab, ConsumeTabPrev]);

#[cfg(target_os = "macos")]
use cocoa::appkit::NSWindowOrderingMode;
#[cfg(target_os = "macos")]
use cocoa::base::{NO, YES, id, nil};
#[cfg(target_os = "macos")]
use cocoa::foundation::NSRect;
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use raw_window_handle::HasWindowHandle;

#[cfg(target_os = "macos")]
const NS_VIEW_WIDTH_SIZABLE: usize = 1 << 1;
#[cfg(target_os = "macos")]
const NS_VIEW_HEIGHT_SIZABLE: usize = 1 << 4;
#[cfg(target_os = "macos")]
const NS_VIEW_LAYER_CONTENTS_REDRAW_DURING_VIEW_RESIZE: isize = 2;
#[cfg(target_os = "macos")]
const NATIVE_SEAM_OVERDRAW_PT: f64 = 2.0;
#[cfg(target_os = "macos")]
static NATIVE_TRANSITION_UNDERLAY_ASSOCIATION_KEY: u8 = 0;
#[cfg(target_os = "macos")]
static NATIVE_TRANSITION_UNDERLAY_OWNERS_ASSOCIATION_KEY: u8 = 0;
#[cfg(target_os = "macos")]
static NEXT_NATIVE_TRANSITION_OWNER_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct NativeBackingSync {
    width_px: u32,
    height_px: u32,
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn objc_getAssociatedObject(object: id, key: *const c_void) -> id;
    fn objc_setAssociatedObject(object: id, key: *const c_void, value: id, policy: usize);
    fn con_ghostty_surface_install_backing_observer(view: *mut c_void, surface: *mut c_void);
    fn con_ghostty_surface_remove_backing_observer(view: *mut c_void);
    fn con_ghostty_surface_sync_backing(
        view: *mut c_void,
        surface: *mut c_void,
        logical_width: f64,
        logical_height: f64,
        fallback_scale: f64,
        out_scale_x: *mut f64,
        out_scale_y: *mut f64,
        out_width: *mut u32,
        out_height: *mut u32,
    ) -> bool;
}

#[cfg(target_os = "macos")]
const OBJC_ASSOCIATION_RETAIN_NONATOMIC: usize = 1;

fn perf_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CON_GHOSTTY_PROFILE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// Emitted when the terminal title changes.
#[allow(dead_code)]
pub struct GhosttyTitleChanged(pub Option<String>);

/// Emitted when the terminal process exits.
pub struct GhosttyProcessExited;

/// Emitted when the terminal gains focus.
pub struct GhosttyFocusChanged;
/// Emitted when Ghostty requests a new split from this surface.
pub struct GhosttySplitRequested(pub GhosttySplitDirection);
pub struct GhosttyCwdChanged(pub Option<String>);

impl EventEmitter<GhosttyTitleChanged> for GhosttyView {}
impl EventEmitter<GhosttyProcessExited> for GhosttyView {}
impl EventEmitter<GhosttyFocusChanged> for GhosttyView {}
impl EventEmitter<GhosttySplitRequested> for GhosttyView {}
impl EventEmitter<GhosttyCwdChanged> for GhosttyView {}

/// GPUI view wrapping a ghostty terminal surface.
pub struct GhosttyView {
    app: Arc<GhosttyApp>,
    terminal: Option<Arc<GhosttyTerminal>>,
    focus_handle: FocusHandle,
    initial_cwd: Option<String>,
    initial_font_size: f32,
    #[cfg(target_os = "macos")]
    host_view: Option<id>,
    #[cfg(target_os = "macos")]
    document_view: Option<id>,
    #[cfg(target_os = "macos")]
    nsview: Option<id>,
    /// The GPUI Metal view (GPUIView) — cached so detach_host_view can restore
    /// first responder to it without walking the view hierarchy.
    #[cfg(target_os = "macos")]
    gpui_nsview: Option<id>,
    #[cfg(target_os = "macos")]
    native_underlay_view: Option<id>,
    #[cfg(target_os = "macos")]
    native_backing_color: Option<(u8, u8, u8, u8)>,
    #[cfg(target_os = "macos")]
    native_underlay_color: Option<(u8, u8, u8, u8)>,
    #[cfg(target_os = "macos")]
    pending_native_layout: bool,
    initialized: bool,
    last_bounds: Option<Bounds<Pixels>>,
    scale_factor: f32,
    last_title: Option<String>,
    last_cwd: Option<String>,
    /// Data queued for the PTY before the surface was created.
    /// Flushed once in `ensure_initialized()` after the terminal exists.
    pending_write: Option<Vec<u8>>,
    /// Private screen text restored from the previous app session. This is a
    /// visual continuity layer only; it is cleared on the first user input so
    /// we never replay commands or write synthetic bytes into the shell.
    restored_screen_text: Option<Vec<String>>,
    /// Desired native view visibility, including before the NSView exists.
    native_view_visible: Cell<bool>,
    /// Desired Ghostty focus state. This is independent from GPUI keyboard
    /// focus so broadcast/custom pane scopes can light multiple cursors.
    surface_focused: Cell<bool>,
    /// When a surface was created before a real layout pass, keep it hidden until
    /// update_frame commits an actual pane geometry.
    awaiting_first_layout_visibility: bool,
    /// Guard: emit GhosttyProcessExited exactly once, not every 8ms tick.
    process_exit_emitted: bool,
    /// Retry surface creation after transient libghostty initialization failures.
    next_surface_init_retry_at: Option<Instant>,
    /// Last mouse position seen by GPUI. Ghostty link hover state depends on
    /// both position and modifiers, but macOS sends modifier changes without a
    /// mouse-move event when the pointer is stationary.
    #[cfg(target_os = "macos")]
    last_mouse_position: Option<Point<Pixels>>,
    #[cfg(target_os = "macos")]
    native_transition_underlay_visible: Cell<bool>,
    #[cfg(target_os = "macos")]
    native_transition_underlay_owner_id: u64,
    ime_marked_text: Option<String>,
}

/// Register ghostty key bindings. Call once at startup.
pub fn init(cx: &mut App) {
    // Bind Tab/Shift-Tab to our consume actions within the GhosttyTerminal context.
    // This prevents Root's Tab handler from intercepting Tab when the terminal is focused.
    cx.bind_keys([
        KeyBinding::new("tab", ConsumeTab, Some("GhosttyTerminal")),
        KeyBinding::new("shift-tab", ConsumeTabPrev, Some("GhosttyTerminal")),
    ]);
}

impl GhosttyView {
    pub fn new(
        app: Arc<GhosttyApp>,
        cwd: Option<String>,
        restored_screen_text: Option<Vec<String>>,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            terminal: None,
            focus_handle: cx.focus_handle(),
            initial_cwd: cwd.clone(),
            initial_font_size: font_size,
            #[cfg(target_os = "macos")]
            host_view: None,
            #[cfg(target_os = "macos")]
            document_view: None,
            #[cfg(target_os = "macos")]
            nsview: None,
            #[cfg(target_os = "macos")]
            gpui_nsview: None,
            #[cfg(target_os = "macos")]
            native_underlay_view: None,
            #[cfg(target_os = "macos")]
            native_backing_color: None,
            #[cfg(target_os = "macos")]
            native_underlay_color: None,
            #[cfg(target_os = "macos")]
            pending_native_layout: false,
            initialized: false,
            last_bounds: None,
            scale_factor: 1.0,
            last_title: None,
            last_cwd: cwd,
            pending_write: None,
            restored_screen_text,
            native_view_visible: Cell::new(true),
            surface_focused: Cell::new(true),
            awaiting_first_layout_visibility: false,
            process_exit_emitted: false,
            next_surface_init_retry_at: None,
            #[cfg(target_os = "macos")]
            last_mouse_position: None,
            #[cfg(target_os = "macos")]
            native_transition_underlay_visible: Cell::new(false),
            #[cfg(target_os = "macos")]
            native_transition_underlay_owner_id: NEXT_NATIVE_TRANSITION_OWNER_ID
                .fetch_add(1, Ordering::Relaxed),
            ime_marked_text: None,
        }
    }

    pub fn drain_surface_state(
        &mut self,
        sync_native_scroll: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ref terminal) = self.terminal else {
            return false;
        };

        let started = perf_trace_enabled().then(Instant::now);
        let mut changed = false;
        for event in terminal.take_pending_events() {
            changed = true;
            match event {
                GhosttySurfaceEvent::SplitRequest(direction) => {
                    cx.emit(GhosttySplitRequested(direction));
                }
                GhosttySurfaceEvent::OpenUrl(url) => {
                    cx.open_url(&url);
                }
                GhosttySurfaceEvent::PwdChanged(cwd) => {
                    self.last_cwd = Some(cwd.clone());
                    cx.emit(GhosttyCwdChanged(Some(cwd)));
                }
            }
        }

        let _ = terminal.take_needs_render();

        if !terminal.is_alive() && !self.process_exit_emitted {
            self.process_exit_emitted = true;
            changed = true;
            cx.emit(GhosttyProcessExited);
        }

        let title = terminal.title();
        if title != self.last_title {
            self.last_title = title.clone();
            changed = true;
            cx.emit(GhosttyTitleChanged(title));
        }

        let cwd = terminal.current_dir().or_else(|| self.initial_cwd.clone());
        if cwd != self.last_cwd {
            self.last_cwd = cwd.clone();
            changed = true;
            cx.emit(GhosttyCwdChanged(cwd));
        }

        #[cfg(target_os = "macos")]
        if sync_native_scroll {
            self.sync_native_scroll_view();
        }

        if let Some(started) = started {
            if changed {
                log::info!(
                    target: "con::perf",
                    "drain_surface_state changed=1 elapsed_ms={:.3}",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }

        changed
    }

    fn send_terminal_paste_payload(&self, payload: TerminalPastePayload) -> bool {
        let Some(terminal) = &self.terminal else {
            return false;
        };

        match payload {
            TerminalPastePayload::Text(text) if !text.is_empty() => {
                terminal.send_text(&text);
                true
            }
            TerminalPastePayload::ForwardCtrlV => {
                terminal.send_key(ffi::ghostty_input_key_s {
                    action: ffi::ghostty_input_action_e::GHOSTTY_ACTION_PRESS,
                    mods: ffi::GHOSTTY_MODS_CTRL,
                    consumed_mods: 0,
                    keycode: 0x09, // kVK_ANSI_V
                    text: std::ptr::null(),
                    unshifted_codepoint: 'v' as u32,
                    composing: false,
                });
                true
            }
            TerminalPastePayload::Text(_) => false,
        }
    }

    fn paste_from_clipboard(&self, cx: &mut Context<Self>) -> bool {
        let Some(payload) = cx
            .read_from_clipboard()
            .and_then(|item| payload_from_clipboard(&item))
        else {
            return false;
        };

        self.send_terminal_paste_payload(payload)
    }

    pub fn pump_deferred_work(&mut self, cx: &mut Context<Self>) -> bool {
        let now = Instant::now();
        let mut changed = false;

        if !self.initialized
            && self
                .next_surface_init_retry_at
                .is_some_and(|deadline| now >= deadline)
        {
            self.next_surface_init_retry_at = None;
            changed = true;
            cx.notify();
        }

        changed
    }

    pub fn terminal(&self) -> Option<&Arc<GhosttyTerminal>> {
        self.terminal.as_ref()
    }

    /// Queue data to write to the PTY. If the terminal is already initialized,
    /// writes immediately. Otherwise, buffers until `ensure_initialized()` runs.
    pub fn write_or_queue(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.restored_screen_text = None;
        }
        if let Some(ref terminal) = self.terminal {
            terminal.write_to_pty(data);
        } else {
            self.pending_write
                .get_or_insert_with(Vec::new)
                .extend_from_slice(data);
        }
    }

    pub fn title(&self) -> Option<String> {
        self.terminal.as_ref().and_then(|t| t.title())
    }

    pub fn current_dir(&self) -> Option<String> {
        self.terminal
            .as_ref()
            .and_then(|t| t.current_dir())
            .or_else(|| self.initial_cwd.clone())
    }

    pub fn is_alive(&self) -> bool {
        self.terminal.as_ref().is_some_and(|t| t.is_alive())
    }

    pub fn surface_ready(&self) -> bool {
        self.terminal.is_some()
    }

    fn show_layout_fallback(&self) -> bool {
        self.terminal.is_none() || self.awaiting_first_layout_visibility
    }

    #[allow(dead_code)]
    pub fn selection_text(&self) -> Option<String> {
        self.terminal.as_ref().and_then(|t| t.selection_text())
    }

    #[cfg(target_os = "macos")]
    fn detach_host_view(&mut self) {
        if let Some(underlay_view) = self.native_underlay_view {
            Self::set_transition_underlay_owner_visible(
                underlay_view,
                self.native_transition_underlay_owner_id,
                false,
            );
        }
        self.native_underlay_view = None;
        if let Some(nsview) = self.nsview {
            unsafe {
                con_ghostty_surface_remove_backing_observer(nsview as *mut c_void);
            }
        }
        if let Some(host_view) = self.host_view.take() {
            unsafe {
                let superview: id = msg_send![host_view, superview];
                if !superview.is_null() {
                    let _: () = msg_send![host_view, removeFromSuperview];
                    // Restore first responder to the GPUI Metal view (GPUIView) so
                    // keyboard events reach GPUI after the Ghostty NSView is gone.
                    if let Some(gpui_view) = self.gpui_nsview {
                        let window: id = msg_send![superview, window];
                        if !window.is_null() {
                            let _: () = msg_send![window, makeFirstResponder: gpui_view];
                        }
                    }
                }
            }
        }
        self.document_view = None;
        self.nsview = None;
        self.native_backing_color = None;
        self.native_underlay_color = None;
        self.pending_native_layout = false;
    }

    #[cfg(not(target_os = "macos"))]
    fn detach_host_view(&mut self) {}

    #[cfg(target_os = "macos")]
    fn transition_underlay_for_parent(parent_nsview: id) -> id {
        unsafe {
            let key = &NATIVE_TRANSITION_UNDERLAY_ASSOCIATION_KEY as *const u8 as *const c_void;
            let existing = objc_getAssociatedObject(parent_nsview, key);
            if !existing.is_null() {
                return existing;
            }

            let frame: NSRect = msg_send![parent_nsview, bounds];
            let underlay: id = msg_send![class!(NSView), alloc];
            let underlay: id = msg_send![underlay, initWithFrame:frame];
            let _: () = msg_send![underlay, setWantsLayer:YES];
            let _: () = msg_send![
                underlay,
                setAutoresizingMask:NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE
            ];
            let _: () = msg_send![underlay, setHidden:YES];
            let _: () = msg_send![
                parent_nsview,
                addSubview: underlay
                positioned: NSWindowOrderingMode::NSWindowBelow
                relativeTo: nil
            ];
            objc_setAssociatedObject(
                parent_nsview,
                key,
                underlay,
                OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
            underlay
        }
    }

    #[cfg(target_os = "macos")]
    fn transition_underlay_owners_for_parent(parent_nsview: id) -> id {
        unsafe {
            let key =
                &NATIVE_TRANSITION_UNDERLAY_OWNERS_ASSOCIATION_KEY as *const u8 as *const c_void;
            let existing = objc_getAssociatedObject(parent_nsview, key);
            if !existing.is_null() {
                return existing;
            }

            let owners: id = msg_send![class!(NSMutableSet), set];
            objc_setAssociatedObject(
                parent_nsview,
                key,
                owners,
                OBJC_ASSOCIATION_RETAIN_NONATOMIC,
            );
            owners
        }
    }

    #[cfg(target_os = "macos")]
    fn set_transition_underlay_owner_visible(underlay_view: id, owner_id: u64, visible: bool) {
        if underlay_view.is_null() {
            return;
        }

        unsafe {
            let parent_nsview: id = msg_send![underlay_view, superview];
            if parent_nsview.is_null() {
                return;
            }

            let owners = Self::transition_underlay_owners_for_parent(parent_nsview);
            let owner: id = msg_send![class!(NSNumber), numberWithUnsignedLongLong:owner_id];
            if visible {
                let _: () = msg_send![owners, addObject:owner];
            } else {
                let _: () = msg_send![owners, removeObject:owner];
            }

            let owner_count: usize = msg_send![owners, count];
            let hidden = if owner_count == 0 { YES } else { NO };
            let _: () = msg_send![underlay_view, setHidden:hidden];
            if hidden == NO {
                let _: () = msg_send![underlay_view, setNeedsDisplay:YES];
            }
        }
    }

    pub fn shutdown_surface(&mut self) {
        self.native_view_visible.set(false);

        if let Some(ref terminal) = self.terminal {
            terminal.set_focus(false);
            terminal.set_occlusion(true);
        }

        self.detach_host_view();
        self.terminal = None;
        self.initialized = false;
        self.awaiting_first_layout_visibility = false;
        self.last_bounds = None;
        self.last_title = None;
        self.pending_write = None;
        self.restored_screen_text = None;
        self.next_surface_init_retry_at = None;
        #[cfg(target_os = "macos")]
        {
            self.last_mouse_position = None;
            self.pending_native_layout = false;
            self.native_transition_underlay_visible.set(false);
        }
        self.ime_marked_text = None;
    }

    pub fn set_surface_focus_state(&mut self, focused: bool) {
        let changed = self.surface_focused.replace(focused) != focused;
        if !changed {
            return;
        }

        if let Some(ref terminal) = self.terminal {
            terminal.set_focus(focused);
            terminal.refresh();
        }
    }

    #[cfg(target_os = "macos")]
    fn ensure_initialized(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.initialized {
            return;
        }
        if self
            .next_surface_init_retry_at
            .is_some_and(|deadline| Instant::now() < deadline)
        {
            return;
        }

        self.scale_factor = window.scale_factor().max(f32::EPSILON);

        let raw_handle: raw_window_handle::WindowHandle<'_> =
            match HasWindowHandle::window_handle(window) {
                Ok(handle) => handle,
                Err(_) => return,
            };

        let gpui_nsview = match raw_handle.as_raw() {
            raw_window_handle::RawWindowHandle::AppKit(handle) => handle.ns_view.as_ptr() as id,
            _ => return,
        };

        // Cache the GPUI Metal view so detach_host_view can restore first responder to it.
        self.gpui_nsview = Some(gpui_nsview);

        let parent_nsview: id = unsafe {
            let superview: id = msg_send![gpui_nsview, superview];
            if superview.is_null() {
                gpui_nsview
            } else {
                superview
            }
        };

        // Create with a zero-origin frame — update_frame() will set the
        // correct flipped position immediately after initialization.
        let (host_view, document_view, nsview, underlay_view): (id, id, id, id) = unsafe {
            let frame = NSRect::new(
                cocoa::foundation::NSPoint::new(0.0, 0.0),
                cocoa::foundation::NSSize::new(
                    f64::from(bounds.size.width),
                    f64::from(bounds.size.height),
                ),
            );
            let host: id = msg_send![class!(NSScrollView), alloc];
            let host: id = msg_send![host, initWithFrame:frame];
            let _: () = msg_send![host, setDrawsBackground:NO];
            let _: () = msg_send![host, setHasVerticalScroller:NO];
            let _: () = msg_send![host, setHasHorizontalScroller:NO];
            let _: () = msg_send![host, setAutohidesScrollers:NO];
            let _: () = msg_send![host, setUsesPredominantAxisScrolling:YES];
            let _: () =
                msg_send![host, setAutoresizingMask:NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE];
            let _: () = msg_send![host, setAutoresizesSubviews:YES];
            let _: () = msg_send![
                host,
                setLayerContentsRedrawPolicy:NS_VIEW_LAYER_CONTENTS_REDRAW_DURING_VIEW_RESIZE
            ];
            let content_view: id = msg_send![host, contentView];
            let _: () = msg_send![content_view, setClipsToBounds:YES];
            let _: () = msg_send![host, setHidden:YES];
            let _: () = msg_send![
                parent_nsview,
                addSubview: host
                positioned: NSWindowOrderingMode::NSWindowBelow
                relativeTo: gpui_nsview
            ];

            let underlay = Self::transition_underlay_for_parent(parent_nsview);

            let document: id = msg_send![class!(NSView), alloc];
            let document: id = msg_send![document, initWithFrame:frame];
            let _: () = msg_send![host, setDocumentView:document];

            let surface: id = msg_send![class!(NSView), alloc];
            let surface: id = msg_send![surface, initWithFrame:frame];
            let _: () = msg_send![surface, setWantsLayer:YES];
            let _: () = msg_send![
                surface,
                setAutoresizingMask:NS_VIEW_WIDTH_SIZABLE | NS_VIEW_HEIGHT_SIZABLE
            ];
            let _: () = msg_send![
                surface,
                setLayerContentsRedrawPolicy:NS_VIEW_LAYER_CONTENTS_REDRAW_DURING_VIEW_RESIZE
            ];
            let _: () = msg_send![document, addSubview:surface];
            (host, document, surface, underlay)
        };

        let scale =
            Self::view_backing_scale(nsview, f64::from(window.scale_factor().max(f32::EPSILON)));
        self.scale_factor = scale as f32;
        match self.app.new_surface(
            nsview as *mut c_void,
            scale,
            self.initial_cwd.as_deref(),
            self.restored_screen_text.as_deref(),
            Some(self.initial_font_size),
        ) {
            Ok(terminal) => {
                terminal.set_focus(self.surface_focused.get());
                let terminal = Arc::new(terminal);
                self.terminal = Some(terminal.clone());
                self.restored_screen_text = None;
                self.host_view = Some(host_view);
                self.document_view = Some(document_view);
                self.nsview = Some(nsview);
                self.native_underlay_view = Some(underlay_view);
                self.initialized = true;
                self.next_surface_init_retry_at = None;
                unsafe {
                    con_ghostty_surface_install_backing_observer(
                        nsview as *mut c_void,
                        terminal.raw_surface() as *mut c_void,
                    );
                }
                self.sync_native_backing_background();
                self.apply_transition_underlay_visibility();
                self.sync_window_background_blur();
                let backing = self.sync_native_backing_properties(bounds, window);
                // Force update_frame() to position the newly-created host.
                // If a previous layout recorded the same bounds while the
                // surface was still pending, an early-return here leaves the
                // NSView at its bootstrap origin until a manual divider resize.
                self.last_bounds = None;
                let size = terminal.size();
                log::info!(
                    "Ghostty surface created: {}x{} px, scale {}",
                    backing.map_or(size.width_px, |backing| backing.width_px),
                    backing.map_or(size.height_px, |backing| backing.height_px),
                    self.scale_factor
                );
                self.sync_native_scroll_view();

                // Flush any data queued before the surface existed.
                if let Some(data) = self.pending_write.take() {
                    self.terminal.as_ref().unwrap().write_to_pty(&data);
                }
            }
            Err(e) => {
                log::error!("Failed to create ghostty surface: {}", e);
                // Discard queued writes — no PTY exists.
                self.pending_write = None;
                self.initialized = false;
                self.next_surface_init_retry_at = Some(Instant::now() + Duration::from_millis(250));
                self.host_view = Some(host_view);
                self.document_view = Some(document_view);
                self.nsview = Some(nsview);
                self.native_underlay_view = Some(underlay_view);
                self.detach_host_view();
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn native_backing_rgba(&self) -> Option<(u8, u8, u8, u8)> {
        let rgb = self.app.background_rgb()?;
        let alpha = if Self::needs_legacy_native_layer_geometry_sync() {
            // macOS 12 and older intentionally disable terminal glass. Keep
            // the AppKit backing opaque there so a temporarily desynced
            // layer never exposes the desktop behind the pane.
            255
        } else {
            // Modern macOS gets opacity/blur from Ghostty's own renderer and
            // NSWindow background blur. Painting the same translucent terminal
            // background behind the Metal surface double-composites it and
            // makes the configured glass effectively opaque.
            0
        };
        Some((rgb[0], rgb[1], rgb[2], alpha))
    }

    #[cfg(target_os = "macos")]
    fn native_underlay_rgba(&self) -> Option<(u8, u8, u8, u8)> {
        let rgb = self.app.background_rgb()?;
        Some((rgb[0], rgb[1], rgb[2], 255))
    }

    #[cfg(target_os = "macos")]
    fn apply_native_backing_color(view: id, rgba: (u8, u8, u8, u8)) {
        if view.is_null() {
            return;
        }

        unsafe {
            let _: () = msg_send![view, setWantsLayer:YES];
            let layer: id = msg_send![view, layer];
            if layer.is_null() {
                return;
            }

            let color: id = msg_send![
                class!(NSColor),
                colorWithSRGBRed:f64::from(rgba.0) / 255.0
                green:f64::from(rgba.1) / 255.0
                blue:f64::from(rgba.2) / 255.0
                alpha:f64::from(rgba.3) / 255.0
            ];
            let cg_color: id = msg_send![color, CGColor];
            let _: () = msg_send![layer, setBackgroundColor:cg_color];
        }
    }

    #[cfg(target_os = "macos")]
    fn sync_native_layer_geometry(view: id, contents_scale: f32) {
        if view.is_null() {
            return;
        }
        if !Self::needs_legacy_native_layer_geometry_sync() {
            return;
        }

        unsafe {
            let layer: id = msg_send![view, layer];
            if layer.is_null() {
                return;
            }

            // Ghostty turns the surface NSView into a layer-hosting view by
            // assigning its IOSurface CALayer directly. On older AppKit
            // compositors (Monterey), layer-hosting views can fail to keep
            // their hosted layer bounds in lockstep with command-driven
            // frame mutations, leaving only our opaque backing color visible.
            let bounds: NSRect = msg_send![view, bounds];
            let _: () = msg_send![layer, setFrame:bounds];
            let _: () = msg_send![layer, setBounds:bounds];
            let _: () = msg_send![layer, setContentsScale:f64::from(contents_scale)];
            let _: () = msg_send![layer, setNeedsDisplay];
        }
    }

    #[cfg(target_os = "macos")]
    fn needs_legacy_native_layer_geometry_sync() -> bool {
        static NEEDS_SYNC: OnceLock<bool> = OnceLock::new();
        *NEEDS_SYNC.get_or_init(|| unsafe {
            #[repr(C)]
            struct NSOperatingSystemVersion {
                major_version: isize,
                minor_version: isize,
                patch_version: isize,
            }

            let process_info: id = msg_send![class!(NSProcessInfo), processInfo];
            if process_info.is_null() {
                return false;
            }

            let version: NSOperatingSystemVersion = msg_send![process_info, operatingSystemVersion];
            version.major_version <= 12
        })
    }

    #[cfg(target_os = "macos")]
    fn sync_native_backing_background(&mut self) {
        let Some(rgba) = self.native_backing_rgba() else {
            return;
        };
        let has_native_views =
            self.host_view.is_some() || self.document_view.is_some() || self.nsview.is_some();
        if has_native_views && self.native_backing_color == Some(rgba) {
            return;
        }

        if let Some(host_view) = self.host_view {
            Self::apply_native_backing_color(host_view, rgba);
            unsafe {
                let content_view: id = msg_send![host_view, contentView];
                Self::apply_native_backing_color(content_view, rgba);
            }
        }
        if let Some(document_view) = self.document_view {
            Self::apply_native_backing_color(document_view, rgba);
        }
        if let Some(nsview) = self.nsview {
            Self::apply_native_backing_color(nsview, rgba);
            Self::sync_native_layer_geometry(nsview, self.scale_factor);
        }
        if let Some(underlay_view) = self.native_underlay_view
            && let Some(underlay_rgba) = self.native_underlay_rgba()
            && self.native_underlay_color != Some(underlay_rgba)
        {
            Self::apply_native_backing_color(underlay_view, underlay_rgba);
            self.native_underlay_color = Some(underlay_rgba);
        }
        if has_native_views {
            self.native_backing_color = Some(rgba);
        }
    }

    #[cfg(target_os = "macos")]
    pub fn set_transition_underlay_visible(&self, visible: bool) {
        self.native_transition_underlay_visible.set(visible);
        self.apply_transition_underlay_visibility();
    }

    #[cfg(target_os = "macos")]
    fn apply_transition_underlay_visibility(&self) {
        if let Some(underlay_view) = self.native_underlay_view {
            Self::set_transition_underlay_owner_visible(
                underlay_view,
                self.native_transition_underlay_owner_id,
                self.native_transition_underlay_visible.get(),
            );
        }
    }

    #[cfg(target_os = "macos")]
    pub fn mark_native_layout_pending(&mut self, cx: &mut Context<Self>) {
        self.pending_native_layout = true;
        cx.notify();
    }

    #[cfg(target_os = "macos")]
    pub fn ensure_initialized_for_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let showed_layout_fallback = self.show_layout_fallback();
        let Some(bounds) = self.last_bounds else {
            // Never create a native NSView from estimated bounds. Nested split
            // and zoom transitions move panes between flex subtrees; a bootstrap
            // frame can survive long enough to paint in the wrong pane until a
            // manual divider resize corrects it. The GPUI fallback matte covers
            // this pane until the canvas supplies authoritative layout bounds.
            self.awaiting_first_layout_visibility = true;
            cx.notify();
            return;
        };

        self.ensure_initialized(bounds, window, cx);
        self.update_frame(bounds, window);
        if showed_layout_fallback != self.show_layout_fallback() {
            cx.notify();
        }
    }

    #[cfg(target_os = "macos")]
    pub fn sync_surface_layout_for_host(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let showed_layout_fallback = self.show_layout_fallback();
        self.ensure_initialized(bounds, window, cx);
        self.update_frame(bounds, window);
        if showed_layout_fallback != self.show_layout_fallback() {
            cx.notify();
        }
    }

    #[cfg(target_os = "macos")]
    fn view_backing_scale(view: id, fallback_scale: f64) -> f64 {
        if view.is_null() {
            return fallback_scale.max(f64::EPSILON);
        }

        unsafe {
            let unit = cocoa::foundation::NSSize::new(1.0, 1.0);
            let backing: cocoa::foundation::NSSize = msg_send![view, convertSizeToBacking:unit];
            if backing.height.is_finite() && backing.height > 0.0 {
                backing.height
            } else if backing.width.is_finite() && backing.width > 0.0 {
                backing.width
            } else {
                fallback_scale.max(f64::EPSILON)
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn cached_scale_matches_native(&self, window: &Window) -> bool {
        let Some(nsview) = self.nsview else {
            return true;
        };
        let fallback_scale = f64::from(window.scale_factor().max(f32::EPSILON));
        let native_scale = Self::view_backing_scale(nsview, fallback_scale);
        (f64::from(self.scale_factor) - native_scale).abs() <= 0.001
    }

    #[cfg(target_os = "macos")]
    fn sync_native_backing_properties(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &Window,
    ) -> Option<NativeBackingSync> {
        let Some(nsview) = self.nsview else {
            self.scale_factor = window.scale_factor().max(f32::EPSILON);
            return None;
        };
        let Some(terminal) = self.terminal.clone() else {
            self.scale_factor = Self::view_backing_scale(
                nsview,
                f64::from(window.scale_factor().max(f32::EPSILON)),
            ) as f32;
            return None;
        };

        let before_size = terminal.size();
        let previous_scale = self.scale_factor;
        let mut scale_x = 0.0;
        let mut scale_y = 0.0;
        let mut width_px = 0;
        let mut height_px = 0;
        let fallback_scale =
            Self::view_backing_scale(nsview, f64::from(window.scale_factor().max(f32::EPSILON)));

        let ok = unsafe {
            con_ghostty_surface_install_backing_observer(
                nsview as *mut c_void,
                terminal.raw_surface() as *mut c_void,
            );
            con_ghostty_surface_sync_backing(
                nsview as *mut c_void,
                terminal.raw_surface() as *mut c_void,
                f64::from(bounds.size.width.as_f32().max(1.0)),
                f64::from(bounds.size.height.as_f32().max(1.0)),
                fallback_scale,
                &mut scale_x,
                &mut scale_y,
                &mut width_px,
                &mut height_px,
            )
        };
        if !ok {
            return None;
        }

        self.scale_factor = scale_y.max(f64::EPSILON) as f32;

        let scale_changed = (previous_scale - self.scale_factor).abs() > 0.001;
        let size_changed = before_size.width_px != width_px || before_size.height_px != height_px;
        let result = NativeBackingSync {
            width_px,
            height_px,
        };

        // Do not force a draw from this pre-frame sync path. Ghostty's Metal
        // renderer reads the hosted CALayer bounds at draw time; during live
        // resize those bounds are only final after `sync_native_scroll_view()`
        // commits the AppKit host/document/surface frames below.

        if perf_trace_enabled() && (scale_changed || size_changed) {
            let after_size = terminal.size();
            log::info!(
                target: "con::perf",
                "native backing sync scale={:.3}x{:.3}->{:.3}x{:.3} old_px={}x{} old_grid={}x{} new_px={}x{} new_grid={}x{} logical_pt={:.1}x{:.1}",
                f64::from(previous_scale),
                f64::from(previous_scale),
                scale_x,
                scale_y,
                before_size.width_px,
                before_size.height_px,
                before_size.columns,
                before_size.rows,
                after_size.width_px,
                after_size.height_px,
                after_size.columns,
                after_size.rows,
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32()
            );
        }

        Some(result)
    }

    #[cfg(target_os = "macos")]
    fn update_frame(&mut self, bounds: Bounds<Pixels>, window: &Window) {
        // `last_bounds` is Con's layout cache, not Ghostty's protocol state.
        // Pane-local surfaces can be hidden, focused, and resized without
        // repainting every sibling; always verify the embedded surface still
        // reports the pixel size that maps to these bounds before skipping.
        if self.last_bounds.as_ref() == Some(&bounds)
            && !self.awaiting_first_layout_visibility
            && !self.pending_native_layout
            && self.terminal_matches_bounds(bounds)
            && self.cached_scale_matches_native(window)
        {
            return;
        }
        let started = perf_trace_enabled().then(Instant::now);
        self.last_bounds = Some(bounds);
        self.sync_native_backing_background();

        // Keep libghostty's framebuffer/PTY metadata ahead of every AppKit
        // frame commit, but do not draw yet. Ghostty's Metal renderer uses the
        // current IOSurface CALayer bounds at draw time, so the synchronous
        // draw must happen after the host/document/surface frames are final.
        self.sync_native_backing_properties(bounds, window);

        if let Some(host_view) = self.host_view {
            unsafe {
                let superview: id = msg_send![host_view, superview];
                let super_frame: NSRect = msg_send![superview, frame];
                if let Some(underlay_view) = self.native_underlay_view {
                    let super_bounds: NSRect = msg_send![superview, bounds];
                    let _: () = msg_send![underlay_view, setFrame:super_bounds];
                }
                let overdraw = NATIVE_SEAM_OVERDRAW_PT;
                let flipped_y = super_frame.size.height
                    - f64::from(bounds.origin.y)
                    - f64::from(bounds.size.height)
                    - overdraw;

                let frame = NSRect::new(
                    cocoa::foundation::NSPoint::new(
                        f64::from(bounds.origin.x) - overdraw,
                        flipped_y,
                    ),
                    cocoa::foundation::NSSize::new(
                        f64::from(bounds.size.width) + overdraw * 2.0,
                        f64::from(bounds.size.height) + overdraw * 2.0,
                    ),
                );
                let _: () = msg_send![host_view, setFrame:frame];
            }
        }

        self.sync_native_scroll_view();

        if let Some(started) = started {
            let in_live_resize = if let Some(host_view) = self.host_view {
                unsafe {
                    let nswindow: id = msg_send![host_view, window];
                    if nswindow.is_null() {
                        false
                    } else {
                        let live: cocoa::base::BOOL = msg_send![nswindow, inLiveResize];
                        live == YES
                    }
                }
            } else {
                false
            };
            log::info!(
                target: "con::perf",
                "update_frame logical_pt={:.1}x{:.1} in_live_resize={} elapsed_ms={:.3}",
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32(),
                in_live_resize,
                started.elapsed().as_secs_f64() * 1000.0
            );
        }

        if self.awaiting_first_layout_visibility {
            self.awaiting_first_layout_visibility = false;
        }
        self.pending_native_layout = false;
        self.apply_native_visibility();
    }

    #[cfg(target_os = "macos")]
    fn sync_native_scroll_view(&mut self) {
        let (Some(scroll_view), Some(document_view), Some(nsview), Some(bounds), Some(terminal)) = (
            self.host_view,
            self.document_view,
            self.nsview,
            self.last_bounds,
            self.terminal.as_ref(),
        ) else {
            return;
        };

        let visible_width = f64::from(bounds.size.width.as_f32().max(1.0));
        let visible_height = f64::from(bounds.size.height.as_f32().max(1.0));
        let overdraw = NATIVE_SEAM_OVERDRAW_PT;
        let size = terminal.size();
        let cell_height = if size.cell_height_px > 0 && self.scale_factor > 0.0 {
            f64::from(size.cell_height_px) / f64::from(self.scale_factor)
        } else {
            0.0
        };
        let scrollbar = terminal.scrollbar();
        let document_height = if let Some(scrollbar) = scrollbar {
            let total_rows = scrollbar.total.max(scrollbar.len).max(1);
            let visible_rows = scrollbar.len.max(1).min(total_rows);
            let padding = if cell_height > 0.0 {
                (visible_height - (visible_rows as f64 * cell_height)).max(0.0)
            } else {
                0.0
            };
            if cell_height > 0.0 {
                (total_rows as f64 * cell_height + padding).max(visible_height)
            } else {
                visible_height
            }
        } else {
            visible_height
        };

        unsafe {
            let document_frame = NSRect::new(
                cocoa::foundation::NSPoint::new(0.0, 0.0),
                cocoa::foundation::NSSize::new(
                    visible_width + overdraw * 2.0,
                    document_height + overdraw * 2.0,
                ),
            );
            let _: () = msg_send![document_view, setFrame:document_frame];

            let content_view: id = msg_send![scroll_view, contentView];
            let scroll_y = if let Some(scrollbar) = scrollbar {
                let total_rows = scrollbar.total.max(scrollbar.len).max(1);
                let visible_rows = scrollbar.len.max(1).min(total_rows);
                let offset_from_bottom = if cell_height > 0.0 {
                    (total_rows
                        .saturating_sub(scrollbar.offset.min(total_rows))
                        .saturating_sub(visible_rows)) as f64
                        * cell_height
                } else {
                    0.0
                };
                offset_from_bottom.clamp(0.0, (document_height - visible_height).max(0.0))
            } else {
                0.0
            };
            let _: () = msg_send![
                content_view,
                scrollToPoint:cocoa::foundation::NSPoint::new(0.0, scroll_y)
            ];
            let _: () = msg_send![scroll_view, reflectScrolledClipView:content_view];

            // Do not read `documentVisibleRect` here and do not skip this frame
            // mutation through a local tuple cache. Con drives this AppKit
            // hierarchy from GPUI layout callbacks, so split/zoom topology
            // changes can leave AppKit's computed geometry or an old host
            // placement stale until the next layout pass. The terminal surface
            // frame must be deterministically reapplied from Con-owned pane
            // bounds plus Ghostty's scrollbar state.
            let surface_frame = NSRect::new(
                cocoa::foundation::NSPoint::new(overdraw, scroll_y + overdraw),
                cocoa::foundation::NSSize::new(visible_width, visible_height),
            );
            let _: () = msg_send![nsview, setFrame:surface_frame];
            Self::sync_native_layer_geometry(nsview, self.scale_factor);
        }
    }

    fn on_layout(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let showed_layout_fallback = self.show_layout_fallback();
        #[cfg(target_os = "macos")]
        {
            self.ensure_initialized(bounds, window, cx);
            self.update_frame(bounds, window);
        }
        if showed_layout_fallback != self.show_layout_fallback() {
            cx.notify();
        }
    }

    #[cfg(target_os = "macos")]
    fn surface_size_in_backing_pixels(&self, bounds: Bounds<Pixels>) -> (u32, u32) {
        let logical_size = cocoa::foundation::NSSize::new(
            f64::from(bounds.size.width.as_f32().max(1.0)),
            f64::from(bounds.size.height.as_f32().max(1.0)),
        );

        if let Some(nsview) = self.nsview {
            unsafe {
                let backing: cocoa::foundation::NSSize =
                    msg_send![nsview, convertSizeToBacking: logical_size];
                return (
                    backing.width.max(1.0).round() as u32,
                    backing.height.max(1.0).round() as u32,
                );
            }
        }

        (
            (logical_size.width * f64::from(self.scale_factor))
                .max(1.0)
                .round() as u32,
            (logical_size.height * f64::from(self.scale_factor))
                .max(1.0)
                .round() as u32,
        )
    }

    #[cfg(target_os = "macos")]
    fn terminal_matches_bounds(&self, bounds: Bounds<Pixels>) -> bool {
        let Some(ref terminal) = self.terminal else {
            return false;
        };
        let (width_px, height_px) = self.surface_size_in_backing_pixels(bounds);
        let size = terminal.size();
        let matches = size.width_px == width_px && size.height_px == height_px;
        if !matches && perf_trace_enabled() {
            log::info!(
                target: "con::perf",
                "surface geometry mismatch terminal_px={}x{} terminal_grid={}x{} expected_px={}x{} logical_pt={:.1}x{:.1}",
                size.width_px,
                size.height_px,
                size.columns,
                size.rows,
                width_px,
                height_px,
                bounds.size.width.as_f32(),
                bounds.size.height.as_f32()
            );
        }
        matches
    }

    /// Show or hide the native NSView. Used to manage z-order when
    /// GPUI overlays (settings, command palette) need to appear on top.
    #[cfg(target_os = "macos")]
    pub fn set_visible(&self, visible: bool) {
        self.native_view_visible.set(visible);
        self.apply_native_visibility();
    }

    #[cfg(target_os = "macos")]
    fn apply_native_visibility(&self) {
        if let Some(host_view) = self.host_view {
            unsafe {
                let effective_visible =
                    self.native_view_visible.get() && !self.awaiting_first_layout_visibility;
                let hidden = if effective_visible { NO } else { YES };
                let _: () = msg_send![host_view, setHidden:hidden];
                if effective_visible {
                    let _: () = msg_send![host_view, setNeedsDisplay:YES];
                }
            }
        }
        if self.native_view_visible.get() && !self.awaiting_first_layout_visibility {
            self.draw_surface_now();
        }
    }

    #[cfg(target_os = "macos")]
    fn draw_surface_now(&self) {
        let Some(terminal) = self.terminal.as_ref() else {
            return;
        };
        terminal.draw();
    }

    #[cfg(target_os = "macos")]
    pub fn sync_window_background_blur(&mut self) {
        self.sync_native_backing_background();

        let Some(host_view) = self.host_view else {
            return;
        };

        unsafe {
            let nswindow: id = msg_send![host_view, window];
            if nswindow.is_null() {
                return;
            }
            ffi::ghostty_set_window_background_blur(self.app.raw(), nswindow as *mut c_void);
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn sync_window_background_blur(&self) {}

    /// Convert GPUI window-global position to view-local logical coordinates.
    /// Ghostty expects logical pixels (it scales internally via content_scale).
    fn view_local_pos(&self, pos: Point<Pixels>) -> (f64, f64) {
        if let Some(ref bounds) = self.last_bounds {
            (
                f64::from(pos.x - bounds.origin.x),
                f64::from(pos.y - bounds.origin.y),
            )
        } else {
            (f64::from(pos.x), f64::from(pos.y))
        }
    }

    #[cfg(target_os = "macos")]
    fn refresh_mouse_modifiers(&self, modifiers: &Modifiers) {
        let (Some(terminal), Some(position)) = (self.terminal.as_ref(), self.last_mouse_position)
        else {
            return;
        };

        let (x, y) = self.view_local_pos(position);
        terminal.send_mouse_pos(x, y, gpui_mods_to_ghostty(modifiers));
    }

    /// Handle key input by forwarding to ghostty's key processing pipeline.
    ///
    /// Uses `ghostty_surface_key` with macOS virtual keycodes so ghostty handles
    /// mode-dependent sequences (application cursor mode, kitty protocol, etc.)
    /// correctly. Falls back to `ghostty_surface_text` for composed/IME text
    /// when no keycode mapping exists.
    fn handle_key_down(
        &self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let terminal = match self.terminal.as_ref() {
            Some(t) => t,
            None => return false,
        };

        let keystroke = &event.keystroke;

        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "c" => {
                    if terminal.has_selection() {
                        if let Some(selection) = terminal.selection_text() {
                            cx.write_to_clipboard(ClipboardItem::new_string(selection));
                        }
                    }
                    return true;
                }
                "v" => {
                    if self.paste_from_clipboard(cx) {
                        cx.notify();
                    }
                    return true;
                }
                _ => {}
            }
        }

        if crate::terminal_shortcuts::key_down_starts_action_binding(
            event,
            window,
            &crate::TogglePaneZoom,
        ) || crate::terminal_shortcuts::key_down_starts_action_binding(
            event,
            window,
            &crate::FocusFiles,
        ) || crate::terminal_shortcuts::key_down_starts_action_binding(
            event,
            window,
            &crate::SearchFiles,
        ) {
            return false;
        }

        // App-level shortcuts — skip forwarding so GPUI action dispatch handles them.
        // All other Cmd/Ctrl combos pass through to ghostty (e.g. cmd-k for clear screen).
        if keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                // Tab management
                "q" | "w" | "t" | "," | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                    return false;
                }
                // Window management (cmd-m, cmd-`, cmd-shift-`)
                "m" | "`" | "~" | ">" | "<" => return false,
                // Splits (cmd-d, cmd-shift-d)
                "d" => return false,
                // Agent & input
                "l" | "i" => return false,
                // Edit menu (handled by OS)
                "c" | "v" | "x" | "z" | "a" => return false,
                // Command palette (cmd-shift-p)
                "p" if keystroke.modifiers.shift => return false,
                // Everything else (including cmd-k) passes to terminal
                _ => {}
            }
        }

        // Ctrl+` is reserved for toggle-input-bar (app shortcut).
        if keystroke.modifiers.control && keystroke.key == "`" {
            return false;
        }

        let mods = gpui_mods_to_ghostty(&keystroke.modifiers);
        let key_name = keystroke.key.as_str();

        // Try to map GPUI key name to macOS virtual keycode.
        if let Some((keycode, unshifted_codepoint)) = gpui_key_to_keycode(key_name) {
            // Build the text field: the character this key produces (if printable).
            // For non-printable keys (arrows, F-keys), text is null.
            let text_string = keystroke.key_char.as_deref().or_else(|| {
                if key_name.len() == 1 {
                    Some(key_name)
                } else {
                    None
                }
            });
            let cstr = text_string.and_then(|s| std::ffi::CString::new(s).ok());
            let text_ptr = cstr
                .as_ref()
                .map(|c| c.as_ptr())
                .unwrap_or(std::ptr::null());

            let key_event = ffi::ghostty_input_key_s {
                action: ffi::ghostty_input_action_e::GHOSTTY_ACTION_PRESS,
                mods,
                consumed_mods: 0,
                keycode,
                text: text_ptr,
                unshifted_codepoint,
                composing: false,
            };

            terminal.send_key(key_event);
            return true;
        }

        // No keycode mapping — fall back to text input.
        // This handles unusual keys and compose sequences.
        if let Some(ref key_char) = keystroke.key_char {
            if !key_char.is_empty() {
                terminal.send_text(key_char);
                return true;
            }
        }
        if key_name.len() == 1 {
            terminal.send_text(key_name);
            return true;
        }
        false
    }
}

struct GhosttyInputHandler {
    view: WeakEntity<GhosttyView>,
}

impl InputHandler for GhosttyInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&mut self, _window: &mut Window, cx: &mut App) -> Option<Range<usize>> {
        self.view
            .read_with(cx, |view, _| {
                view.ime_marked_text
                    .as_ref()
                    .map(|text| 0..text.encode_utf16().count())
            })
            .ok()
            .flatten()
    }

    fn text_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<String> {
        *adjusted_range = Some(0..0);
        Some(String::new())
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) {
        if text.is_empty() {
            return;
        }
        let _ = self.view.update(cx, |view, cx| {
            let had_marked_text = view.ime_marked_text.take().is_some();
            if let Some(terminal) = &view.terminal {
                if !had_marked_text && should_send_ime_insert_as_key_event(text) {
                    send_ime_insert_as_key_events(terminal, text);
                } else {
                    // Use key event path (not ghostty_surface_text) to avoid
                    // bracketed-paste wrapping and post-paste selection highlight.
                    terminal.send_text_as_key_event(text);
                }
                terminal.refresh();
            }
            cx.notify();
        });
        window.invalidate_character_coordinates();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _ = self.view.update(cx, |view, cx| {
            view.ime_marked_text = if new_text.is_empty() {
                None
            } else {
                Some(new_text.to_string())
            };
            if let Some(terminal) = &view.terminal {
                terminal.refresh();
            }
            cx.notify();
        });
        window.invalidate_character_coordinates();
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut App) {
        let _ = self.view.update(cx, |view, cx| {
            view.ime_marked_text = None;
            if let Some(terminal) = &view.terminal {
                terminal.refresh();
            }
            cx.notify();
        });
        window.invalidate_character_coordinates();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        self.view
            .read_with(cx, |view, _| {
                let terminal = view.terminal.as_ref()?;
                let bounds = view.last_bounds?;
                let ime_point = terminal.ime_point();
                Some(Bounds::new(
                    point(
                        bounds.origin.x + px(ime_point.x as f32),
                        bounds.origin.y + px(ime_point.y as f32),
                    ),
                    size(
                        px((ime_point.width as f32).max(1.0)),
                        px((ime_point.height as f32).max(1.0)),
                    ),
                ))
            })
            .ok()
            .flatten()
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        Some(0)
    }

    fn prefers_ime_for_printable_keys(&mut self, _window: &mut Window, _cx: &mut App) -> bool {
        true
    }
}

fn should_send_ime_insert_as_key_event(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|ch| ch.is_ascii() && !ch.is_control())
}

fn send_ime_insert_as_key_events(terminal: &GhosttyTerminal, text: &str) {
    for ch in text.chars() {
        let mut buffer = [0; 4];
        terminal.send_text_as_key_event(ch.encode_utf8(&mut buffer));
    }
}

// ── GPUI → ghostty modifier mapping ─────────────────────────

fn gpui_mods_to_ghostty(mods: &Modifiers) -> i32 {
    let mut m: i32 = 0;
    if mods.shift {
        m |= ffi::GHOSTTY_MODS_SHIFT;
    }
    if mods.control {
        m |= ffi::GHOSTTY_MODS_CTRL;
    }
    if mods.alt {
        m |= ffi::GHOSTTY_MODS_ALT;
    }
    if mods.platform {
        m |= ffi::GHOSTTY_MODS_SUPER;
    }
    m
}

fn gpui_scroll_mods_to_ghostty(delta: &ScrollDelta) -> i32 {
    match delta {
        ScrollDelta::Pixels(_) => ffi::GHOSTTY_SCROLL_MODS_PRECISION,
        ScrollDelta::Lines(_) => 0,
    }
}

// ── GPUI key name → macOS virtual keycode mapping ────────────
//
// These are the kVK_* constants from Carbon/HIToolbox/Events.h.
// GPUI gives us key names (strings); ghostty needs raw keycodes
// so it can process them through its key binding and terminal
// mode pipeline (DECCKM, kitty keyboard protocol, etc.)

fn gpui_key_to_keycode(key: &str) -> Option<(u32, u32)> {
    Some(match key {
        // Letters (macOS kVK_ANSI_* — NOT sequential, based on QWERTY position)
        "a" => (0x00, 'a' as u32),
        "s" => (0x01, 's' as u32),
        "d" => (0x02, 'd' as u32),
        "f" => (0x03, 'f' as u32),
        "h" => (0x04, 'h' as u32),
        "g" => (0x05, 'g' as u32),
        "z" => (0x06, 'z' as u32),
        "x" => (0x07, 'x' as u32),
        "c" => (0x08, 'c' as u32),
        "v" => (0x09, 'v' as u32),
        "b" => (0x0B, 'b' as u32),
        "q" => (0x0C, 'q' as u32),
        "w" => (0x0D, 'w' as u32),
        "e" => (0x0E, 'e' as u32),
        "r" => (0x0F, 'r' as u32),
        "y" => (0x10, 'y' as u32),
        "t" => (0x11, 't' as u32),
        "o" => (0x1F, 'o' as u32),
        "u" => (0x20, 'u' as u32),
        "i" => (0x22, 'i' as u32),
        "p" => (0x23, 'p' as u32),
        "l" => (0x25, 'l' as u32),
        "j" => (0x26, 'j' as u32),
        "k" => (0x28, 'k' as u32),
        "n" => (0x2D, 'n' as u32),
        "m" => (0x2E, 'm' as u32),
        // Numbers
        "1" | "!" => (0x12, '1' as u32),
        "2" | "@" => (0x13, '2' as u32),
        "3" | "#" => (0x14, '3' as u32),
        "4" | "$" => (0x15, '4' as u32),
        "5" | "%" => (0x17, '5' as u32),
        "6" | "^" => (0x16, '6' as u32),
        "7" | "&" => (0x1A, '7' as u32),
        "8" | "*" => (0x1C, '8' as u32),
        "9" | "(" => (0x19, '9' as u32),
        "0" | ")" => (0x1D, '0' as u32),
        // Punctuation
        "-" | "_" => (0x1B, '-' as u32),
        "=" | "+" => (0x18, '=' as u32),
        "[" | "{" => (0x21, '[' as u32),
        "]" | "}" => (0x1E, ']' as u32),
        "\\" | "|" => (0x2A, '\\' as u32),
        ";" | ":" => (0x29, ';' as u32),
        "'" | "\"" => (0x27, '\'' as u32),
        "`" | "~" => (0x32, '`' as u32),
        "," | "<" => (0x2B, ',' as u32),
        "." | ">" => (0x2F, '.' as u32),
        "/" | "?" => (0x2C, '/' as u32),
        // Special keys
        "enter" | "return" => (0x24, 0),
        "tab" => (0x30, '\t' as u32),
        "space" => (0x31, ' ' as u32),
        "backspace" => (0x33, 0),
        "escape" => (0x35, 0),
        "delete" => (0x75, 0),
        "home" => (0x73, 0),
        "end" => (0x77, 0),
        "pageup" => (0x74, 0),
        "pagedown" => (0x79, 0),
        "up" => (0x7E, 0),
        "down" => (0x7D, 0),
        "left" => (0x7B, 0),
        "right" => (0x7C, 0),
        // Function keys
        "f1" => (0x7A, 0),
        "f2" => (0x78, 0),
        "f3" => (0x63, 0),
        "f4" => (0x76, 0),
        "f5" => (0x60, 0),
        "f6" => (0x61, 0),
        "f7" => (0x62, 0),
        "f8" => (0x64, 0),
        "f9" => (0x65, 0),
        "f10" => (0x6D, 0),
        "f11" => (0x67, 0),
        "f12" => (0x6F, 0),
        _ => return None,
    })
}

impl Drop for GhosttyView {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            // The AppKit backing observer stores Ghostty's raw surface pointer.
            // Detach it before `terminal` is dropped so a late screen/backing
            // notification cannot call into freed libghostty state.
            self.detach_host_view();
        }
        self.host_view = None;
    }
}

impl Focusable for GhosttyView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GhosttyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focus = self.focus_handle.clone();
        let input_focus = focus.clone();
        let context_focus = focus.clone();
        let menu_focus = focus.clone();
        // Set by the Right mouse-down handler when libghostty consumed the
        // click (mouse reporting active); read by the context-menu builder
        // to suppress con's menu. Mirrors Ghostty's AppKit host.
        let right_click_consumed = Rc::new(Cell::new(false));
        let ui_font = cx.theme().font_family.clone();
        let mono_font = cx.theme().mono_font_family.clone();
        let mono_font_size = cx.theme().mono_font_size;
        let entity = cx.entity().downgrade();
        let show_layout_fallback = self.show_layout_fallback();

        // Preedit overlay: render IME composition text at the cursor position.
        let preedit_overlay: Option<gpui::AnyElement> = self
            .ime_marked_text
            .as_deref()
            .filter(|t| !t.is_empty())
            .and_then(|preedit_text| {
                let terminal = self.terminal.as_ref()?;
                let bounds = self.last_bounds?;
                let ime = terminal.ime_point();
                let fg = self
                    .app
                    .foreground_rgb()
                    .map(|rgb| {
                        Rgba {
                            r: rgb[0] as f32 / 255.0,
                            g: rgb[1] as f32 / 255.0,
                            b: rgb[2] as f32 / 255.0,
                            a: 1.0,
                        }
                        .into()
                    })
                    .unwrap_or(cx.theme().foreground);
                let bg = self
                    .app
                    .background_rgb()
                    .map(|rgb| {
                        // Force opaque so the overlay covers ghostty's real cursor.
                        Rgba {
                            r: rgb[0] as f32 / 255.0,
                            g: rgb[1] as f32 / 255.0,
                            b: rgb[2] as f32 / 255.0,
                            a: 1.0,
                        }
                        .into()
                    })
                    .unwrap_or(cx.theme().background);
                let left = px(ime.x as f32)
                    .min(bounds.size.width - px(1.0))
                    .max(px(0.0));
                // ime_point y is the bottom of the cursor cell (candidate anchor).
                // Subtract cell height to align the overlay with the cursor row.
                let cell_height = if ime.height > 0.0 {
                    ime.height as f32
                } else {
                    f32::from(mono_font_size)
                };
                let top = px(ime.y as f32 - cell_height)
                    .min(bounds.size.height - mono_font_size)
                    .max(px(0.0));
                let text = preedit_text.to_string();
                Some(
                    div()
                        .absolute()
                        .left(left)
                        .top(top)
                        .bg(bg)
                        .text_color(fg)
                        .font_family(mono_font.clone())
                        .text_size(mono_font_size)
                        .underline()
                        .child(text)
                        .into_any_element(),
                )
            });

        let layout_fallback_bg = self
            .app
            .background_rgb()
            .map(|rgb| {
                let alpha = self.app.background_opacity().unwrap_or(1.0).clamp(0.0, 1.0);
                Rgba {
                    r: f32::from(rgb[0]) / 255.0,
                    g: f32::from(rgb[1]) / 255.0,
                    b: f32::from(rgb[2]) / 255.0,
                    a: alpha,
                }
                .into()
            })
            .unwrap_or_else(|| cx.theme().background);

        div()
            .size_full()
            .font_family(ui_font)
            .map(|div| {
                if show_layout_fallback {
                    div.bg(layout_fallback_bg)
                } else {
                    div
                }
            })
            .key_context("GhosttyTerminal")
            .track_focus(&focus)
            // Consume Tab/Shift-Tab so Root's focus cycling doesn't intercept them.
            // The actual Tab key is forwarded to ghostty via on_key_down below.
            .on_action(cx.listener(|this, _: &ConsumeTab, window, _cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                // Send Tab to terminal
                if let Some(terminal) = &this.terminal {
                    let key_event = ffi::ghostty_input_key_s {
                        action: ffi::ghostty_input_action_e::GHOSTTY_ACTION_PRESS,
                        mods: 0,
                        consumed_mods: 0,
                        keycode: 0x30, // Tab
                        text: b"\t\0".as_ptr() as *const _,
                        unshifted_codepoint: '\t' as u32,
                        composing: false,
                    };
                    terminal.send_key(key_event);
                }
            }))
            .on_action(cx.listener(|this, _: &ConsumeTabPrev, window, _cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                // Send Shift-Tab (backtab) to terminal
                if let Some(terminal) = &this.terminal {
                    let key_event = ffi::ghostty_input_key_s {
                        action: ffi::ghostty_input_action_e::GHOSTTY_ACTION_PRESS,
                        mods: ffi::GHOSTTY_MODS_SHIFT,
                        consumed_mods: 0,
                        keycode: 0x30, // Tab
                        text: std::ptr::null(),
                        unshifted_codepoint: '\t' as u32,
                        composing: false,
                    };
                    terminal.send_key(key_event);
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Copy, _window, cx| {
                let Some(terminal) = &this.terminal else {
                    return;
                };
                if !terminal.has_selection() {
                    return;
                }
                if let Some(selection) = terminal.selection_text() {
                    cx.write_to_clipboard(ClipboardItem::new_string(selection));
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Paste, _window, cx| {
                this.restored_screen_text = None;
                if this.paste_from_clipboard(cx) {
                    cx.notify();
                }
            }))
            .drag_over::<ExternalPaths>(|style, _, _, _| style)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let Some(payload) = payload_from_external_paths(paths) else {
                    return;
                };
                window.focus(&this.focus_handle, cx);
                this.restored_screen_text = None;
                if this.send_terminal_paste_payload(payload) {
                    cx.emit(GhosttyFocusChanged);
                    cx.notify();
                }
            }))
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener({
                    let right_click_consumed = right_click_consumed.clone();
                    move |this, event: &MouseDownEvent, window, cx| {
                        window.focus(&context_focus, cx);
                        this.last_mouse_position = Some(event.position);
                        if let Some(ref terminal) = this.terminal {
                            let (x, y) = this.view_local_pos(event.position);
                            let mods = gpui_mods_to_ghostty(&event.modifiers);
                            terminal.send_mouse_pos(x, y, mods);
                            let consumed =
                                terminal.send_mouse_button(true, MouseButton::Right, mods);
                            right_click_consumed.set(consumed);
                        }
                        cx.emit(GhosttyFocusChanged);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&focus, cx);
                    this.last_mouse_position = Some(event.position);
                    if let Some(ref terminal) = this.terminal {
                        let (x, y) = this.view_local_pos(event.position);
                        let mods = gpui_mods_to_ghostty(&event.modifiers);
                        terminal.send_mouse_pos(x, y, mods);
                        terminal.send_mouse_button(true, MouseButton::Left, mods);
                    }
                    cx.emit(GhosttyFocusChanged);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.last_mouse_position = Some(event.position);
                    if let Some(ref terminal) = this.terminal {
                        let (x, y) = this.view_local_pos(event.position);
                        let mods = gpui_mods_to_ghostty(&event.modifiers);
                        terminal.send_mouse_pos(x, y, mods);
                        terminal.send_mouse_button(false, MouseButton::Left, mods);
                    }
                    let changed = this.drain_surface_state(true, cx);
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                gpui::MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _window, _cx| {
                    this.last_mouse_position = Some(event.position);
                    if let Some(ref terminal) = this.terminal {
                        let (x, y) = this.view_local_pos(event.position);
                        let mods = gpui_mods_to_ghostty(&event.modifiers);
                        terminal.send_mouse_pos(x, y, mods);
                        terminal.send_mouse_button(false, MouseButton::Right, mods);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, _cx| {
                this.last_mouse_position = Some(event.position);
                if let Some(ref terminal) = this.terminal {
                    let (x, y) = this.view_local_pos(event.position);
                    terminal.send_mouse_pos(x, y, gpui_mods_to_ghostty(&event.modifiers));
                }
            }))
            .on_modifiers_changed(cx.listener(
                |this, event: &ModifiersChangedEvent, _window, _cx| {
                    this.refresh_mouse_modifiers(&event.modifiers);
                },
            ))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, _cx| {
                this.last_mouse_position = Some(event.position);
                if let Some(ref terminal) = this.terminal {
                    let delta = match event.delta {
                        ScrollDelta::Lines(d) => (f64::from(d.x), f64::from(d.y)),
                        ScrollDelta::Pixels(d) => {
                            // Match Ghostty's AppKit host: precise trackpad
                            // deltas are sent as-is with a subjective 2x
                            // multiplier and the ScrollMods precision bit.
                            // Ghostty core then accumulates sub-row remainders.
                            (f64::from(d.x) * 2.0, f64::from(d.y) * 2.0)
                        }
                    };
                    terminal.send_mouse_scroll(
                        delta.0,
                        delta.1,
                        gpui_scroll_mods_to_ghostty(&event.delta),
                    );
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                let special_key_writes = gpui_key_to_keycode(event.keystroke.key.as_str())
                    .is_some()
                    || event
                        .keystroke
                        .key_char
                        .as_deref()
                        .is_some_and(|key_char| !key_char.is_empty());
                if key_down_may_write_terminal(event, special_key_writes) {
                    this.restored_screen_text = None;
                }
                if this.handle_key_down(event, window, cx) {
                    window.prevent_default();
                    cx.stop_propagation();
                }
                // Force repaint — some ghostty key bindings (e.g. cmd-k clear screen)
                // modify the terminal without emitting GHOSTTY_ACTION_RENDER.
                cx.notify();
            }))
            .child(
                canvas(
                    {
                        let entity = entity.clone();
                        move |bounds, window, cx| {
                            let _ = entity.update(cx, |view: &mut GhosttyView, _cx| {
                                view.on_layout(bounds, window, _cx);
                            });
                        }
                    },
                    {
                        let focus = input_focus.clone();
                        let entity = entity.clone();
                        move |_bounds, _state, window, cx| {
                            window.handle_input(
                                &focus,
                                GhosttyInputHandler {
                                    view: entity.clone(),
                                },
                                cx,
                            );
                        }
                    },
                )
                .size_full(),
            )
            .children(preedit_overlay)
            .context_menu(move |menu, window, cx| {
                // An empty PopupMenu renders nothing (ContextMenu checks
                // `is_empty`), so suppress con's menu when the terminal app
                // consumed the right-click via mouse reporting.
                if right_click_consumed.get() {
                    return menu;
                }
                crate::terminal_context_menu::terminal_context_menu(
                    menu.action_context(menu_focus.clone()),
                    window,
                    cx,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::should_send_ime_insert_as_key_event;

    #[test]
    fn direct_ascii_ime_commits_use_key_event_path() {
        assert!(should_send_ime_insert_as_key_event("abc"));
        assert!(should_send_ime_insert_as_key_event("A quick test."));
        assert!(!should_send_ime_insert_as_key_event(""));
        assert!(!should_send_ime_insert_as_key_event("hello\n"));
        assert!(!should_send_ime_insert_as_key_event("你好"));
    }
}
