use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

use super::pty::{LinuxPtyOptions, LinuxPtySession, LinuxPtySpawnError, LinuxWakeCallback};
use crate::stub::{
    CommandFinishedSignal, CommandRecord, GhosttyConfigPatch, GhosttySplitDirection,
    GhosttySurfaceEvent, MouseButton, SurfaceSize, TerminalColors,
};
use crate::vt::{ScreenSnapshot, VtKeyEvent, VtKeyOutcome};
use crate::{
    ClipboardWritePolicy, DesktopNotificationPolicy, clipboard_write_policy,
    desktop_notification_policy,
};

#[derive(Debug, Clone)]
pub struct LinuxBackendConfig {
    pub shell_program: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub colors: Option<TerminalColors>,
    /// 0.0 (fully see-through) … 1.0 (opaque). Multiplied into the
    /// terminal pane's solid background fill so the GPUI window
    /// composites over the desktop / Wayland blur surface beneath.
    /// Mirrors the Windows backend's `RendererConfig.background_opacity`
    /// and the macOS pass-through to libghostty.
    pub background_opacity: f32,
    /// Whether the user opted into the Wayland `org_kde_kwin_blur`
    /// surface region (only honored on KDE Plasma). Stored so
    /// `LinuxGhosttyApp::backend_config()` consumers see the
    /// authoritative state. Has no effect on the per-cell paint
    /// itself — the `WindowBackgroundAppearance::Blurred` toggle is
    /// applied at the GPUI window level in `con-app/main.rs`.
    pub background_blur: bool,
    pub clipboard_write: bool,
}

impl Default for LinuxBackendConfig {
    fn default() -> Self {
        Self {
            shell_program: None,
            font_family: None,
            font_size: None,
            colors: None,
            background_opacity: 1.0,
            background_blur: false,
            clipboard_write: false,
        }
    }
}

/// One per GPUI window. Holds Linux backend configuration that future
/// PTY and renderer setup can consume.
pub struct LinuxGhosttyApp {
    config: Mutex<LinuxBackendConfig>,
    wake_generation: Arc<AtomicU64>,
    clipboard_write_policy: Arc<ClipboardWritePolicy>,
    desktop_notification_policy: Arc<DesktopNotificationPolicy>,
}

impl LinuxGhosttyApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        colors: Option<&TerminalColors>,
        font_family: Option<&str>,
        font_size: Option<f32>,
        background_opacity: Option<f32>,
        background_blur: Option<bool>,
        _cursor_style: Option<&str>,
        _background_image: Option<&str>,
        _background_image_opacity: Option<f32>,
        _background_image_position: Option<&str>,
        _background_image_fit: Option<&str>,
        _background_image_repeat: Option<bool>,
        clipboard_write: bool,
    ) -> Result<Self, String> {
        Ok(Self {
            config: Mutex::new(LinuxBackendConfig {
                shell_program: default_linux_shell_program(),
                font_family: font_family.map(ToOwned::to_owned),
                font_size,
                colors: colors.cloned(),
                background_opacity: clamp_opacity(background_opacity.unwrap_or(1.0)),
                background_blur: background_blur.unwrap_or(false),
                clipboard_write,
            }),
            wake_generation: Arc::new(AtomicU64::new(1)),
            clipboard_write_policy: clipboard_write_policy(clipboard_write),
            desktop_notification_policy: desktop_notification_policy(),
        })
    }

    pub fn tick(&self) {}

    pub fn wake_generation(&self) -> u64 {
        self.wake_generation.load(Ordering::Acquire)
    }

    pub fn update_colors(&self, colors: &TerminalColors) -> Result<(), String> {
        let mut config = self.config.lock();
        config.colors = Some(colors.clone());
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_appearance(
        &self,
        colors: &TerminalColors,
        font_family: &str,
        font_size: f32,
        background_opacity: f32,
        background_blur: bool,
        _cursor_style: &str,
        _background_image: Option<&str>,
        _background_image_opacity: f32,
        _background_image_position: Option<&str>,
        _background_image_fit: Option<&str>,
        _background_image_repeat: bool,
    ) -> Result<(), String> {
        let mut config = self.config.lock();
        config.font_family = Some(font_family.to_string());
        config.font_size = Some(font_size);
        config.colors = Some(colors.clone());
        config.background_opacity = clamp_opacity(background_opacity);
        config.background_blur = background_blur;
        Ok(())
    }

    /// Current background opacity (0.0..=1.0). The view multiplies
    /// this into its terminal-pane fill so per-pane translucency
    /// composites against the GPUI window's transparent or blurred
    /// background.
    pub fn background_opacity(&self) -> f32 {
        self.config.lock().background_opacity
    }

    pub fn update_config(&self, _patch: &GhosttyConfigPatch) -> Result<(), String> {
        Ok(())
    }

    pub fn set_color_scheme(&self, _dark: bool) {}

    pub fn set_clipboard_write_enabled(&self, enabled: bool) -> Result<(), String> {
        if !enabled {
            self.clipboard_write_policy.set_enabled(false);
        }
        self.config.lock().clipboard_write = enabled;
        if enabled {
            self.clipboard_write_policy.set_enabled(true);
        }
        Ok(())
    }

    pub fn backend_config(&self) -> LinuxBackendConfig {
        self.config.lock().clone()
    }

    pub fn default_pty_options(&self, cwd: Option<&std::path::Path>) -> LinuxPtyOptions {
        let config = self.backend_config();
        LinuxPtyOptions {
            cwd: cwd.map(PathBuf::from),
            program: config.shell_program,
            wake_generation: Some(self.wake_generation.clone()),
            theme: config.colors,
            clipboard_write: config.clipboard_write,
            clipboard_write_policy: self.clipboard_write_policy.clone(),
            desktop_notification_policy: self.desktop_notification_policy.clone(),
            ..LinuxPtyOptions::default()
        }
    }
}

unsafe impl Send for LinuxGhosttyApp {}
unsafe impl Sync for LinuxGhosttyApp {}

fn clamp_opacity(value: f32) -> f32 {
    // `f32::clamp` propagates NaN, which would then leak into the
    // pane fill alpha and downstream color math (every Hsla
    // multiplication also propagates NaN, so a malformed setting
    // could make the entire pane go black). Settings already
    // round-trip through serde validation in practice, but we get
    // the safety belt for free.
    if !value.is_finite() {
        return 1.0;
    }
    value.clamp(0.0, 1.0)
}

fn default_linux_shell_program() -> Option<String> {
    if let Some(shell) = std::env::var("CON_LINUX_SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Some(shell);
    }

    if let Some(shell) = std::env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return Some(shell);
    }

    for candidate in ["/bin/bash", "/usr/bin/bash", "/bin/sh", "/usr/bin/sh"] {
        if PathBuf::from(candidate).exists() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// One per pane. The GPUI view attaches the PTY + VT session lazily
/// once the pane has real bounds.
pub struct LinuxGhosttyTerminal {
    inner: Arc<Mutex<Option<LinuxPtySession>>>,
    wake_callback: Arc<Mutex<Option<LinuxWakeCallback>>>,
}

impl LinuxGhosttyTerminal {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            wake_callback: Arc::new(Mutex::new(None)),
        }
    }

    pub fn attach(&self, session: LinuxPtySession) {
        *self.inner.lock() = Some(session);
    }

    pub fn is_attached(&self) -> bool {
        self.inner.lock().is_some()
    }

    pub fn spawn_with_options(&self, options: LinuxPtyOptions) -> Result<(), LinuxPtySpawnError> {
        let mut options = options;
        if options.wake_callback.is_none() {
            options.wake_callback = self.wake_callback.lock().clone();
        }
        let session = LinuxPtySession::spawn(options)?;
        self.attach(session);
        Ok(())
    }

    /// Set the callback used by subsequent `spawn_with_options` calls.
    /// Existing PTY sessions keep the callback captured at spawn time.
    pub fn set_wake_callback(&self, callback: Option<LinuxWakeCallback>) {
        *self.wake_callback.lock() = callback;
    }

    pub fn inner(&self) -> Arc<Mutex<Option<LinuxPtySession>>> {
        self.inner.clone()
    }

    pub fn draw(&self) {}

    pub fn refresh(&self) {}

    pub fn set_size(&self, width_px: u32, height_px: u32) {
        if let Some(session) = self.inner.lock().as_ref() {
            if let Err(err) = session.set_pixel_size(width_px, height_px) {
                log::debug!("linux pty pixel resize failed: {err:#}");
            }
        }
    }

    pub fn resize_surface(&self, size: SurfaceSize) -> Result<(), String> {
        let guard = self.inner.lock();
        let Some(session) = guard.as_ref() else {
            return Err("linux terminal is not attached".to_string());
        };
        session.resize(size).map_err(|err| format!("{err:#}"))
    }

    pub fn size(&self) -> SurfaceSize {
        self.inner
            .lock()
            .as_ref()
            .map(LinuxPtySession::size)
            .unwrap_or(SurfaceSize {
                columns: 0,
                rows: 0,
                width_px: 0,
                height_px: 0,
                cell_width_px: 0,
                cell_height_px: 0,
            })
    }

    pub fn set_content_scale(&self, _scale: f64) {}
    pub fn set_focus(&self, _focused: bool) {}
    pub fn set_occlusion(&self, _occluded: bool) {}
    pub fn set_color_scheme(&self, dark: bool) {
        if let Some(session) = self.inner.lock().as_ref() {
            session.set_dark_mode(dark);
        }
    }

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
        _background_opacity: f32,
        _background_blur: bool,
        _cursor_style: &str,
        _background_image: Option<&str>,
        _background_image_opacity: f32,
        _background_image_position: Option<&str>,
        _background_image_fit: Option<&str>,
        _background_image_repeat: bool,
    ) -> Result<(), String> {
        if let Some(session) = self.inner.lock().as_ref() {
            session.set_theme(colors);
        }
        Ok(())
    }

    /// Returns the current libghostty-vt screen snapshot, if a PTY
    /// session has been spawned. Used by `con-app/src/linux_view.rs`
    /// to drive the styled-cell paint path.
    pub fn snapshot(&self) -> Option<ScreenSnapshot> {
        self.inner.lock().as_ref().map(LinuxPtySession::snapshot)
    }

    pub fn write_to_pty(&self, data: &[u8]) {
        if let Some(session) = self.inner.lock().as_ref() {
            if let Err(err) = session.write_input(data) {
                log::debug!("linux pty write failed: {err:#}");
            }
        }
    }

    pub fn send_text(&self, text: &str) {
        self.write_to_pty(text.as_bytes());
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

    pub fn is_decckm(&self) -> bool {
        self.inner
            .lock()
            .as_ref()
            .is_some_and(LinuxPtySession::is_decckm)
    }

    pub fn send_mouse_button(&self, _pressed: bool, _button: MouseButton, _mods: i32) -> bool {
        false
    }

    pub fn send_mouse_pos(&self, _x: f64, _y: f64, _mods: i32) {}
    pub fn send_mouse_scroll(&self, _x: f64, _y: f64, _mods: i32) {}

    /// True when the child app has enabled terminal mouse reporting
    /// (DECSET 1000/1002/1003). The Linux view gates its SGR reports on
    /// this so clicks don't leak escape sequences into plain shells.
    pub fn mouse_tracking_active(&self) -> bool {
        self.inner
            .lock()
            .as_ref()
            .is_some_and(LinuxPtySession::mouse_tracking_active)
    }

    /// Report a mouse button press/release to the child as an SGR
    /// (1006) escape sequence, but only when the child has enabled SGR
    /// mouse reporting. `button` is the SGR button index (0=Left,
    /// 1=Middle, 2=Right). Shift bypasses reporting so the user can
    /// always select text with Shift+click. Returns `true` when the
    /// report was emitted (i.e. the click was consumed by the app).
    pub fn mouse_report(&self, button: u8, col: u16, row: u16, shift: bool) -> bool {
        let inner = self.inner.lock();
        let Some(session) = inner.as_ref() else {
            return false;
        };
        if !session.mouse_tracking_active() || !session.is_sgr_mouse() || shift {
            return false;
        }
        let seq = sgr_mouse_sequence(button, col, row, true);
        match session.write_input(seq.as_bytes()) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("linux pty mouse report write failed: {err:#}");
                false
            }
        }
    }

    /// Report a mouse button release to the child as an SGR (1006)
    /// sequence, gated on the same mouse-reporting mode as
    /// [`Self::mouse_report`].
    pub fn mouse_release(&self, button: u8, col: u16, row: u16, shift: bool) -> bool {
        let inner = self.inner.lock();
        let Some(session) = inner.as_ref() else {
            return false;
        };
        if !session.mouse_tracking_active() || !session.is_sgr_mouse() || shift {
            return false;
        }
        let seq = sgr_mouse_sequence(button, col, row, false);
        match session.write_control(seq.as_bytes()) {
            Ok(()) => true,
            Err(err) => {
                log::debug!("linux pty mouse release write failed: {err:#}");
                false
            }
        }
    }

    pub fn request_close(&self) {
        *self.inner.lock() = None;
    }

    pub fn title(&self) -> Option<String> {
        self.inner.lock().as_ref().and_then(LinuxPtySession::title)
    }

    pub fn take_bell(&self) -> bool {
        self.inner
            .lock()
            .as_ref()
            .is_some_and(LinuxPtySession::take_bell)
    }

    pub fn progress(&self) -> Option<crate::TerminalProgress> {
        self.inner
            .lock()
            .as_ref()
            .and_then(LinuxPtySession::progress)
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
            .and_then(LinuxPtySession::take_clipboard_write)
    }

    pub fn take_desktop_notification(&self) -> Option<crate::DesktopNotification> {
        self.inner
            .lock()
            .as_ref()
            .and_then(LinuxPtySession::take_desktop_notification)
    }

    pub fn current_dir(&self) -> Option<String> {
        self.inner
            .lock()
            .as_ref()
            .and_then(LinuxPtySession::current_dir)
    }

    pub fn is_alive(&self) -> bool {
        self.inner
            .lock()
            .as_ref()
            .is_some_and(LinuxPtySession::is_alive)
    }

    pub fn is_busy(&self) -> bool {
        false
    }

    pub fn command_history(&self) -> Vec<CommandRecord> {
        Vec::new()
    }

    pub fn take_command_finished(&self) -> Option<CommandFinishedSignal> {
        self.inner
            .lock()
            .as_ref()
            .and_then(LinuxPtySession::take_command_finished)
    }

    pub fn last_exit_code(&self) -> Option<i32> {
        self.inner
            .lock()
            .as_ref()
            .and_then(LinuxPtySession::last_exit_code)
    }

    pub fn last_command_duration(&self) -> Option<Duration> {
        self.inner
            .lock()
            .as_ref()
            .and_then(LinuxPtySession::last_command_duration)
    }

    pub fn input_generation(&self) -> u64 {
        self.inner
            .lock()
            .as_ref()
            .map(LinuxPtySession::input_generation)
            .unwrap_or(0)
    }

    pub fn last_command_finished_input_generation(&self) -> u64 {
        self.input_generation()
    }

    pub fn recover_shell_prompt_state(&self) {}

    pub fn has_selection(&self) -> bool {
        false
    }

    pub fn selection_text(&self) -> Option<String> {
        None
    }

    pub fn clear_selection(&self) {}

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
        self.inner
            .lock()
            .as_ref()
            .is_some_and(LinuxPtySession::take_needs_render)
    }

    pub fn take_pending_events(&self) -> Vec<GhosttySurfaceEvent> {
        Vec::new()
    }
}

impl Default for LinuxGhosttyTerminal {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for LinuxGhosttyTerminal {}
unsafe impl Sync for LinuxGhosttyTerminal {}

/// Build an SGR (1006) mouse report escape sequence for the given
/// 0-based cell coordinates. `pressed` selects the press (`M`) or
/// release (`m`) terminator.
fn sgr_mouse_sequence(button: u8, col: u16, row: u16, pressed: bool) -> String {
    let col = col.saturating_add(1);
    let row = row.saturating_add(1);
    let terminator = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{button};{col};{row}{terminator}")
}

#[cfg(test)]
mod tests {
    use super::sgr_mouse_sequence;

    #[test]
    fn sgr_right_press_uses_button_2_and_one_based_coords() {
        assert_eq!(sgr_mouse_sequence(2, 0, 0, true), "\x1b[<2;1;1M");
        assert_eq!(sgr_mouse_sequence(2, 5, 9, true), "\x1b[<2;6;10M");
    }

    #[test]
    fn sgr_left_press_and_release_terminators() {
        assert_eq!(sgr_mouse_sequence(0, 3, 7, true), "\x1b[<0;4;8M");
        assert_eq!(sgr_mouse_sequence(0, 3, 7, false), "\x1b[<0;4;8m");
    }

    #[test]
    fn sgr_coordinates_saturate_without_wrapping() {
        assert_eq!(
            sgr_mouse_sequence(0, u16::MAX, u16::MAX, true),
            "\x1b[<0;65535;65535M"
        );
    }
}
