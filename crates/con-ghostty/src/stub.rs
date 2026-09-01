//! Non-macOS stub backend for con-ghostty.
//!
//! libghostty's full embedded C API is macOS-only (April 2026). This module
//! exposes the same public type names the rest of the workspace references
//! (`GhosttyApp`, `GhosttyTerminal`, `TerminalColors`, `MouseButton`,
//! `GhosttySurfaceEvent`, `GhosttySplitDirection`, `CommandFinishedSignal`,
//! `CommandRecord`) so cross-platform code in `con-app` compiles without
//! per-call-site `cfg` gates.
//!
//! Every method here is a placeholder that returns an empty/None/false
//! result. A real Windows/Linux backend will replace this module with
//! working implementations. See `docs/impl/windows-port.md` and
//! `docs/impl/linux-port.md` for the staged plans.
//!
//! Keeping the stubs in `con-ghostty` (rather than per-platform crates)
//! preserves a single "terminal backend" contract that the UI layer
//! consumes; future impls land as siblings of `terminal.rs` under a
//! `windows/` or `linux/` submodule and are selected via `cfg`.

use std::time::Duration;

/// Palette colors passed to the backend when (re)configuring a surface.
#[derive(Debug, Clone)]
pub struct TerminalColors {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub palette: [[u8; 3]; 16],
}

/// Incremental config patch. Empty on the stub path; kept for API parity.
#[derive(Debug, Clone, Default)]
pub struct GhosttyConfigPatch {
    pub colors: Option<TerminalColors>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub background_opacity: Option<f32>,
    pub background_opacity_cells: Option<bool>,
    pub background_blur: Option<bool>,
    pub cursor_style: Option<String>,
    pub background_image: Option<String>,
    pub background_image_opacity: Option<f32>,
    pub background_image_position: Option<String>,
    pub background_image_fit: Option<String>,
    pub background_image_repeat: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttySplitDirection {
    Right,
    Down,
    Left,
    Up,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhosttySurfaceEvent {
    SplitRequest(GhosttySplitDirection),
    OpenUrl(String),
    PwdChanged(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Unknown,
    Left,
    Right,
    Middle,
    Four,
    Five,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceSize {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhosttyScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

pub struct CommandFinishedSignal {
    pub exit_code: Option<i32>,
    pub duration: Duration,
}

#[derive(Debug, Clone)]
pub struct CommandRecord {
    pub exit_code: Option<i32>,
    pub duration: Duration,
}

/// Opaque terminal "app" handle. One per window on macOS; on the stub
/// path it carries no state.
pub struct GhosttyApp {
    _sealed: (),
}

impl GhosttyApp {
    /// Matches the macOS signature so `workspace.rs` can call it without
    /// `cfg` gating. All arguments are accepted and ignored.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        _colors: Option<&TerminalColors>,
        _font_family: Option<&str>,
        _font_size: Option<f32>,
        _background_opacity: Option<f32>,
        _background_blur: Option<bool>,
        _cursor_style: Option<&str>,
        _background_image: Option<&str>,
        _background_image_opacity: Option<f32>,
        _background_image_position: Option<&str>,
        _background_image_fit: Option<&str>,
        _background_image_repeat: Option<bool>,
    ) -> Result<Self, String> {
        Ok(Self { _sealed: () })
    }

    pub fn tick(&self) {}

    /// Cross-platform parity with the macOS and Windows backends.
    /// The stub backend never receives native wakeups, so it remains at 0.
    pub fn wake_generation(&self) -> u64 {
        0
    }

    pub fn update_colors(&self, _colors: &TerminalColors) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_appearance(
        &self,
        _colors: &TerminalColors,
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
        Ok(())
    }

    pub fn update_config(&self, _patch: &GhosttyConfigPatch) -> Result<(), String> {
        Ok(())
    }

    pub fn set_color_scheme(&self, _dark: bool) {}
}

// SAFETY: the stub has no interior state, so it is trivially Send+Sync.
unsafe impl Send for GhosttyApp {}
unsafe impl Sync for GhosttyApp {}

/// A single terminal surface. All methods are no-ops that return empty
/// results so UI code relying on `Option<&Arc<GhosttyTerminal>>` degrades
/// gracefully (no commands, no history, no output).
pub struct GhosttyTerminal {
    _sealed: (),
}

impl GhosttyTerminal {
    pub const SUPPORTS_FOREGROUND_PROCESS_GROUP_ID: bool = false;
    pub const SUPPORTS_TTY_NAME: bool = false;

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
    pub fn set_content_scale(&self, _scale: f64) {}
    pub fn set_focus(&self, _focused: bool) {}
    pub fn set_occlusion(&self, _occluded: bool) {}
    pub fn set_color_scheme(&self, _dark: bool) {}
    pub fn perform_binding_action(&self, _action: &str) -> Result<bool, String> {
        Ok(false)
    }
    pub fn clear_screen_and_scrollback(&self) -> Result<(), String> {
        Ok(())
    }
    pub fn request_split(&self, _direction: GhosttySplitDirection) {}
    pub fn update_config(&self, _patch: &GhosttyConfigPatch) -> Result<(), String> {
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_appearance(
        &self,
        _colors: &TerminalColors,
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
        Ok(())
    }

    pub fn write_to_pty(&self, _data: &[u8]) {}
    pub fn send_text(&self, _text: &str) {}
    pub fn send_mouse_button(&self, _pressed: bool, _button: MouseButton, _mods: i32) -> bool {
        false
    }
    pub fn send_mouse_pos(&self, _x: f64, _y: f64, _mods: i32) {}
    pub fn send_mouse_scroll(&self, _x: f64, _y: f64, _mods: i32) {}
    pub fn request_close(&self) {}

    pub fn title(&self) -> Option<String> {
        None
    }
    pub fn current_dir(&self) -> Option<String> {
        None
    }
    pub fn foreground_process_group_id(&self) -> Option<u64> {
        None
    }
    pub fn tty_name(&self) -> Option<String> {
        None
    }
    pub fn is_alive(&self) -> bool {
        false
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
        false
    }
    pub fn selection_text(&self) -> Option<String> {
        None
    }
    pub fn read_screen_text(&self, _max_lines: usize) -> Vec<String> {
        Vec::new()
    }
    pub fn read_recent_lines(&self, _max_lines: usize) -> Vec<String> {
        Vec::new()
    }
    pub fn search_text(&self, _pattern: &str, _limit: usize) -> Vec<(usize, String)> {
        Vec::new()
    }
    pub fn take_needs_render(&self) -> bool {
        false
    }
    pub fn take_pending_events(&self) -> Vec<GhosttySurfaceEvent> {
        Vec::new()
    }
}

// SAFETY: no interior state — trivially thread-safe.
unsafe impl Send for GhosttyTerminal {}
unsafe impl Sync for GhosttyTerminal {}
