//! Non-macOS placeholder for `ghostty_view`.
//!
//! Mirrors the public surface of `crate::ghostty_view` exactly — same
//! types, same event names, same method signatures — so callers in
//! `terminal_pane.rs` and `workspace/` compile unchanged. The view
//! paints a centered "Terminal backend under construction" card that
//! links to the porting plan, and all terminal operations are no-ops.
//!
//! This file is selected automatically on non-macOS targets that do not
//! yet have a real terminal backend. When the real Linux backend lands,
//! it will provide a `LinuxTerminalView` and the module swap will point
//! at that instead — the rest of the UI stays the same.

use std::sync::Arc;

use con_ghostty::{GhosttyApp, GhosttySplitDirection, GhosttyTerminal};
use gpui::*;
use gpui_component::ActiveTheme;

actions!(ghostty, [ConsumeTab, ConsumeTabPrev]);

#[allow(dead_code)]
pub struct GhosttyTitleChanged(pub Option<String>);
pub struct GhosttyProcessExited;
pub struct GhosttyFocusChanged;
pub struct GhosttySplitRequested(pub GhosttySplitDirection);
pub struct GhosttyCwdChanged(pub Option<String>);

impl EventEmitter<GhosttyTitleChanged> for GhosttyView {}
impl EventEmitter<GhosttyProcessExited> for GhosttyView {}
impl EventEmitter<GhosttyFocusChanged> for GhosttyView {}
impl EventEmitter<GhosttySplitRequested> for GhosttyView {}
impl EventEmitter<GhosttyCwdChanged> for GhosttyView {}

/// Placeholder terminal view. Holds the shared `GhosttyApp` handle so the
/// field shape matches the macOS view, but never creates a real surface.
pub struct GhosttyView {
    #[allow(dead_code)]
    app: Arc<GhosttyApp>,
    terminal: Option<Arc<GhosttyTerminal>>,
    focus_handle: FocusHandle,
    initial_cwd: Option<String>,
    #[allow(dead_code)]
    initial_font_size: f32,
}

/// Register stub key bindings. On macOS the real view binds Tab/Shift-Tab
/// so the terminal captures focus-cycling keys; on the stub there is no
/// terminal to forward them to, but we keep the action names so
/// `workspace/` can reference them without cfg branches.
pub fn init(_cx: &mut App) {}

impl GhosttyView {
    pub fn new(
        app: Arc<GhosttyApp>,
        cwd: Option<String>,
        _restored_screen_text: Option<Vec<String>>,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            app,
            terminal: None,
            focus_handle: cx.focus_handle(),
            initial_cwd: cwd,
            initial_font_size: font_size,
        }
    }

    pub fn terminal(&self) -> Option<&Arc<GhosttyTerminal>> {
        self.terminal.as_ref()
    }

    pub fn write_or_queue(&mut self, _data: &[u8]) {}

    pub fn title(&self) -> Option<String> {
        None
    }

    pub fn current_dir(&self) -> Option<String> {
        self.initial_cwd.clone()
    }

    pub fn is_alive(&self) -> bool {
        false
    }

    pub fn surface_ready(&self) -> bool {
        false
    }

    #[allow(dead_code)]
    pub fn selection_text(&self) -> Option<String> {
        None
    }

    pub fn shutdown_surface(&mut self, _window: Option<&mut Window>, _cx: &mut App) {}

    pub fn set_surface_focus_state(&mut self, _focused: bool) {}

    pub fn ensure_initialized_for_control(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    pub fn set_visible(&self, _visible: bool) {}

    pub fn sync_window_background_blur(&self) {}

    pub fn drain_surface_state(
        &mut self,
        _sync_native_scroll: bool,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    pub fn pump_deferred_work(&mut self, _cx: &mut Context<Self>) -> bool {
        false
    }
}

impl Focusable for GhosttyView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GhosttyView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .size_full()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .bg(theme.background.opacity(0.9))
            .text_color(theme.foreground.opacity(0.8))
            .child(div().text_lg().child("Terminal backend under construction"))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.foreground.opacity(0.5))
                    .child("Linux port — see docs/impl/linux-port.md"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.foreground.opacity(0.4))
                    .child(
                        "Agent panel, settings, command palette, and the control \
                         socket all work. The terminal surface will render here \
                         when the Linux backend lands.",
                    ),
            )
    }
}
