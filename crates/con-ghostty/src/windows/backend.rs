//! Public facade: `WindowsGhosttyApp` / `WindowsGhosttyTerminal` that
//! the rest of the workspace consumes through the same `GhosttyApp` /
//! `GhosttyTerminal` type names that the macOS path uses (re-exported
//! from `crate::lib`). This keeps callers in `con-app` shape-identical
//! across platforms.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use super::host_view::RenderSession;
use super::render::{RendererConfig, ThemeColors};
use crate::stub::{
    CommandFinishedSignal, CommandRecord, GhosttyConfigPatch, GhosttyScrollbar,
    GhosttySplitDirection, GhosttySurfaceEvent, MouseButton, SurfaceSize, TerminalColors,
};
use crate::vt::{VtKeyEvent, VtKeyOutcome};

fn theme_from_colors(colors: &TerminalColors) -> ThemeColors {
    ThemeColors::from_ansi16(colors.foreground, colors.background, colors.palette)
}

fn clamp_opacity(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

/// One per GPUI window. Holds shared, app-wide terminal config.
pub struct WindowsGhosttyApp {
    config: Mutex<RendererConfig>,
    clipboard_write_enabled: AtomicBool,
}

impl WindowsGhosttyApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        colors: Option<&TerminalColors>,
        font_family: Option<&str>,
        font_size: Option<f32>,
        background_opacity: Option<f32>,
        _background_blur: Option<bool>,
        _cursor_style: Option<&str>,
        _background_image: Option<&str>,
        _background_image_opacity: Option<f32>,
        _background_image_position: Option<&str>,
        _background_image_fit: Option<&str>,
        _background_image_repeat: Option<bool>,
        clipboard_write_enabled: bool,
    ) -> Result<Self, String> {
        let mut config = RendererConfig::default();
        if let Some(family) = font_family {
            config.font_family = family.to_string();
        }
        if let Some(size) = font_size {
            config.font_size_px = size;
        }
        if let Some(colors) = colors {
            let theme = theme_from_colors(colors);
            config.clear_color = [
                colors.background[0] as f32 / 255.0,
                colors.background[1] as f32 / 255.0,
                colors.background[2] as f32 / 255.0,
                1.0,
            ];
            config.theme = Some(theme);
        }
        if let Some(op) = background_opacity {
            config.background_opacity = clamp_opacity(op);
        }
        Ok(Self {
            config: Mutex::new(config),
            clipboard_write_enabled: AtomicBool::new(clipboard_write_enabled),
        })
    }

    pub fn tick(&self) {}

    /// Stub for parity with the macOS `GhosttyApp::wake_generation`.
    pub fn wake_generation(&self) -> u64 {
        0
    }

    pub fn update_colors(&self, _colors: &TerminalColors) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_appearance(
        &self,
        colors: &TerminalColors,
        font_family: &str,
        font_size: f32,
        background_opacity: f32,
        _background_blur: bool,
        _cursor_style: &str,
        _background_image: Option<&str>,
        _background_image_opacity: f32,
        _background_image_position: Option<&str>,
        _background_image_fit: Option<&str>,
        _background_image_repeat: bool,
    ) -> Result<(), String> {
        let theme = theme_from_colors(colors);
        let mut config = self.config.lock();
        config.font_family = font_family.to_string();
        config.font_size_px = font_size;
        config.clear_color = [
            colors.background[0] as f32 / 255.0,
            colors.background[1] as f32 / 255.0,
            colors.background[2] as f32 / 255.0,
            1.0,
        ];
        config.theme = Some(theme);
        config.background_opacity = clamp_opacity(background_opacity);
        Ok(())
    }

    pub fn update_config(&self, _patch: &GhosttyConfigPatch) -> Result<(), String> {
        Ok(())
    }

    pub fn set_color_scheme(&self, _dark: bool) {}

    pub fn set_clipboard_write_enabled(&self, enabled: bool) -> Result<(), String> {
        self.clipboard_write_enabled
            .store(enabled, Ordering::Release);
        Ok(())
    }

    pub fn clipboard_write_enabled(&self) -> bool {
        self.clipboard_write_enabled.load(Ordering::Acquire)
    }

    /// Snapshot the current renderer config — used by `WindowsTerminalView`
    /// when constructing a new `RenderSession`.
    pub fn renderer_config(&self) -> RendererConfig {
        self.config.lock().clone()
    }
}

unsafe impl Send for WindowsGhosttyApp {}
unsafe impl Sync for WindowsGhosttyApp {}

/// One per pane. The `RenderSession` (renderer + VT + ConPTY) is
/// constructed lazily by the GPUI view once it has a size and DPI.
pub struct WindowsGhosttyTerminal {
    inner: Arc<Mutex<Option<RenderSession>>>,
}

impl WindowsGhosttyTerminal {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach a live `RenderSession`. Idempotent: replaces any existing one.
    pub fn attach(&self, session: RenderSession) {
        *self.inner.lock() = Some(session);
    }

    pub fn is_attached(&self) -> bool {
        self.inner.lock().is_some()
    }

    /// Accessor for the GPUI view to reach into the session for render
    /// / input methods. Guarded by the same Mutex that protects attach.
    pub fn inner(&self) -> Arc<Mutex<Option<RenderSession>>> {
        self.inner.clone()
    }

    pub fn draw(&self) {}
    pub fn refresh(&self) {}
    pub fn set_size(&self, _w: u32, _h: u32) {}

    pub fn size(&self) -> SurfaceSize {
        SurfaceSize {
            columns: 0,
            rows: 0,
            width_px: 0,
            height_px: 0,
            cell_width_px: 0,
            cell_height_px: 0,
        }
    }

    pub fn scrollbar(&self) -> Option<GhosttyScrollbar> {
        self.inner.lock().as_ref().and_then(|s| s.scrollbar())
    }

    pub fn set_content_scale(&self, _scale: f64) {}
    pub fn set_focus(&self, _focused: bool) {}
    pub fn set_occlusion(&self, _occluded: bool) {}
    pub fn set_color_scheme(&self, _dark: bool) {}
    pub fn perform_binding_action(&self, _action: &str) -> Result<bool, String> {
        Ok(false)
    }
    pub fn clear_screen_and_scrollback(&self) -> Result<(), String> {
        if let Some(session) = self.inner.lock().as_ref() {
            session.clear_screen_and_scrollback();
        }
        Ok(())
    }
    pub fn request_split(&self, _direction: GhosttySplitDirection) {}
    pub fn update_config(&self, _patch: &GhosttyConfigPatch) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_appearance(
        &self,
        colors: &TerminalColors,
        _font_family: &str,
        _font_size: f32,
        background_opacity: f32,
        _background_blur: bool,
        _cursor_style: &str,
        _background_image: Option<&str>,
        _background_image_opacity: f32,
        _background_image_position: Option<&str>,
        _background_image_fit: Option<&str>,
        _background_image_repeat: bool,
    ) -> Result<(), String> {
        if let Some(session) = self.inner.lock().as_ref() {
            let theme = theme_from_colors(colors);
            session.set_appearance(Some(&theme), clamp_opacity(background_opacity));
        }
        Ok(())
    }

    pub fn write_to_pty(&self, data: &[u8]) {
        if let Some(session) = self.inner.lock().as_ref() {
            if let Ok(s) = std::str::from_utf8(data)
                && let Err(err) = session.write_input(s)
            {
                log::debug!("windows terminal input write failed: {err:#}");
            }
        }
    }

    pub fn send_text(&self, text: &str) {
        if let Some(session) = self.inner.lock().as_ref()
            && let Err(err) = session.write_input(text)
        {
            log::debug!("windows terminal text write failed: {err:#}");
        }
    }

    pub fn send_key(&self, event: &VtKeyEvent<'_>) -> Result<VtKeyOutcome, String> {
        let guard = self.inner.lock();
        let Some(session) = guard.as_ref() else {
            return Ok(VtKeyOutcome::default());
        };
        session.send_key(event).map_err(|err| err.to_string())
    }

    pub fn paste_text(
        &self,
        text: &str,
        source: crate::vt::VtPasteSource,
        confirm_unsafe_paste: bool,
    ) -> Result<crate::vt::VtPasteResult, String> {
        let guard = self.inner.lock();
        let Some(session) = guard.as_ref() else {
            return Ok(crate::vt::VtPasteResult::Empty);
        };
        session
            .paste_text(text, source, confirm_unsafe_paste)
            .map_err(|err| err.to_string())
    }

    pub fn send_mouse_button(&self, _pressed: bool, _button: MouseButton, _mods: i32) -> bool {
        false
    }
    pub fn send_mouse_pos(&self, _x: f64, _y: f64, _mods: i32) {}
    pub fn send_mouse_scroll(&self, _x: f64, _y: f64, _mods: i32) {}
    pub fn request_close(&self) {
        *self.inner.lock() = None;
    }

    pub fn title(&self) -> Option<String> {
        self.inner.lock().as_ref().and_then(RenderSession::title)
    }
    pub fn take_bell(&self) -> bool {
        self.inner
            .lock()
            .as_ref()
            .is_some_and(RenderSession::take_bell)
    }
    pub fn progress(&self) -> Option<crate::TerminalProgress> {
        self.inner.lock().as_ref().and_then(RenderSession::progress)
    }
    pub fn set_clipboard_write_enabled(&self, enabled: bool) -> Result<(), String> {
        if let Some(session) = self.inner.lock().as_ref() {
            session.set_clipboard_write_enabled(enabled)?;
        }
        Ok(())
    }
    pub fn take_clipboard_write(&self) -> Option<String> {
        self.inner
            .lock()
            .as_ref()
            .and_then(RenderSession::take_clipboard_write)
    }
    pub fn current_dir(&self) -> Option<String> {
        self.inner
            .lock()
            .as_ref()
            .and_then(RenderSession::current_dir)
    }
    pub fn is_alive(&self) -> bool {
        match self.inner.lock().as_ref() {
            Some(session) => session.is_alive(),
            None => true,
        }
    }
    pub fn is_busy(&self) -> bool {
        false
    }
    pub fn command_history(&self) -> Vec<CommandRecord> {
        Vec::new()
    }
    pub fn take_command_finished(&self) -> Option<CommandFinishedSignal> {
        None
    }
    pub fn last_exit_code(&self) -> Option<i32> {
        None
    }
    pub fn last_command_duration(&self) -> Option<Duration> {
        None
    }
    pub fn input_generation(&self) -> u64 {
        0
    }
    pub fn last_command_finished_input_generation(&self) -> u64 {
        0
    }
    pub fn recover_shell_prompt_state(&self) {}
    pub fn has_selection(&self) -> bool {
        self.inner
            .lock()
            .as_ref()
            .is_some_and(|s| s.has_selection())
    }
    pub fn selection_text(&self) -> Option<String> {
        self.inner.lock().as_ref().and_then(|s| s.selection_text())
    }
    pub fn clear_selection(&self) {
        if let Some(session) = self.inner.lock().as_ref() {
            session.clear_selection();
        }
    }
    pub fn read_screen_text(&self, max_lines: usize) -> Vec<String> {
        self.inner
            .lock()
            .as_ref()
            .map(|session| session.read_screen_text(max_lines))
            .unwrap_or_default()
    }
    pub fn read_recent_lines(&self, max_lines: usize) -> Vec<String> {
        self.inner
            .lock()
            .as_ref()
            .map(|session| session.read_recent_lines(max_lines))
            .unwrap_or_default()
    }
    pub fn search_text(&self, pattern: &str, limit: usize) -> Vec<(usize, String)> {
        self.inner
            .lock()
            .as_ref()
            .map(|session| session.search_text(pattern, limit))
            .unwrap_or_default()
    }
    pub fn take_needs_render(&self) -> bool {
        false
    }
    pub fn take_pending_events(&self) -> Vec<GhosttySurfaceEvent> {
        Vec::new()
    }
}

impl Default for WindowsGhosttyTerminal {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for WindowsGhosttyTerminal {}
unsafe impl Sync for WindowsGhosttyTerminal {}
