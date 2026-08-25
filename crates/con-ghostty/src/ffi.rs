//! Raw FFI bindings to libghostty's C embedding API.
//!
//! These types and functions correspond exactly to upstream ghostty.h.
//! Do NOT add custom APIs here — contribute upstream if needed.
#![allow(non_camel_case_types, dead_code)]

use std::os::raw::{c_char, c_double, c_int, c_void};

// ── Opaque handles ──────────────────────────────────────────

pub type ghostty_app_t = *mut c_void;
pub type ghostty_surface_t = *mut c_void;
pub type ghostty_config_t = *mut c_void;
pub type ghostty_inspector_t = *mut c_void;

// ── Platform ────────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_platform_e {
    GHOSTTY_PLATFORM_INVALID = 0,
    GHOSTTY_PLATFORM_MACOS = 1,
    GHOSTTY_PLATFORM_IOS = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_platform_macos_s {
    pub nsview: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_platform_ios_s {
    pub uiview: *mut c_void,
}

#[repr(C)]
pub union ghostty_platform_u {
    pub macos: ghostty_platform_macos_s,
    pub ios: ghostty_platform_ios_s,
}

// ── Color scheme ────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_color_scheme_e {
    GHOSTTY_COLOR_SCHEME_LIGHT = 0,
    GHOSTTY_COLOR_SCHEME_DARK = 1,
}

// ── Input types ─────────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_input_action_e {
    GHOSTTY_ACTION_RELEASE = 0,
    GHOSTTY_ACTION_PRESS = 1,
    GHOSTTY_ACTION_REPEAT = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_input_mouse_state_e {
    GHOSTTY_MOUSE_RELEASE = 0,
    GHOSTTY_MOUSE_PRESS = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_input_mouse_button_e {
    GHOSTTY_MOUSE_UNKNOWN = 0,
    GHOSTTY_MOUSE_LEFT = 1,
    GHOSTTY_MOUSE_RIGHT = 2,
    GHOSTTY_MOUSE_MIDDLE = 3,
    GHOSTTY_MOUSE_FOUR = 4,
    GHOSTTY_MOUSE_FIVE = 5,
    GHOSTTY_MOUSE_SIX = 6,
    GHOSTTY_MOUSE_SEVEN = 7,
    GHOSTTY_MOUSE_EIGHT = 8,
    GHOSTTY_MOUSE_NINE = 9,
    GHOSTTY_MOUSE_TEN = 10,
    GHOSTTY_MOUSE_ELEVEN = 11,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_input_mods_e {
    GHOSTTY_MODS_NONE = 0,
}

// Modifier flags (can be OR'd together)
pub const GHOSTTY_MODS_SHIFT: c_int = 1 << 0;
pub const GHOSTTY_MODS_CTRL: c_int = 1 << 1;
pub const GHOSTTY_MODS_ALT: c_int = 1 << 2;
pub const GHOSTTY_MODS_SUPER: c_int = 1 << 3;
pub const GHOSTTY_MODS_CAPS: c_int = 1 << 4;
pub const GHOSTTY_MODS_NUM: c_int = 1 << 5;

/// Packed scroll modifier struct (see ghostty input/mouse.zig).
pub type ghostty_input_scroll_mods_t = c_int;
pub const GHOSTTY_SCROLL_MODS_PRECISION: ghostty_input_scroll_mods_t = 1 << 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_input_key_s {
    pub action: ghostty_input_action_e,
    pub mods: c_int, // ghostty_input_mods_e bitmask
    pub consumed_mods: c_int,
    pub keycode: u32,
    pub text: *const c_char,
    pub unshifted_codepoint: u32,
    pub composing: bool,
}

// ── Surface types ───────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_surface_context_e {
    GHOSTTY_SURFACE_CONTEXT_WINDOW = 0,
    GHOSTTY_SURFACE_CONTEXT_TAB = 1,
    GHOSTTY_SURFACE_CONTEXT_SPLIT = 2,
}

#[repr(C)]
pub struct ghostty_surface_config_s {
    pub platform_tag: c_int,
    pub platform: ghostty_platform_u,
    pub userdata: *mut c_void,
    pub scale_factor: c_double,
    pub font_size: f32,
    pub working_directory: *const c_char,
    pub command: *const c_char,
    pub env_vars: *mut ghostty_env_var_s,
    pub env_var_count: usize,
    pub initial_input: *const c_char,
    #[cfg(con_ghostty_embedded_initial_output)]
    pub initial_output: *const c_char,
    pub wait_after_command: bool,
    pub context: ghostty_surface_context_e,
}

#[repr(C)]
pub struct ghostty_env_var_s {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_surface_size_s {
    pub columns: u16,
    pub rows: u16,
    pub width_px: u32,
    pub height_px: u32,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

// ── Text / selection types ──────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_text_s {
    pub tl_px_x: c_double,
    pub tl_px_y: c_double,
    pub offset_start: u32,
    pub offset_len: u32,
    pub text: *const c_char,
    pub text_len: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_point_tag_e {
    GHOSTTY_POINT_ACTIVE = 0,
    GHOSTTY_POINT_VIEWPORT = 1,
    GHOSTTY_POINT_SCREEN = 2,
    GHOSTTY_POINT_SURFACE = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_point_coord_e {
    GHOSTTY_POINT_COORD_EXACT = 0,
    GHOSTTY_POINT_COORD_TOP_LEFT = 1,
    GHOSTTY_POINT_COORD_BOTTOM_RIGHT = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_point_s {
    pub tag: ghostty_point_tag_e,
    pub coord: ghostty_point_coord_e,
    pub x: u32,
    pub y: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_selection_s {
    pub top_left: ghostty_point_s,
    pub bottom_right: ghostty_point_s,
    pub rectangle: bool,
}

// ── Action callback types ───────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_target_tag_e {
    GHOSTTY_TARGET_APP = 0,
    GHOSTTY_TARGET_SURFACE = 1,
}

#[repr(C)]
pub union ghostty_target_u {
    pub surface: ghostty_surface_t,
}

#[repr(C)]
pub struct ghostty_target_s {
    pub tag: ghostty_target_tag_e,
    pub target: ghostty_target_u,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ghostty_action_split_direction_e {
    GHOSTTY_SPLIT_DIRECTION_RIGHT = 0,
    GHOSTTY_SPLIT_DIRECTION_DOWN,
    GHOSTTY_SPLIT_DIRECTION_LEFT,
    GHOSTTY_SPLIT_DIRECTION_UP,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ghostty_action_goto_split_e {
    GHOSTTY_GOTO_SPLIT_PREVIOUS = 0,
    GHOSTTY_GOTO_SPLIT_NEXT,
    GHOSTTY_GOTO_SPLIT_UP,
    GHOSTTY_GOTO_SPLIT_LEFT,
    GHOSTTY_GOTO_SPLIT_DOWN,
    GHOSTTY_GOTO_SPLIT_RIGHT,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ghostty_action_resize_split_direction_e {
    GHOSTTY_RESIZE_SPLIT_UP = 0,
    GHOSTTY_RESIZE_SPLIT_DOWN,
    GHOSTTY_RESIZE_SPLIT_LEFT,
    GHOSTTY_RESIZE_SPLIT_RIGHT,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_resize_split_s {
    pub amount: u16,
    pub direction: ghostty_action_resize_split_direction_e,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ghostty_action_tag_e {
    GHOSTTY_ACTION_QUIT = 0,
    GHOSTTY_ACTION_NEW_WINDOW,
    GHOSTTY_ACTION_NEW_TAB,
    GHOSTTY_ACTION_CLOSE_TAB,
    GHOSTTY_ACTION_NEW_SPLIT,
    GHOSTTY_ACTION_CLOSE_ALL_WINDOWS,
    GHOSTTY_ACTION_TOGGLE_MAXIMIZE,
    GHOSTTY_ACTION_TOGGLE_FULLSCREEN,
    GHOSTTY_ACTION_TOGGLE_TAB_OVERVIEW,
    GHOSTTY_ACTION_TOGGLE_WINDOW_DECORATIONS,
    GHOSTTY_ACTION_TOGGLE_QUICK_TERMINAL,
    GHOSTTY_ACTION_TOGGLE_COMMAND_PALETTE,
    GHOSTTY_ACTION_TOGGLE_VISIBILITY,
    GHOSTTY_ACTION_TOGGLE_BACKGROUND_OPACITY,
    GHOSTTY_ACTION_MOVE_TAB,
    GHOSTTY_ACTION_GOTO_TAB,
    GHOSTTY_ACTION_GOTO_SPLIT,
    GHOSTTY_ACTION_GOTO_WINDOW,
    GHOSTTY_ACTION_RESIZE_SPLIT,
    GHOSTTY_ACTION_EQUALIZE_SPLITS,
    GHOSTTY_ACTION_TOGGLE_SPLIT_ZOOM,
    GHOSTTY_ACTION_PRESENT_TERMINAL,
    GHOSTTY_ACTION_SIZE_LIMIT,
    GHOSTTY_ACTION_RESET_WINDOW_SIZE,
    GHOSTTY_ACTION_INITIAL_SIZE,
    GHOSTTY_ACTION_CELL_SIZE,
    GHOSTTY_ACTION_SCROLLBAR,
    GHOSTTY_ACTION_RENDER,
    GHOSTTY_ACTION_INSPECTOR,
    GHOSTTY_ACTION_SHOW_GTK_INSPECTOR,
    GHOSTTY_ACTION_RENDER_INSPECTOR,
    GHOSTTY_ACTION_DESKTOP_NOTIFICATION,
    GHOSTTY_ACTION_SET_TITLE,
    GHOSTTY_ACTION_SET_TAB_TITLE,
    GHOSTTY_ACTION_PROMPT_TITLE,
    GHOSTTY_ACTION_PWD,
    GHOSTTY_ACTION_MOUSE_SHAPE,
    GHOSTTY_ACTION_MOUSE_VISIBILITY,
    GHOSTTY_ACTION_MOUSE_OVER_LINK,
    GHOSTTY_ACTION_RENDERER_HEALTH,
    GHOSTTY_ACTION_OPEN_CONFIG,
    GHOSTTY_ACTION_QUIT_TIMER,
    GHOSTTY_ACTION_FLOAT_WINDOW,
    GHOSTTY_ACTION_SECURE_INPUT,
    GHOSTTY_ACTION_KEY_SEQUENCE,
    GHOSTTY_ACTION_KEY_TABLE,
    GHOSTTY_ACTION_COLOR_CHANGE,
    GHOSTTY_ACTION_RELOAD_CONFIG,
    GHOSTTY_ACTION_CONFIG_CHANGE,
    GHOSTTY_ACTION_CLOSE_WINDOW,
    GHOSTTY_ACTION_RING_BELL,
    GHOSTTY_ACTION_UNDO,
    GHOSTTY_ACTION_REDO,
    GHOSTTY_ACTION_CHECK_FOR_UPDATES,
    GHOSTTY_ACTION_OPEN_URL,
    GHOSTTY_ACTION_SHOW_CHILD_EXITED,
    GHOSTTY_ACTION_PROGRESS_REPORT,
    GHOSTTY_ACTION_SHOW_ON_SCREEN_KEYBOARD,
    GHOSTTY_ACTION_COMMAND_FINISHED,
    GHOSTTY_ACTION_START_SEARCH,
    GHOSTTY_ACTION_END_SEARCH,
    GHOSTTY_ACTION_SEARCH_TOTAL,
    GHOSTTY_ACTION_SEARCH_SELECTED,
    GHOSTTY_ACTION_READONLY,
    GHOSTTY_ACTION_COPY_TITLE_TO_CLIPBOARD,
}

/// Action payload for DESKTOP_NOTIFICATION (OSC 9 and OSC 777).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_desktop_notification_s {
    pub title: *const c_char,
    pub body: *const c_char,
}

/// Action payload for SET_TITLE.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_set_title_s {
    pub title: *const c_char,
}

/// Action payload for PWD.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_pwd_s {
    pub pwd: *const c_char,
}

/// Action payload for COMMAND_FINISHED (shell integration OSC 133;D).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_command_finished_s {
    /// Exit code: -1 if unknown, otherwise 0-255.
    pub exit_code: i16,
    /// Duration the command was running, in nanoseconds.
    pub duration: u64,
}

/// Action payload for SCROLLBAR.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_scrollbar_s {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum ghostty_action_open_url_kind_e {
    GHOSTTY_ACTION_OPEN_URL_KIND_UNKNOWN = 0,
    GHOSTTY_ACTION_OPEN_URL_KIND_TEXT = 1,
    GHOSTTY_ACTION_OPEN_URL_KIND_HTML = 2,
}

/// Action payload for OPEN_URL.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_open_url_s {
    pub kind: ghostty_action_open_url_kind_e,
    pub url: *const c_char,
    pub len: usize,
}

/// Action union — only relevant fields are accessed based on tag.
#[repr(C)]
pub union ghostty_action_u {
    pub new_split: ghostty_action_split_direction_e,
    pub goto_split: ghostty_action_goto_split_e,
    pub resize_split: ghostty_action_resize_split_s,
    pub scrollbar: ghostty_action_scrollbar_s,
    pub desktop_notification: ghostty_action_desktop_notification_s,
    pub set_title: ghostty_action_set_title_s,
    pub pwd: ghostty_action_pwd_s,
    pub command_finished: ghostty_action_command_finished_s,
    pub open_url: ghostty_action_open_url_s,
}

#[repr(C)]
pub struct ghostty_action_s {
    pub tag: ghostty_action_tag_e,
    pub action: ghostty_action_u,
}

// The action is passed by value to ghostty_runtime_action_cb, so these sizes
// must exactly match ghostty.h at the pinned Ghostty revision.
const _: [(); 16] = [(); std::mem::size_of::<ghostty_action_desktop_notification_s>()];
const _: [(); 24] = [(); std::mem::size_of::<ghostty_action_u>()];
const _: [(); 32] = [(); std::mem::size_of::<ghostty_action_s>()];

// ── Clipboard types ─────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_clipboard_e {
    GHOSTTY_CLIPBOARD_STANDARD = 0,
    GHOSTTY_CLIPBOARD_SELECTION = 1,
}

#[repr(C)]
pub struct ghostty_clipboard_content_s {
    pub mime: *const c_char,
    pub data: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_clipboard_request_e {
    GHOSTTY_CLIPBOARD_REQUEST_PASTE = 0,
    GHOSTTY_CLIPBOARD_REQUEST_OSC_52_READ = 1,
    GHOSTTY_CLIPBOARD_REQUEST_OSC_52_WRITE = 2,
}

// ── Runtime config (callbacks for embedded apprt) ───────────

pub type ghostty_runtime_wakeup_cb = Option<unsafe extern "C" fn(userdata: *mut c_void)>;

pub type ghostty_runtime_action_cb = Option<
    unsafe extern "C" fn(
        app: ghostty_app_t,
        target: ghostty_target_s,
        action: ghostty_action_s,
    ) -> bool,
>;

pub type ghostty_runtime_read_clipboard_cb = Option<
    unsafe extern "C" fn(
        userdata: *mut c_void,
        clipboard: ghostty_clipboard_e,
        request: *mut c_void,
    ) -> bool,
>;

pub type ghostty_runtime_confirm_read_clipboard_cb = Option<
    unsafe extern "C" fn(
        userdata: *mut c_void,
        text: *const c_char,
        request: *mut c_void,
        request_type: ghostty_clipboard_request_e,
    ),
>;

pub type ghostty_runtime_write_clipboard_cb = Option<
    unsafe extern "C" fn(
        userdata: *mut c_void,
        clipboard: ghostty_clipboard_e,
        content: *const ghostty_clipboard_content_s,
        content_count: usize,
        confirm: bool,
    ),
>;

pub type ghostty_runtime_close_surface_cb =
    Option<unsafe extern "C" fn(userdata: *mut c_void, process_alive: bool)>;

#[repr(C)]
pub struct ghostty_runtime_config_s {
    pub userdata: *mut c_void,
    pub supports_selection_clipboard: bool,
    pub wakeup_cb: ghostty_runtime_wakeup_cb,
    pub action_cb: ghostty_runtime_action_cb,
    pub read_clipboard_cb: ghostty_runtime_read_clipboard_cb,
    pub confirm_read_clipboard_cb: ghostty_runtime_confirm_read_clipboard_cb,
    pub write_clipboard_cb: ghostty_runtime_write_clipboard_cb,
    pub close_surface_cb: ghostty_runtime_close_surface_cb,
}

// ── C API functions ─────────────────────────────────────────

unsafe extern "C" {
    // Init
    pub fn ghostty_init(argc: usize, argv: *mut *mut c_char) -> c_int;

    // Config
    pub fn ghostty_config_new() -> ghostty_config_t;
    pub fn ghostty_config_free(config: ghostty_config_t);
    pub fn ghostty_config_load_default_files(config: ghostty_config_t);
    pub fn ghostty_config_load_file(config: ghostty_config_t, path: *const c_char);
    pub fn ghostty_config_finalize(config: ghostty_config_t);
    pub fn ghostty_config_get(
        config: ghostty_config_t,
        value: *mut c_void,
        key: *const c_char,
        key_len: usize,
    ) -> bool;

    // App
    pub fn ghostty_app_new(
        runtime_config: *const ghostty_runtime_config_s,
        config: ghostty_config_t,
    ) -> ghostty_app_t;
    pub fn ghostty_app_free(app: ghostty_app_t);
    pub fn ghostty_app_tick(app: ghostty_app_t);
    pub fn ghostty_app_userdata(app: ghostty_app_t) -> *mut c_void;
    pub fn ghostty_app_set_focus(app: ghostty_app_t, focused: bool);
    pub fn ghostty_app_update_config(app: ghostty_app_t, config: ghostty_config_t);
    pub fn ghostty_app_set_color_scheme(app: ghostty_app_t, scheme: ghostty_color_scheme_e);
    pub fn ghostty_set_window_background_blur(app: ghostty_app_t, window: *mut c_void);

    // Surface config
    pub fn ghostty_surface_config_new() -> ghostty_surface_config_s;

    // Surface lifecycle
    pub fn ghostty_surface_new(
        app: ghostty_app_t,
        config: *const ghostty_surface_config_s,
    ) -> ghostty_surface_t;
    pub fn ghostty_surface_free(surface: ghostty_surface_t);
    pub fn ghostty_surface_userdata(surface: ghostty_surface_t) -> *mut c_void;

    // Surface rendering
    pub fn ghostty_surface_draw(surface: ghostty_surface_t);
    pub fn ghostty_surface_refresh(surface: ghostty_surface_t);

    // Surface size
    pub fn ghostty_surface_set_size(surface: ghostty_surface_t, w: u32, h: u32);
    pub fn ghostty_surface_size(surface: ghostty_surface_t) -> ghostty_surface_size_s;
    pub fn ghostty_surface_set_content_scale(surface: ghostty_surface_t, x: c_double, y: c_double);

    // Surface focus / state
    pub fn ghostty_surface_set_focus(surface: ghostty_surface_t, focused: bool);
    pub fn ghostty_surface_set_occlusion(surface: ghostty_surface_t, occluded: bool);
    pub fn ghostty_surface_set_color_scheme(
        surface: ghostty_surface_t,
        scheme: ghostty_color_scheme_e,
    );
    pub fn ghostty_surface_process_exited(surface: ghostty_surface_t) -> bool;
    pub fn ghostty_surface_needs_confirm_quit(surface: ghostty_surface_t) -> bool;

    // Surface input
    pub fn ghostty_surface_key(surface: ghostty_surface_t, key: ghostty_input_key_s) -> bool;
    pub fn ghostty_surface_text(surface: ghostty_surface_t, text: *const c_char, len: usize);
    pub fn ghostty_surface_ime_point(
        surface: ghostty_surface_t,
        x: *mut c_double,
        y: *mut c_double,
        width: *mut c_double,
        height: *mut c_double,
    );
    pub fn ghostty_surface_mouse_button(
        surface: ghostty_surface_t,
        state: ghostty_input_mouse_state_e,
        button: ghostty_input_mouse_button_e,
        mods: c_int,
    ) -> bool;
    pub fn ghostty_surface_mouse_pos(
        surface: ghostty_surface_t,
        x: c_double,
        y: c_double,
        mods: c_int,
    );
    pub fn ghostty_surface_mouse_scroll(
        surface: ghostty_surface_t,
        x: c_double,
        y: c_double,
        mods: ghostty_input_scroll_mods_t,
    );
    pub fn ghostty_surface_request_close(surface: ghostty_surface_t);
    pub fn ghostty_surface_split(
        surface: ghostty_surface_t,
        direction: ghostty_action_split_direction_e,
    );
    pub fn ghostty_surface_split_focus(
        surface: ghostty_surface_t,
        direction: ghostty_action_goto_split_e,
    );
    pub fn ghostty_surface_split_resize(
        surface: ghostty_surface_t,
        direction: ghostty_action_resize_split_direction_e,
        amount: u16,
    );
    pub fn ghostty_surface_split_equalize(surface: ghostty_surface_t);

    // Surface text/selection
    pub fn ghostty_surface_has_selection(surface: ghostty_surface_t) -> bool;
    pub fn ghostty_surface_read_selection(
        surface: ghostty_surface_t,
        text: *mut ghostty_text_s,
    ) -> bool;
    pub fn ghostty_surface_read_text(
        surface: ghostty_surface_t,
        selection: ghostty_selection_s,
        text: *mut ghostty_text_s,
    ) -> bool;
    pub fn ghostty_surface_free_text(surface: ghostty_surface_t, text: *mut ghostty_text_s);
    pub fn ghostty_surface_update_config(surface: ghostty_surface_t, config: ghostty_config_t);
    pub fn ghostty_surface_binding_action(
        surface: ghostty_surface_t,
        action: *const c_char,
        arg: usize,
    ) -> bool;

    // Clipboard
    pub fn ghostty_surface_complete_clipboard_request(
        surface: ghostty_surface_t,
        text: *const c_char,
        request: *mut c_void,
        confirmed: bool,
    );

    // macOS-specific
    #[cfg(target_os = "macos")]
    pub fn ghostty_surface_set_display_id(surface: ghostty_surface_t, display_id: u32);
}
