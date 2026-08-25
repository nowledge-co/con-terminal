//! Safe wrapper around ghostty's embedded C API.
//!
//! Uses the macOS platform: creates an NSView for ghostty to render into
//! via its GPU-accelerated Metal renderer. State (title, pwd) is received
//! through action callbacks, not polling.
//!
//! Design invariants:
//! - `ghostty_init` is called exactly once per process (via `std::sync::Once`)
//! - Each `GhosttyTerminal` has its own `TerminalState` via per-surface userdata
//! - Clipboard callbacks are always set (ghostty dereferences them without null checks)
//! - The GhosttyApp must be ticked from the main thread (Metal rendering)

use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::io::Write;
use std::os::raw::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Once};
use std::time::Instant;

use dispatch::Queue;
use parking_lot::Mutex;

use crate::ffi;

const DEFAULT_GHOSTTY_FONT_FAMILY: &str = "Ioskeley Mono";
const CON_SHELL_INTEGRATION_FEATURES: &str = "no-cursor,ssh-env,ssh-terminfo";

fn sanitize_font_family_for_ghostty(font_family: &str) -> &str {
    let trimmed = font_family.trim();
    if trimmed.is_empty() || trimmed.starts_with('.') {
        DEFAULT_GHOSTTY_FONT_FAMILY
    } else {
        trimmed
    }
}

// ── Theme colors for ghostty config ──────────────────────────

/// Terminal colors in a format ghostty understands.
/// Decoupled from con-terminal's TerminalTheme to avoid cross-crate dependency.
#[derive(Debug, Clone)]
pub struct TerminalColors {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub palette: [[u8; 3]; 16],
}

impl TerminalColors {
    fn append_config(&self, s: &mut String) {
        s.push_str(&format!(
            "background = {:02x}{:02x}{:02x}\n",
            self.background[0], self.background[1], self.background[2]
        ));
        s.push_str(&format!(
            "foreground = {:02x}{:02x}{:02x}\n",
            self.foreground[0], self.foreground[1], self.foreground[2]
        ));
        for (i, c) in self.palette.iter().enumerate() {
            s.push_str(&format!(
                "palette = {}={:02x}{:02x}{:02x}\n",
                i, c[0], c[1], c[2]
            ));
        }
    }
}

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

impl GhosttyConfigPatch {
    fn merge(&mut self, patch: &GhosttyConfigPatch) {
        if let Some(colors) = &patch.colors {
            self.colors = Some(colors.clone());
        }
        if let Some(font_family) = &patch.font_family {
            self.font_family = Some(font_family.clone());
        }
        if let Some(font_size) = patch.font_size {
            self.font_size = Some(font_size);
        }
        if let Some(background_opacity) = patch.background_opacity {
            self.background_opacity = Some(background_opacity);
        }
        if let Some(background_opacity_cells) = patch.background_opacity_cells {
            self.background_opacity_cells = Some(background_opacity_cells);
        }
        if let Some(background_blur) = patch.background_blur {
            self.background_blur = Some(background_blur);
        }
        if let Some(cursor_style) = &patch.cursor_style {
            self.cursor_style = Some(cursor_style.clone());
        }
        if let Some(background_image) = &patch.background_image {
            self.background_image = Some(background_image.clone());
        }
        if let Some(background_image_opacity) = patch.background_image_opacity {
            self.background_image_opacity = Some(background_image_opacity);
        }
        if let Some(background_image_position) = &patch.background_image_position {
            self.background_image_position = Some(background_image_position.clone());
        }
        if let Some(background_image_fit) = &patch.background_image_fit {
            self.background_image_fit = Some(background_image_fit.clone());
        }
        if let Some(background_image_repeat) = patch.background_image_repeat {
            self.background_image_repeat = Some(background_image_repeat);
        }
    }

    fn to_config_string(&self) -> String {
        let mut s = String::with_capacity(512);
        if let Some(colors) = &self.colors {
            colors.append_config(&mut s);
        }
        if let Some(font_family) = &self.font_family {
            let font_family = sanitize_font_family_for_ghostty(font_family);
            s.push_str("font-family = \"\"\n");
            s.push_str(&format!("font-family = {:?}\n", font_family));
        }
        if let Some(font_size) = self.font_size {
            s.push_str(&format!("font-size = {:.2}\n", font_size));
        }
        if let Some(background_opacity) = self.background_opacity {
            s.push_str(&format!(
                "background-opacity = {:.2}\n",
                background_opacity.clamp(0.0, 1.0)
            ));
        }
        if let Some(background_opacity_cells) = self.background_opacity_cells {
            s.push_str(&format!(
                "background-opacity-cells = {}\n",
                background_opacity_cells
            ));
        }
        if let Some(background_blur) = self.background_blur {
            s.push_str(&format!("background-blur = {}\n", background_blur));
        }
        if let Some(cursor_style) = &self.cursor_style {
            s.push_str(&format!("cursor-style = {}\n", cursor_style));
            s.push_str("cursor-color = cell-foreground\n");
            s.push_str("cursor-text = cell-background\n");
        }
        // Disable ghostty shell-integration cursor override so con's
        // cursor-style setting is respected. Ghostty's integration
        // unconditionally forces a bar cursor at prompts otherwise.
        // Enable ssh-env and ssh-terminfo so Ghostty's shell integration
        // auto-installs xterm-ghostty terminfo on remote SSH hosts.
        s.push_str("shell-integration-features = ");
        s.push_str(CON_SHELL_INTEGRATION_FEATURES);
        s.push('\n');
        // Force Kitty writes through Ghostty's confirmation request so Con can
        // reject them until it has a user-visible permission flow. OSC 52 uses
        // the write callback's `confirm` flag instead; that callback preserves
        // Con's existing unconditional-write behavior during this migration.
        s.push_str("clipboard-write = ask\n");
        // This limit applies only to Kitty OSC 5522, not OSC 52. Avoid buffering
        // an untrusted 64 MiB transaction just to reject it at confirmation.
        s.push_str("clipboard-write-limit-bytes = 0\n");
        if let Some(background_image) = &self.background_image {
            s.push_str(&format!("background-image = {:?}\n", background_image));
            if let Some(background_image_opacity) = self.background_image_opacity {
                s.push_str(&format!(
                    "background-image-opacity = {:.2}\n",
                    background_image_opacity.max(0.0)
                ));
            }
            if let Some(background_image_position) = &self.background_image_position {
                s.push_str(&format!(
                    "background-image-position = {}\n",
                    background_image_position
                ));
            }
            if let Some(background_image_fit) = &self.background_image_fit {
                s.push_str(&format!(
                    "background-image-fit = {}\n",
                    background_image_fit
                ));
            }
            if let Some(background_image_repeat) = self.background_image_repeat {
                s.push_str(&format!(
                    "background-image-repeat = {}\n",
                    background_image_repeat
                ));
            }
        }
        s
    }

    fn write_config_file(&self) -> Result<std::path::PathBuf, String> {
        let dir = std::env::temp_dir().join("con-ghostty");
        std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir: {}", e))?;
        let path = dir.join("runtime.conf");
        let mut f = std::fs::File::create(&path).map_err(|e| format!("create: {}", e))?;
        f.write_all(self.to_config_string().as_bytes())
            .map_err(|e| format!("write: {}", e))?;
        Ok(path)
    }
}

fn build_ghostty_config(patch: &GhosttyConfigPatch) -> Result<ffi::ghostty_config_t, String> {
    let config = unsafe { ffi::ghostty_config_new() };
    if config.is_null() {
        return Err("ghostty_config_new returned null".into());
    }

    // Always load Con's runtime config, even when no appearance patch fields
    // are present. Some terminal behavior is part of Con's product default
    // rather than a user patch, most notably shell integration features.
    let path = patch.write_config_file()?;
    let path_str = path.to_str().ok_or("non-UTF8 path")?;
    let cpath = CString::new(path_str).map_err(|e| format!("CString: {}", e))?;
    unsafe { ffi::ghostty_config_load_file(config, cpath.as_ptr()) };

    unsafe { ffi::ghostty_config_finalize(config) };
    Ok(config)
}

fn normalize_cursor_style(style: &str) -> &'static str {
    match style.trim().to_ascii_lowercase().as_str() {
        "block" => "block",
        "underline" => "underline",
        "block_hollow" | "block-hollow" | "hollow" => "block_hollow",
        _ => "bar",
    }
}

// ── Per-surface state updated by action callbacks ───────────

/// Signal emitted when ghostty fires COMMAND_FINISHED (OSC 133;D).
/// Consumed once by `take_command_finished()`.
pub struct CommandFinishedSignal {
    pub exit_code: Option<i32>,
    pub duration: std::time::Duration,
}

/// A completed command from COMMAND_FINISHED, stored in history ring buffer.
#[derive(Debug, Clone)]
pub struct CommandRecord {
    pub exit_code: Option<i32>,
    pub duration: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GhosttyScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
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

/// Terminal state received via ghostty action callbacks.
/// Each GhosttyTerminal has its own instance, stored as surface userdata.
pub struct TerminalState {
    pub title: Option<String>,
    pub pwd: Option<String>,
    pub needs_render: bool,
    pub child_exited: bool,
    /// Last exit code from COMMAND_FINISHED (persists across commands).
    pub last_exit_code: Option<i32>,
    /// Last command duration from COMMAND_FINISHED.
    pub last_command_duration: Option<std::time::Duration>,
    /// One-shot signal for the most recent COMMAND_FINISHED.
    /// Set by action callback, consumed by terminal_exec handler.
    pub command_finished_signal: Option<CommandFinishedSignal>,
    /// Circular buffer of recent command completions (last 20).
    pub command_history: Vec<CommandRecord>,
    /// Whether a command is currently running (between command_start and command_finished).
    /// Set true when we write a command to PTY, cleared by COMMAND_FINISHED.
    pub is_busy: bool,
    /// Monotonic counter incremented whenever input is sent into the PTY.
    pub input_generation: u64,
    /// The latest input generation that was followed by a shell command finish event.
    pub last_command_finished_input_generation: u64,
    /// Surface handle — stored so clipboard callbacks can complete requests.
    pub surface: ffi::ghostty_surface_t,
    /// Pending host-side events emitted by Ghostty actions for this surface.
    pub pending_events: VecDeque<GhosttySurfaceEvent>,
    /// Latest viewport scrollbar state emitted by Ghostty.
    pub scrollbar: Option<GhosttyScrollbar>,
}

const MAX_COMMAND_HISTORY: usize = 20;

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            title: None,
            pwd: None,
            needs_render: false,
            child_exited: false,
            last_exit_code: None,
            last_command_duration: None,
            command_finished_signal: None,
            command_history: Vec::with_capacity(MAX_COMMAND_HISTORY),
            is_busy: false,
            input_generation: 0,
            last_command_finished_input_generation: 0,
            surface: std::ptr::null_mut(),
            pending_events: VecDeque::new(),
            scrollbar: None,
        }
    }
}

pub type StateRef = Arc<Mutex<TerminalState>>;

// ── One-time global init ────────────────────────────────────

static GHOSTTY_INIT: Once = Once::new();
static mut GHOSTTY_INIT_RESULT: i32 = -1;

fn perf_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CON_GHOSTTY_PROFILE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

fn ensure_ghostty_init() -> Result<(), String> {
    ensure_resources_dir_env();

    GHOSTTY_INIT.call_once(|| {
        let ret = unsafe { ffi::ghostty_init(0, std::ptr::null_mut()) };
        unsafe { GHOSTTY_INIT_RESULT = ret };
    });
    let ret = unsafe { GHOSTTY_INIT_RESULT };
    if ret != 0 {
        Err(format!("ghostty_init failed with code {}", ret))
    } else {
        Ok(())
    }
}

fn ensure_resources_dir_env() {
    if std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some() {
        return;
    }

    let resources_dir = installed_app_ghostty_resources_dir().or_else(|| {
        let resources_dir = option_env!("CON_GHOSTTY_RESOURCES_DIR")?;
        Path::new(resources_dir)
            .is_dir()
            .then(|| PathBuf::from(resources_dir))
    });

    let Some(resources_dir) = resources_dir else {
        return;
    };

    // SAFETY: this runs during Ghostty initialization before the app spins up
    // worker threads, so mutating process env here is acceptable.
    unsafe {
        std::env::set_var("GHOSTTY_RESOURCES_DIR", resources_dir);
    }
}

fn installed_app_ghostty_resources_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    installed_app_ghostty_resources_dir_for_exe(&exe)
}

fn installed_app_ghostty_resources_dir_for_exe(exe: &Path) -> Option<PathBuf> {
    for ancestor in exe.ancestors() {
        if ancestor.extension().is_some_and(|ext| ext == "app") {
            let resources_root = ancestor.join("Contents/Resources");
            let ghostty_dir = resources_root.join("ghostty");
            let terminfo_dir = resources_root.join("terminfo");
            if ghostty_dir.is_dir() && has_xterm_ghostty_terminfo(&terminfo_dir) {
                return Some(ghostty_dir);
            }
        }
    }
    None
}

fn has_xterm_ghostty_terminfo(terminfo_dir: &Path) -> bool {
    // Compiled terminfo databases normally bucket `xterm-ghostty` by either
    // its first byte in hex (`78`) or first character (`x`). Check those
    // directly so startup does not walk the whole bundled database.
    for bucket in ["78", "x"] {
        if terminfo_dir.join(bucket).join("xterm-ghostty").is_file() {
            return true;
        }
    }

    // Keep a compatibility fallback for alternate tic layouts, but only after
    // the known O(1) bundle layouts fail.
    contains_file_named(terminfo_dir, std::ffi::OsStr::new("xterm-ghostty"))
}

fn contains_file_named(dir: &Path, file_name: &std::ffi::OsStr) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_file() && entry.file_name() == file_name {
            return true;
        }
        if file_type.is_dir() && contains_file_named(&entry.path(), file_name) {
            return true;
        }
    }

    false
}

// ── GhosttyApp — singleton managing all surfaces ────────────

/// Global ghostty application context. One per process.
///
/// Must be ticked from the main thread — ghostty's Metal renderer
/// requires main thread access on macOS.
pub struct GhosttyApp {
    app: ffi::ghostty_app_t,
    // Box prevents the runtime_config from being moved while ghostty holds a pointer.
    _runtime_config: Box<ffi::ghostty_runtime_config_s>,
    appearance: Mutex<GhosttyConfigPatch>,
    wake_handle: Arc<GhosttyWakeHandle>,
}

struct GhosttyWakeHandle {
    app: AtomicPtr<c_void>,
    tick_scheduled: AtomicBool,
    generation: AtomicU64,
}

impl Default for GhosttyWakeHandle {
    fn default() -> Self {
        Self {
            app: AtomicPtr::new(std::ptr::null_mut()),
            tick_scheduled: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        }
    }
}

impl GhosttyApp {
    /// Create a new ghostty app with the given terminal colors.
    pub fn new(
        colors: Option<&TerminalColors>,
        font_family: Option<&str>,
        font_size: Option<f32>,
        background_opacity: Option<f32>,
        background_blur: Option<bool>,
        cursor_style: Option<&str>,
        background_image: Option<&str>,
        background_image_opacity: Option<f32>,
        background_image_position: Option<&str>,
        background_image_fit: Option<&str>,
        background_image_repeat: Option<bool>,
    ) -> Result<Self, String> {
        ensure_ghostty_init()?;

        let appearance = GhosttyConfigPatch {
            colors: colors.cloned(),
            font_family: font_family.map(ToOwned::to_owned),
            font_size,
            background_opacity,
            background_opacity_cells: background_opacity.map(|opacity| opacity < 0.999),
            background_blur,
            cursor_style: cursor_style
                .map(normalize_cursor_style)
                .map(ToOwned::to_owned),
            background_image: background_image.map(ToOwned::to_owned),
            background_image_opacity,
            background_image_position: background_image_position.map(ToOwned::to_owned),
            background_image_fit: background_image_fit.map(ToOwned::to_owned),
            background_image_repeat,
        };
        let config = build_ghostty_config(&appearance)?;

        let wake_handle = Arc::new(GhosttyWakeHandle::default());
        let runtime_config = Box::new(ffi::ghostty_runtime_config_s {
            userdata: Arc::as_ptr(&wake_handle) as *mut c_void,
            supports_selection_clipboard: false,
            wakeup_cb: Some(wakeup_callback),
            action_cb: Some(action_callback),
            // These MUST be Some — ghostty dereferences them without null checks.
            read_clipboard_cb: Some(read_clipboard_callback),
            confirm_read_clipboard_cb: Some(confirm_read_clipboard_callback),
            write_clipboard_cb: Some(write_clipboard_callback),
            close_surface_cb: Some(close_surface_callback),
        });

        let app = unsafe { ffi::ghostty_app_new(&*runtime_config as *const _, config) };

        // Ghostty clones the config — we must free the original.
        unsafe { ffi::ghostty_config_free(config) };

        if app.is_null() {
            return Err("ghostty_app_new returned null".into());
        }
        wake_handle
            .app
            .store(app.cast::<c_void>(), Ordering::Release);

        Ok(Self {
            app,
            _runtime_config: runtime_config,
            appearance: Mutex::new(appearance),
            wake_handle,
        })
    }

    /// Drive the ghostty event loop.
    ///
    /// **Must be called from the main thread** — ghostty's Metal renderer
    /// and AppKit operations require it.
    pub fn tick(&self) {
        unsafe { ffi::ghostty_app_tick(self.app) }
    }

    /// Monotonic counter bumped after every Ghostty wakeup-driven app tick.
    pub fn wake_generation(&self) -> u64 {
        self.wake_handle.generation.load(Ordering::Acquire)
    }

    /// Current configured terminal background, used by the embedding UI for
    /// short-lived native-view layout mattes before Ghostty has painted.
    pub fn background_rgb(&self) -> Option<[u8; 3]> {
        self.appearance
            .lock()
            .colors
            .as_ref()
            .map(|colors| colors.background)
    }

    /// Current configured terminal foreground color.
    pub fn foreground_rgb(&self) -> Option<[u8; 3]> {
        self.appearance
            .lock()
            .colors
            .as_ref()
            .map(|colors| colors.foreground)
    }

    /// Current configured terminal background opacity. This lets the macOS
    /// embedding layer keep short-lived AppKit backing mattes visually aligned
    /// with Ghostty's own transparent terminal background.
    pub fn background_opacity(&self) -> Option<f32> {
        self.appearance.lock().background_opacity
    }

    /// Update the app's terminal colors at runtime.
    pub fn update_colors(&self, colors: &TerminalColors) -> Result<(), String> {
        self.update_config(&GhosttyConfigPatch {
            colors: Some(colors.clone()),
            font_family: None,
            font_size: None,
            background_opacity: None,
            background_opacity_cells: None,
            background_blur: None,
            cursor_style: None,
            background_image: None,
            background_image_opacity: None,
            background_image_position: None,
            background_image_fit: None,
            background_image_repeat: None,
        })
    }

    pub fn update_appearance(
        &self,
        colors: &TerminalColors,
        font_family: &str,
        font_size: f32,
        background_opacity: f32,
        background_blur: bool,
        cursor_style: &str,
        background_image: Option<&str>,
        background_image_opacity: f32,
        background_image_position: Option<&str>,
        background_image_fit: Option<&str>,
        background_image_repeat: bool,
    ) -> Result<(), String> {
        self.update_config(&GhosttyConfigPatch {
            colors: Some(colors.clone()),
            font_family: Some(font_family.to_string()),
            font_size: Some(font_size),
            background_opacity: Some(background_opacity),
            background_opacity_cells: Some(background_opacity < 0.999),
            background_blur: Some(background_blur),
            cursor_style: Some(normalize_cursor_style(cursor_style).to_string()),
            background_image: background_image.map(ToOwned::to_owned),
            background_image_opacity: background_image.map(|_| background_image_opacity),
            background_image_position: background_image_position.map(ToOwned::to_owned),
            background_image_fit: background_image_fit.map(ToOwned::to_owned),
            background_image_repeat: background_image.map(|_| background_image_repeat),
        })
    }

    pub fn update_config(&self, patch: &GhosttyConfigPatch) -> Result<(), String> {
        let merged = {
            let mut appearance = self.appearance.lock();
            appearance.merge(patch);
            appearance.clone()
        };
        let config = build_ghostty_config(&merged)?;
        unsafe {
            ffi::ghostty_app_update_config(self.app, config);
            ffi::ghostty_config_free(config);
        }
        Ok(())
    }

    /// Set the global color scheme.
    pub fn set_color_scheme(&self, dark: bool) {
        let scheme = if dark {
            ffi::ghostty_color_scheme_e::GHOSTTY_COLOR_SCHEME_DARK
        } else {
            ffi::ghostty_color_scheme_e::GHOSTTY_COLOR_SCHEME_LIGHT
        };
        unsafe { ffi::ghostty_app_set_color_scheme(self.app, scheme) }
    }

    /// Create a new terminal surface. The `nsview` must be a valid NSView pointer
    /// that ghostty will attach its Metal IOSurfaceLayer to.
    ///
    /// Each surface gets its own `TerminalState` for independent title/pwd tracking.
    #[cfg(target_os = "macos")]
    pub fn new_surface(
        &self,
        nsview: *mut c_void,
        scale_factor: f64,
        cwd: Option<&str>,
        restored_screen_text: Option<&[String]>,
        font_size: Option<f32>,
    ) -> Result<GhosttyTerminal, String> {
        // Per-surface state — stored as surface userdata so callbacks can
        // update the correct terminal's state.
        let state: StateRef = Arc::new(Mutex::new(TerminalState::default()));
        let surface_userdata = Box::into_raw(Box::new(state.clone())) as *mut c_void;

        let mut config = unsafe { ffi::ghostty_surface_config_new() };
        config.platform_tag =
            ffi::ghostty_platform_e::GHOSTTY_PLATFORM_MACOS as std::os::raw::c_int;
        config.platform = ffi::ghostty_platform_u {
            macos: ffi::ghostty_platform_macos_s { nsview },
        };
        config.userdata = surface_userdata;
        config.scale_factor = scale_factor;
        config.context = ffi::ghostty_surface_context_e::GHOSTTY_SURFACE_CONTEXT_TAB;
        config.font_size = font_size.unwrap_or(14.0);

        let cwd_cstr = cwd.and_then(|s| CString::new(s).ok());
        if let Some(ref s) = cwd_cstr {
            config.working_directory = s.as_ptr();
        }

        #[cfg(con_ghostty_embedded_initial_output)]
        let restored_output = restored_screen_text.and_then(restored_terminal_output);
        #[cfg(con_ghostty_embedded_initial_output)]
        if let Some(ref output) = restored_output {
            config.initial_output = output.as_ptr();
        }
        #[cfg(not(con_ghostty_embedded_initial_output))]
        let _ = restored_screen_text;

        let surface = unsafe { ffi::ghostty_surface_new(self.app, &config as *const _) };
        if surface.is_null() {
            // Clean up the userdata we allocated
            unsafe { drop(Box::from_raw(surface_userdata as *mut StateRef)) };
            return Err("ghostty_surface_new returned null".into());
        }

        // Store the surface handle so clipboard callbacks can complete requests.
        state.lock().surface = surface;

        Ok(GhosttyTerminal {
            surface,
            state,
            userdata_ptr: surface_userdata,
        })
    }

    /// Raw app handle.
    pub fn raw(&self) -> ffi::ghostty_app_t {
        self.app
    }
}

#[cfg(con_ghostty_embedded_initial_output)]
fn restored_terminal_output(lines: &[String]) -> Option<CString> {
    CString::new(crate::restored_terminal_output_text(lines)?).ok()
}

impl Drop for GhosttyApp {
    fn drop(&mut self) {
        self.wake_handle
            .app
            .store(std::ptr::null_mut(), Ordering::Release);
        // Free the app first — this allows ghostty to run any final cleanup
        // and fire callbacks while userdata is still valid.
        unsafe { ffi::ghostty_app_free(self.app) }
    }
}

// SAFETY: GhosttyApp's C-side state is protected by ghostty's internal
// synchronization. The Rust-side runtime_config is heap-pinned and read-only.
unsafe impl Send for GhosttyApp {}
unsafe impl Sync for GhosttyApp {}

// ── GhosttyTerminal — a single terminal surface ─────────────

/// A single ghostty terminal surface backed by GPU-accelerated Metal rendering.
///
/// Ghostty renders directly into the NSView provided at creation time.
/// Input is forwarded via the ghostty_surface_* APIs. State updates
/// (title, pwd) arrive via the per-surface action callback.
pub struct GhosttyTerminal {
    surface: ffi::ghostty_surface_t,
    state: StateRef,
    /// Raw pointer to the Box<StateRef> we allocated for surface userdata.
    /// Must be recovered and freed when the terminal is dropped.
    userdata_ptr: *mut c_void,
}

impl GhosttyTerminal {
    fn mark_input_observed(&self) {
        let mut state = self.state.lock();
        state.input_generation = state.input_generation.saturating_add(1);
    }

    /// Trigger a draw (ghostty renders into its Metal layer).
    pub fn draw(&self) {
        unsafe { ffi::ghostty_surface_draw(self.surface) }
    }

    /// Request a refresh (marks the surface as needing redraw).
    pub fn refresh(&self) {
        unsafe { ffi::ghostty_surface_refresh(self.surface) }
    }

    /// Set the surface size in pixels.
    pub fn set_size(&self, width_px: u32, height_px: u32) {
        unsafe { ffi::ghostty_surface_set_size(self.surface, width_px, height_px) }
    }

    /// Get the current size (columns, rows, pixel dimensions, cell size).
    pub fn size(&self) -> SurfaceSize {
        let s = unsafe { ffi::ghostty_surface_size(self.surface) };
        SurfaceSize {
            columns: s.columns,
            rows: s.rows,
            width_px: s.width_px,
            height_px: s.height_px,
            cell_width_px: s.cell_width_px,
            cell_height_px: s.cell_height_px,
        }
    }

    pub fn scrollbar(&self) -> Option<GhosttyScrollbar> {
        self.state.lock().scrollbar
    }

    /// Set content scale (e.g., 2.0 for Retina).
    pub fn set_content_scale(&self, scale: f64) {
        unsafe { ffi::ghostty_surface_set_content_scale(self.surface, scale, scale) }
    }

    /// Set focus state.
    pub fn set_focus(&self, focused: bool) {
        unsafe { ffi::ghostty_surface_set_focus(self.surface, focused) }
    }

    /// Set occlusion state (hidden behind other windows).
    pub fn set_occlusion(&self, occluded: bool) {
        unsafe { ffi::ghostty_surface_set_occlusion(self.surface, occluded) }
    }

    /// Set color scheme (light/dark).
    pub fn set_color_scheme(&self, dark: bool) {
        let scheme = if dark {
            ffi::ghostty_color_scheme_e::GHOSTTY_COLOR_SCHEME_DARK
        } else {
            ffi::ghostty_color_scheme_e::GHOSTTY_COLOR_SCHEME_LIGHT
        };
        unsafe { ffi::ghostty_surface_set_color_scheme(self.surface, scheme) }
    }

    pub fn perform_binding_action(&self, action: &str) -> Result<bool, String> {
        let action = CString::new(action).map_err(|e| format!("CString: {}", e))?;
        Ok(unsafe {
            ffi::ghostty_surface_binding_action(
                self.surface,
                action.as_ptr(),
                action.as_bytes().len(),
            )
        })
    }

    pub fn clear_screen_and_scrollback(&self) -> Result<(), String> {
        let handled = self.perform_binding_action("clear_screen")?;
        if handled {
            Ok(())
        } else {
            Err("Ghostty rejected clear_screen binding action".to_string())
        }
    }

    pub fn request_split(&self, direction: GhosttySplitDirection) {
        let direction = match direction {
            GhosttySplitDirection::Right => {
                ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_RIGHT
            }
            GhosttySplitDirection::Down => {
                ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_DOWN
            }
            GhosttySplitDirection::Left => {
                ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_LEFT
            }
            GhosttySplitDirection::Up => {
                ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_UP
            }
        };
        unsafe { ffi::ghostty_surface_split(self.surface, direction) }
    }

    pub fn update_config(&self, patch: &GhosttyConfigPatch) -> Result<(), String> {
        let config = build_ghostty_config(patch)?;
        unsafe {
            ffi::ghostty_surface_update_config(self.surface, config);
            ffi::ghostty_config_free(config);
        }
        self.refresh();
        Ok(())
    }

    pub fn update_appearance(
        &self,
        colors: &TerminalColors,
        font_family: &str,
        font_size: f32,
        background_opacity: f32,
        background_blur: bool,
        cursor_style: &str,
        background_image: Option<&str>,
        background_image_opacity: f32,
        background_image_position: Option<&str>,
        background_image_fit: Option<&str>,
        background_image_repeat: bool,
    ) -> Result<(), String> {
        self.update_config(&GhosttyConfigPatch {
            colors: Some(colors.clone()),
            font_family: Some(font_family.to_string()),
            font_size: Some(font_size),
            background_opacity: Some(background_opacity),
            background_opacity_cells: Some(background_opacity < 0.999),
            background_blur: Some(background_blur),
            cursor_style: Some(normalize_cursor_style(cursor_style).to_string()),
            background_image: background_image.map(ToOwned::to_owned),
            background_image_opacity: background_image.map(|_| background_image_opacity),
            background_image_position: background_image_position.map(ToOwned::to_owned),
            background_image_fit: background_image_fit.map(ToOwned::to_owned),
            background_image_repeat: background_image.map(|_| background_image_repeat),
        })?;
        self.refresh();
        Ok(())
    }

    /// Send a key event to the terminal. Returns true if ghostty consumed it.
    pub fn send_key(&self, key: ffi::ghostty_input_key_s) -> bool {
        self.mark_input_observed();
        unsafe { ffi::ghostty_surface_key(self.surface, key) }
    }

    /// Send UTF-8 text input to the terminal (for composed/IME text).
    ///
    /// Note: this uses `ghostty_surface_text` which is the IME/compose pipeline.
    /// It handles printable characters but NOT control characters like `\n` or `\r`.
    /// For writing command strings with newlines, use `write_to_pty` instead.
    pub fn send_text(&self, text: &str) {
        self.mark_input_observed();
        if let Ok(cstr) = CString::new(text) {
            let len = cstr.as_bytes().len(); // excludes NUL, matches original text
            unsafe { ffi::ghostty_surface_text(self.surface, cstr.as_ptr(), len) }
        }
        // If text contains NUL bytes, we silently drop it — this matches
        // terminal semantics where NUL in text input is meaningless.
    }

    pub fn ime_point(&self) -> ImePoint {
        let mut x = 0.0;
        let mut y = 0.0;
        let mut width = 0.0;
        let mut height = 0.0;
        unsafe {
            ffi::ghostty_surface_ime_point(self.surface, &mut x, &mut y, &mut width, &mut height);
        }
        ImePoint {
            x,
            y,
            width,
            height,
        }
    }

    /// Write raw bytes to the terminal via the key event path.
    ///
    /// All characters — both printable and control — are sent through
    /// `ghostty_surface_key`. Control characters (Enter, Tab, Escape, Backspace)
    /// use their macOS keycodes. Printable text is batched and sent via the
    /// key event's `text` field, which ghostty writes directly to the PTY as
    /// UTF-8 without bracketed-paste wrapping or control-character stripping.
    ///
    /// This is critical for TUI interaction: `ghostty_surface_text` (the IME/paste
    /// pipeline) wraps all input in bracketed paste markers (`\x1b[200~…\x1b[201~`)
    /// when the application has mode 2004 enabled (vim, neovim, etc.), and strips
    /// control characters. The key event path bypasses all of that.
    pub fn write_to_pty(&self, data: &[u8]) {
        let text = String::from_utf8_lossy(data);

        // If this looks like a command (ends with newline), mark as busy
        if text.contains('\n') {
            self.state.lock().is_busy = true;
        }

        let mut pending_text = String::new();

        for ch in text.chars() {
            if let Some(key_event) = char_to_key_event(ch) {
                // Flush any pending printable text before sending a control char
                if !pending_text.is_empty() {
                    self.send_text_as_key_event(&pending_text);
                    pending_text.clear();
                }
                self.send_key(key_event);
            } else {
                pending_text.push(ch);
            }
        }

        if !pending_text.is_empty() {
            self.send_text_as_key_event(&pending_text);
        }
    }

    /// Send printable text through the key event path, bypassing the paste pipeline.
    ///
    /// Unlike `send_text()` which uses `ghostty_surface_text` (triggers bracketed
    /// paste wrapping in mode 2004), this sends via `ghostty_surface_key` with the
    /// `text` field set. Ghostty writes the UTF-8 directly to the PTY.
    pub fn send_text_as_key_event(&self, text: &str) {
        if let Ok(cstr) = CString::new(text) {
            let key_event = ffi::ghostty_input_key_s {
                action: ffi::ghostty_input_action_e::GHOSTTY_ACTION_PRESS,
                mods: 0,
                consumed_mods: 0,
                keycode: 0xFFFF, // unmapped — text field carries the content
                text: cstr.as_ptr(),
                unshifted_codepoint: 0,
                composing: false,
            };
            self.send_key(key_event);
        }
    }

    /// Send a mouse button event.
    pub fn send_mouse_button(&self, pressed: bool, button: MouseButton, mods: i32) -> bool {
        let state = if pressed {
            ffi::ghostty_input_mouse_state_e::GHOSTTY_MOUSE_PRESS
        } else {
            ffi::ghostty_input_mouse_state_e::GHOSTTY_MOUSE_RELEASE
        };
        let btn = match button {
            MouseButton::Left => ffi::ghostty_input_mouse_button_e::GHOSTTY_MOUSE_LEFT,
            MouseButton::Right => ffi::ghostty_input_mouse_button_e::GHOSTTY_MOUSE_RIGHT,
            MouseButton::Middle => ffi::ghostty_input_mouse_button_e::GHOSTTY_MOUSE_MIDDLE,
        };
        unsafe { ffi::ghostty_surface_mouse_button(self.surface, state, btn, mods) }
    }

    /// Send mouse position event.
    pub fn send_mouse_pos(&self, x: f64, y: f64, mods: i32) {
        unsafe { ffi::ghostty_surface_mouse_pos(self.surface, x, y, mods) }
    }

    /// Send mouse scroll event.
    pub fn send_mouse_scroll(&self, x: f64, y: f64, mods: i32) {
        unsafe { ffi::ghostty_surface_mouse_scroll(self.surface, x, y, mods) }
    }

    /// Request close (sends signal to child process).
    pub fn request_close(&self) {
        unsafe { ffi::ghostty_surface_request_close(self.surface) }
    }

    // ── State queries ───────────────────────────────────────

    /// Terminal title (from per-surface action callback, set by OSC 0/1/2).
    pub fn title(&self) -> Option<String> {
        self.state.lock().title.clone()
    }

    /// Working directory (from per-surface action callback, set by OSC 7).
    pub fn current_dir(&self) -> Option<String> {
        self.state.lock().pwd.clone()
    }

    /// Whether the child process has exited.
    pub fn is_alive(&self) -> bool {
        !unsafe { ffi::ghostty_surface_process_exited(self.surface) }
    }

    /// Take the command-finished signal (if any). Consuming — returns None on second call.
    /// Used by terminal_exec to detect command completion with exit code.
    pub fn take_command_finished(&self) -> Option<CommandFinishedSignal> {
        self.state.lock().command_finished_signal.take()
    }

    /// Last exit code from the most recent COMMAND_FINISHED action.
    pub fn last_exit_code(&self) -> Option<i32> {
        self.state.lock().last_exit_code
    }

    /// Last command duration from the most recent COMMAND_FINISHED action.
    pub fn last_command_duration(&self) -> Option<std::time::Duration> {
        self.state.lock().last_command_duration
    }

    /// Whether a command is currently running (between write_to_pty and COMMAND_FINISHED).
    pub fn is_busy(&self) -> bool {
        self.state.lock().is_busy
    }

    /// Mark the terminal as busy (called when writing a command to PTY).
    pub fn set_busy(&self) {
        self.state.lock().is_busy = true;
    }

    /// Recent command history (exit codes + durations, last 20).
    pub fn command_history(&self) -> Vec<CommandRecord> {
        self.state.lock().command_history.clone()
    }

    /// Monotonic counter that advances whenever input is sent to the PTY.
    pub fn input_generation(&self) -> u64 {
        self.state.lock().input_generation
    }

    /// Latest input generation confirmed by Ghostty shell integration as finished.
    pub fn last_command_finished_input_generation(&self) -> u64 {
        self.state.lock().last_command_finished_input_generation
    }

    /// Recover shell-ready state when the prompt is visibly back but Ghostty did not
    /// emit a matching COMMAND_FINISHED signal for the latest input generation.
    pub fn recover_shell_prompt_state(&self) {
        let mut state = self.state.lock();
        state.is_busy = false;
        state.last_command_finished_input_generation = state.input_generation;
    }

    /// Whether the terminal has a text selection.
    pub fn has_selection(&self) -> bool {
        unsafe { ffi::ghostty_surface_has_selection(self.surface) }
    }

    /// Read the current selection text. Returns None if no selection.
    pub fn selection_text(&self) -> Option<String> {
        let mut text = ffi::ghostty_text_s {
            tl_px_x: 0.0,
            tl_px_y: 0.0,
            offset_start: 0,
            offset_len: 0,
            text: std::ptr::null(),
            text_len: 0,
        };
        let ok = unsafe { ffi::ghostty_surface_read_selection(self.surface, &mut text) };
        if !ok || text.text.is_null() || text.text_len == 0 {
            return None;
        }
        let result = unsafe {
            let bytes = std::slice::from_raw_parts(text.text as *const u8, text.text_len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe { ffi::ghostty_surface_free_text(self.surface, &mut text) };
        Some(result)
    }

    /// Read text from a specific screen region.
    pub fn read_text(&self, selection: ffi::ghostty_selection_s) -> Option<String> {
        let mut text = ffi::ghostty_text_s {
            tl_px_x: 0.0,
            tl_px_y: 0.0,
            offset_start: 0,
            offset_len: 0,
            text: std::ptr::null(),
            text_len: 0,
        };
        let ok = unsafe { ffi::ghostty_surface_read_text(self.surface, selection, &mut text) };
        if !ok || text.text.is_null() || text.text_len == 0 {
            return None;
        }
        let result = unsafe {
            let bytes = std::slice::from_raw_parts(text.text as *const u8, text.text_len);
            String::from_utf8_lossy(bytes).into_owned()
        };
        unsafe { ffi::ghostty_surface_free_text(self.surface, &mut text) };
        Some(result)
    }

    /// Read visible screen text, returning the last `max_lines` lines.
    ///
    /// Uses ghostty's `read_text` API with a viewport-sized selection to
    /// extract the current terminal content. This enables agent tools to
    /// read ghostty terminal output.
    pub fn read_screen_text(&self, max_lines: usize) -> Vec<String> {
        let size = self.size();
        if size.columns == 0 || size.rows == 0 {
            return Vec::new();
        }

        // Select the entire viewport (visible area only)
        let selection = ffi::ghostty_selection_s {
            top_left: ffi::ghostty_point_s {
                tag: ffi::ghostty_point_tag_e::GHOSTTY_POINT_VIEWPORT,
                coord: ffi::ghostty_point_coord_e::GHOSTTY_POINT_COORD_TOP_LEFT,
                x: 0,
                y: 0,
            },
            bottom_right: ffi::ghostty_point_s {
                tag: ffi::ghostty_point_tag_e::GHOSTTY_POINT_VIEWPORT,
                coord: ffi::ghostty_point_coord_e::GHOSTTY_POINT_COORD_BOTTOM_RIGHT,
                x: (size.columns - 1) as u32,
                y: (size.rows - 1) as u32,
            },
            rectangle: false,
        };

        match self.read_text(selection) {
            Some(text) => {
                let lines: Vec<String> = text.lines().map(String::from).collect();
                if lines.len() > max_lines {
                    lines[lines.len() - max_lines..].to_vec()
                } else {
                    lines
                }
            }
            None => Vec::new(),
        }
    }

    /// Read recent lines including scrollback, returning the last `max_lines`.
    ///
    /// Uses SCREEN coordinates to access the full scrollback buffer,
    /// not just the visible viewport.
    pub fn read_recent_lines(&self, max_lines: usize) -> Vec<String> {
        let size = self.size();
        if size.columns == 0 || size.rows == 0 {
            return Vec::new();
        }

        // Select from far back in scrollback to current viewport bottom.
        // SCREEN coordinates cover the full scrollback buffer.
        let selection = ffi::ghostty_selection_s {
            top_left: ffi::ghostty_point_s {
                tag: ffi::ghostty_point_tag_e::GHOSTTY_POINT_SCREEN,
                coord: ffi::ghostty_point_coord_e::GHOSTTY_POINT_COORD_TOP_LEFT,
                x: 0,
                y: 0,
            },
            bottom_right: ffi::ghostty_point_s {
                tag: ffi::ghostty_point_tag_e::GHOSTTY_POINT_SCREEN,
                coord: ffi::ghostty_point_coord_e::GHOSTTY_POINT_COORD_BOTTOM_RIGHT,
                x: (size.columns - 1) as u32,
                // Use a large Y to capture all scrollback
                y: u32::MAX,
            },
            rectangle: false,
        };

        match self.read_text(selection) {
            Some(text) => {
                let lines: Vec<String> = text.lines().map(String::from).collect();
                if lines.len() > max_lines {
                    lines[lines.len() - max_lines..].to_vec()
                } else {
                    lines
                }
            }
            None => {
                // Fallback: try viewport if screen selection fails
                self.read_screen_text(max_lines)
            }
        }
    }

    /// Search terminal text (viewport + scrollback) for a pattern.
    /// Returns (line_number, matched_line) tuples, up to `limit` results.
    pub fn search_text(&self, pattern: &str, limit: usize) -> Vec<(usize, String)> {
        let lines = self.read_recent_lines(5000); // read up to 5000 lines of scrollback
        let mut results = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if line.contains(pattern) {
                results.push((i, line.clone()));
                if results.len() >= limit {
                    break;
                }
            }
        }
        results
    }

    /// Check and clear the needs_render flag.
    pub fn take_needs_render(&self) -> bool {
        let mut state = self.state.lock();
        let r = state.needs_render;
        state.needs_render = false;
        r
    }

    /// Access the per-surface state.
    pub fn state(&self) -> &StateRef {
        &self.state
    }

    /// Raw FFI surface handle.
    pub fn raw_surface(&self) -> ffi::ghostty_surface_t {
        self.surface
    }

    pub fn take_pending_events(&self) -> Vec<GhosttySurfaceEvent> {
        let mut state = self.state.lock();
        state.pending_events.drain(..).collect()
    }
}

impl Drop for GhosttyTerminal {
    fn drop(&mut self) {
        // Free the surface first — ghostty may fire callbacks during cleanup
        // while userdata is still valid.
        unsafe { ffi::ghostty_surface_free(self.surface) };
        // Recover the Box<StateRef> we allocated in new_surface.
        // After surface_free, ghostty no longer references this pointer.
        if !self.userdata_ptr.is_null() {
            unsafe { drop(Box::from_raw(self.userdata_ptr as *mut StateRef)) };
        }
    }
}

// SAFETY: The ghostty surface is thread-safe — all state access is mutex-protected.
unsafe impl Send for GhosttyTerminal {}
unsafe impl Sync for GhosttyTerminal {}

// ── Public types ────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct SurfaceSize {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ImePoint {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

// ── Control character → key event mapping ────────────────────
//
// ghostty_surface_text is the IME text input pipeline and only handles
// printable characters. Control characters (Enter, Tab, Escape, etc.)
// must go through ghostty_surface_key with the appropriate macOS keycode.

fn char_to_key_event(ch: char) -> Option<ffi::ghostty_input_key_s> {
    let keycode = match ch {
        '\n' | '\r' => 0x24, // kVK_Return
        '\t' => 0x30,        // kVK_Tab
        '\x1b' => 0x35,      // kVK_Escape
        '\x7f' => 0x33,      // kVK_Delete (backspace)
        _ => return None,
    };
    Some(ffi::ghostty_input_key_s {
        action: ffi::ghostty_input_action_e::GHOSTTY_ACTION_PRESS,
        mods: 0,
        consumed_mods: 0,
        keycode,
        text: std::ptr::null(),
        unshifted_codepoint: 0,
        composing: false,
    })
}

// ── C callback implementations ──────────────────────────────

/// Resolve per-surface state from a ghostty_target_s.
/// For SURFACE-targeted actions, reads the surface's userdata.
/// Returns None if the target is app-level or has no userdata.
unsafe fn resolve_surface_state(target: &ffi::ghostty_target_s) -> Option<StateRef> {
    unsafe {
        if target.tag != ffi::ghostty_target_tag_e::GHOSTTY_TARGET_SURFACE {
            return None;
        }
        let surface = target.target.surface;
        if surface.is_null() {
            return None;
        }
        let userdata = ffi::ghostty_surface_userdata(surface);
        if userdata.is_null() {
            return None;
        }
        let state_ref = &*(userdata as *const StateRef);
        Some(state_ref.clone())
    }
}

unsafe extern "C" fn wakeup_callback(userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }

    let wake_ptr = userdata.cast::<GhosttyWakeHandle>();
    unsafe {
        Arc::increment_strong_count(wake_ptr);
    }
    let wake_handle = unsafe { Arc::from_raw(wake_ptr) };
    if wake_handle.app.load(Ordering::Acquire).is_null() {
        return;
    }

    if wake_handle.tick_scheduled.swap(true, Ordering::AcqRel) {
        return;
    }

    let scheduled_at = Instant::now();
    Queue::main().exec_async(move || {
        wake_handle.tick_scheduled.store(false, Ordering::Release);
        let app = wake_handle.app.load(Ordering::Acquire) as ffi::ghostty_app_t;
        if app.is_null() {
            return;
        }
        let queue_delay = scheduled_at.elapsed();
        let tick_started = Instant::now();
        unsafe { ffi::ghostty_app_tick(app) };
        let tick_elapsed = tick_started.elapsed();
        if perf_trace_enabled() {
            log::info!(
                target: "con_ghostty::perf",
                "ghostty wake tick queue_delay_ms={:.3} tick_ms={:.3}",
                queue_delay.as_secs_f64() * 1000.0,
                tick_elapsed.as_secs_f64() * 1000.0
            );
        }
        wake_handle.generation.fetch_add(1, Ordering::AcqRel);
    });
}

fn mark_child_exited_state(state: &StateRef) {
    let mut s = state.lock();
    s.child_exited = true;
    s.needs_render = true;
    s.is_busy = false;
    s.last_command_finished_input_generation = s.input_generation;
}

unsafe extern "C" fn action_callback(
    _app: ffi::ghostty_app_t,
    target: ffi::ghostty_target_s,
    action: ffi::ghostty_action_s,
) -> bool {
    unsafe {
        let state = match resolve_surface_state(&target) {
            Some(s) => s,
            None => return false,
        };

        match action.tag {
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_SET_TITLE => {
                let title_ptr = action.action.set_title.title;
                if !title_ptr.is_null() {
                    let title = CStr::from_ptr(title_ptr).to_string_lossy().into_owned();
                    state.lock().title = Some(title);
                }
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_PWD => {
                let pwd_ptr = action.action.pwd.pwd;
                if !pwd_ptr.is_null() {
                    let pwd = CStr::from_ptr(pwd_ptr).to_string_lossy().into_owned();
                    let mut state = state.lock();
                    if state.pwd.as_deref() != Some(pwd.as_str()) {
                        state
                            .pending_events
                            .push_back(GhosttySurfaceEvent::PwdChanged(pwd.clone()));
                    }
                    state.pwd = Some(pwd);
                }
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_RENDER => {
                state.lock().needs_render = true;
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_SCROLLBAR => {
                let scrollbar = action.action.scrollbar;
                state.lock().scrollbar = Some(GhosttyScrollbar {
                    total: scrollbar.total,
                    offset: scrollbar.offset,
                    len: scrollbar.len,
                });
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_DESKTOP_NOTIFICATION => {
                let notification = action.action.desktop_notification;
                con_ghostty_show_desktop_notification(notification.title, notification.body)
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_NEW_SPLIT => {
                let direction = match action.action.new_split {
                    ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_RIGHT => {
                        GhosttySplitDirection::Right
                    }
                    ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_DOWN => {
                        GhosttySplitDirection::Down
                    }
                    ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_LEFT => {
                        GhosttySplitDirection::Left
                    }
                    ffi::ghostty_action_split_direction_e::GHOSTTY_SPLIT_DIRECTION_UP => {
                        GhosttySplitDirection::Up
                    }
                };
                state
                    .lock()
                    .pending_events
                    .push_back(GhosttySurfaceEvent::SplitRequest(direction));
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_OPEN_URL => {
                let open_url = action.action.open_url;
                if open_url.url.is_null() || open_url.len == 0 {
                    return false;
                }

                let bytes = std::slice::from_raw_parts(open_url.url as *const u8, open_url.len);
                let url = String::from_utf8_lossy(bytes).into_owned();
                state
                    .lock()
                    .pending_events
                    .push_back(GhosttySurfaceEvent::OpenUrl(url));
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_COMMAND_FINISHED => {
                let cf = action.action.command_finished;
                let exit_code = if cf.exit_code < 0 {
                    None
                } else {
                    Some(cf.exit_code as i32)
                };
                let duration = std::time::Duration::from_nanos(cf.duration);
                let mut s = state.lock();
                s.last_exit_code = exit_code;
                s.last_command_duration = Some(duration);
                s.command_finished_signal = Some(CommandFinishedSignal {
                    exit_code,
                    duration,
                });
                s.is_busy = false;
                s.last_command_finished_input_generation = s.input_generation;
                // Append to command history ring buffer
                if s.command_history.len() >= MAX_COMMAND_HISTORY {
                    s.command_history.remove(0);
                }
                s.command_history.push(CommandRecord {
                    exit_code,
                    duration,
                });
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_SHOW_CHILD_EXITED => {
                mark_child_exited_state(&state);
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_COLOR_CHANGE => {
                state.lock().needs_render = true;
                true
            }
            ffi::ghostty_action_tag_e::GHOSTTY_ACTION_RING_BELL => {
                // macOS system beep
                #[cfg(target_os = "macos")]
                {
                    unsafe extern "C" {
                        fn NSBeep();
                    }
                    NSBeep();
                }
                true
            }
            _ => false,
        }
    }
}

unsafe extern "C" {
    fn con_ghostty_show_desktop_notification(
        title: *const std::os::raw::c_char,
        body: *const std::os::raw::c_char,
    ) -> bool;
}

/// Clipboard read — ghostty wants to paste. Read from macOS pasteboard and complete the request.
unsafe extern "C" fn read_clipboard_callback(
    userdata: *mut c_void,
    clipboard: ffi::ghostty_clipboard_e,
    request: *mut c_void,
    mime_types: *const *const std::os::raw::c_char,
    mime_types_len: usize,
    needs_listing: bool,
) -> ffi::ghostty_clipboard_read_result_e {
    unsafe {
        // macOS has no X11-style primary clipboard. Keep the pre-migration
        // behavior that maps standard and selection requests to NSPasteboard.
        if userdata.is_null()
            || request.is_null()
            || clipboard == ffi::ghostty_clipboard_e::GHOSTTY_CLIPBOARD_PRIMARY
        {
            return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNSUPPORTED;
        }

        let state = &*(userdata as *const StateRef);
        let surface = state.lock().surface;
        if surface.is_null() {
            return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNSUPPORTED;
        }

        #[cfg(target_os = "macos")]
        {
            use objc::runtime::Object;
            use objc::{class, msg_send, sel, sel_impl};

            if mime_types_len > 0 && mime_types.is_null() {
                return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNAVAILABLE;
            }
            // MIME listings are used by Kitty OSC 5522 reads and paste events.
            // Keep that protocol unavailable until its permission flow lands;
            // otherwise an `ask` policy still exposes clipboard metadata for
            // listing-only requests that upstream intentionally exempts from a
            // content confirmation prompt.
            if needs_listing {
                return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNSUPPORTED;
            }
            let requested = if mime_types_len == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(mime_types, mime_types_len)
            };
            let wants_text = requested
                .iter()
                .copied()
                .any(|mime| clipboard_mime_is_text(mime));
            if !wants_text {
                return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNAVAILABLE;
            }

            let pb: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
            // NSPasteboardTypeString = @"public.utf8-plain-text"
            let ns_type: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c"public.utf8-plain-text".as_ptr()];
            let text: *mut Object = msg_send![pb, stringForType: ns_type];
            if text.is_null() {
                return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNAVAILABLE;
            }
            let data: *const std::os::raw::c_char = msg_send![text, UTF8String];
            if data.is_null() {
                return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNAVAILABLE;
            }
            let len: usize = msg_send![text, lengthOfBytesUsingEncoding: 4usize];
            let mime = c"text/plain";
            let content = ffi::ghostty_clipboard_content_s {
                mime: mime.as_ptr(),
                data,
                len,
            };
            let complete = ffi::ghostty_clipboard_complete_s {
                contents: &content,
                contents_len: 1,
                available: std::ptr::null(),
                available_len: 0,
                confirmed: false,
                remember: false,
            };
            ffi::ghostty_surface_complete_clipboard_request(surface, &complete, request);
            return ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_STARTED;
        }

        #[cfg(not(target_os = "macos"))]
        ffi::ghostty_clipboard_read_result_e::GHOSTTY_CLIPBOARD_READ_UNSUPPORTED
    }
}

/// Clipboard confirmation — ghostty confirmed a clipboard read (e.g. OSC 52).
unsafe extern "C" fn confirm_read_clipboard_callback(
    userdata: *mut c_void,
    confirm: *const ffi::ghostty_clipboard_confirm_s,
    request: *mut c_void,
    request_type: ffi::ghostty_clipboard_request_e,
) {
    unsafe {
        if userdata.is_null() || request.is_null() {
            return;
        }
        let state = &*(userdata as *const StateRef);
        let surface = state.lock().surface;
        if surface.is_null() {
            return;
        }

        if matches!(
            request_type,
            ffi::ghostty_clipboard_request_e::GHOSTTY_CLIPBOARD_REQUEST_KITTY_READ
                | ffi::ghostty_clipboard_request_e::GHOSTTY_CLIPBOARD_REQUEST_KITTY_WRITE
        ) || confirm.is_null()
        {
            // Kitty grants require a real user-visible permission flow. Until
            // Con can present one, deny instead of silently granting a shell
            // persistent access to the system clipboard.
            ffi::ghostty_surface_deny_clipboard_request(surface, request);
            return;
        }

        let confirm = &*confirm;
        let complete = ffi::ghostty_clipboard_complete_s {
            contents: confirm.contents,
            contents_len: confirm.contents_len,
            available: confirm.available,
            available_len: confirm.available_len,
            confirmed: true,
            remember: false,
        };
        ffi::ghostty_surface_complete_clipboard_request(surface, &complete, request);
    }
}

/// Clipboard write — ghostty wants to copy (selection, OSC 52). Write to macOS pasteboard.
unsafe extern "C" fn write_clipboard_callback(
    _userdata: *mut c_void,
    clipboard: ffi::ghostty_clipboard_e,
    content: *const ffi::ghostty_clipboard_content_s,
    content_count: usize,
    _confirm: bool,
) {
    unsafe {
        // Standard and selection both map to the macOS general pasteboard;
        // primary is unsupported on this platform.
        if clipboard == ffi::ghostty_clipboard_e::GHOSTTY_CLIPBOARD_PRIMARY
            || content.is_null()
            || content_count == 0
        {
            return;
        }

        #[cfg(target_os = "macos")]
        {
            use objc::runtime::Object;
            use objc::{class, msg_send, sel, sel_impl};

            let items = std::slice::from_raw_parts(content, content_count);

            for item in items {
                if !clipboard_mime_is_text(item.mime) || (item.data.is_null() && item.len > 0) {
                    continue;
                }
                let data = if item.len == 0 {
                    c"".as_ptr()
                } else {
                    item.data
                };
                let ns_str: *mut Object = msg_send![
                    class!(NSString),
                    stringWithBytes: data
                    length: item.len
                    encoding: 4usize
                ];
                if ns_str.is_null() {
                    continue;
                }

                // NSPasteboardTypeString = @"public.utf8-plain-text"
                let ns_type: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c"public.utf8-plain-text".as_ptr()];
                let pb: *mut Object = msg_send![class!(NSPasteboard), generalPasteboard];
                let _: () = msg_send![pb, clearContents];
                let _success: bool = msg_send![pb, setString: ns_str forType: ns_type];
                return;
            }
        }
    }
}

unsafe fn clipboard_mime_is_text(mime: *const std::os::raw::c_char) -> bool {
    if mime.is_null() {
        return false;
    }
    matches!(
        unsafe { CStr::from_ptr(mime) }.to_bytes(),
        b"text/plain" | b"text/plain;charset=utf-8" | b"UTF8_STRING" | b"TEXT" | b"STRING"
    )
}

unsafe extern "C" fn close_surface_callback(userdata: *mut c_void, _process_alive: bool) {
    // userdata here is the surface's userdata (per-surface StateRef)
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &*(userdata as *const StateRef) };
    mark_child_exited_state(state);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::{
        GhosttyConfigPatch, TerminalColors, TerminalState,
        installed_app_ghostty_resources_dir_for_exe, mark_child_exited_state,
    };

    fn sample_colors(seed: u8) -> TerminalColors {
        TerminalColors {
            foreground: [seed, seed.saturating_add(1), seed.saturating_add(2)],
            background: [
                seed.saturating_add(3),
                seed.saturating_add(4),
                seed.saturating_add(5),
            ],
            palette: [[seed; 3]; 16],
        }
    }

    #[test]
    fn mark_child_exited_state_clears_busy_and_marks_input_finished() {
        let state = Arc::new(Mutex::new(TerminalState {
            is_busy: true,
            input_generation: 7,
            last_command_finished_input_generation: 3,
            ..TerminalState::default()
        }));

        mark_child_exited_state(&state);

        let state = state.lock();
        assert!(state.child_exited);
        assert!(state.needs_render);
        assert!(!state.is_busy);
        assert_eq!(state.last_command_finished_input_generation, 7);
    }

    #[test]
    fn config_patch_merge_preserves_existing_colors() {
        let original_colors = sample_colors(10);
        let mut patch = GhosttyConfigPatch {
            colors: Some(original_colors.clone()),
            font_size: Some(14.0),
            ..Default::default()
        };

        patch.merge(&GhosttyConfigPatch {
            colors: None,
            font_size: Some(16.0),
            ..Default::default()
        });

        assert_eq!(
            patch.colors.as_ref().map(|c| c.background),
            Some(original_colors.background)
        );
        assert_eq!(
            patch.colors.as_ref().map(|c| c.foreground),
            Some(original_colors.foreground)
        );
        assert_eq!(patch.font_size, Some(16.0));
    }

    #[test]
    fn config_patch_merge_replaces_colors_when_present() {
        let replacement_colors = sample_colors(40);
        let mut patch = GhosttyConfigPatch {
            colors: Some(sample_colors(10)),
            font_size: Some(14.0),
            ..Default::default()
        };

        patch.merge(&GhosttyConfigPatch {
            colors: Some(replacement_colors.clone()),
            font_size: None,
            ..Default::default()
        });

        assert_eq!(
            patch.colors.as_ref().map(|c| c.background),
            Some(replacement_colors.background)
        );
        assert_eq!(
            patch.colors.as_ref().map(|c| c.foreground),
            Some(replacement_colors.foreground)
        );
        assert_eq!(patch.font_size, Some(14.0));
    }

    #[test]
    fn ghostty_config_sanitizes_gpui_pseudo_font_family() {
        let patch = GhosttyConfigPatch {
            font_family: Some(".ZedMono".to_string()),
            ..Default::default()
        };

        let config = patch.to_config_string();
        assert!(config.contains("font-family = \"Ioskeley Mono\""));
        assert!(!config.contains(".ZedMono"));
    }

    #[test]
    fn ghostty_config_always_includes_con_shell_integration_features() {
        let config = GhosttyConfigPatch::default().to_config_string();

        assert!(config.contains("shell-integration-features = no-cursor,ssh-env,ssh-terminfo"));
    }

    #[test]
    fn ghostty_config_rejects_kitty_clipboard_writes_until_permission_ui_exists() {
        let config = GhosttyConfigPatch::default().to_config_string();

        assert!(config.contains("clipboard-write = ask"));
        assert!(config.contains("clipboard-write-limit-bytes = 0"));
    }

    #[test]
    fn finds_ghostty_resources_inside_installed_app_bundle() {
        let (_cleanup, root) = temp_test_dir("con-ghostty-resources-test");
        let app_root = root.join("con Beta.app");
        let macos_dir = app_root.join("Contents/MacOS");
        let resources_dir = app_root.join("Contents/Resources/ghostty");
        let terminfo_file = app_root.join("Contents/Resources/terminfo/78/xterm-ghostty");
        std::fs::create_dir_all(&macos_dir).unwrap();
        std::fs::create_dir_all(&resources_dir).unwrap();
        std::fs::create_dir_all(terminfo_file.parent().unwrap()).unwrap();
        std::fs::write(&terminfo_file, b"terminfo").unwrap();

        let found = installed_app_ghostty_resources_dir_for_exe(&macos_dir.join("con"));

        assert_eq!(found.as_deref(), Some(resources_dir.as_path()));
    }

    #[test]
    fn finds_ghostty_resources_when_terminfo_uses_letter_bucket() {
        let (_cleanup, root) = temp_test_dir("con-ghostty-letter-terminfo-test");
        let app_root = root.join("con Beta.app");
        let macos_dir = app_root.join("Contents/MacOS");
        let resources_dir = app_root.join("Contents/Resources/ghostty");
        let terminfo_file = app_root.join("Contents/Resources/terminfo/x/xterm-ghostty");
        std::fs::create_dir_all(&macos_dir).unwrap();
        std::fs::create_dir_all(&resources_dir).unwrap();
        std::fs::create_dir_all(terminfo_file.parent().unwrap()).unwrap();
        std::fs::write(&terminfo_file, b"terminfo").unwrap();

        let found = installed_app_ghostty_resources_dir_for_exe(&macos_dir.join("con"));

        assert_eq!(found.as_deref(), Some(resources_dir.as_path()));
    }

    #[test]
    fn ignores_installed_app_resources_when_bundled_terminfo_is_missing() {
        let (_cleanup, root) = temp_test_dir("con-ghostty-missing-terminfo-test");
        let app_root = root.join("con.app");
        let macos_dir = app_root.join("Contents/MacOS");
        let resources_dir = app_root.join("Contents/Resources/ghostty");
        std::fs::create_dir_all(&macos_dir).unwrap();
        std::fs::create_dir_all(&resources_dir).unwrap();

        let found = installed_app_ghostty_resources_dir_for_exe(&macos_dir.join("con"));

        assert_eq!(found, None);
    }

    struct Cleanup(std::path::PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_test_dir(prefix: &str) -> (Cleanup, std::path::PathBuf) {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("{prefix}-{}-{unique}", std::process::id()));
        (Cleanup(root.clone()), root)
    }
}
