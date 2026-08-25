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

#[cfg(con_ghostty_embedded_initial_output)]
const _: [(); 96] = [(); std::mem::size_of::<ghostty_surface_config_s>()];
#[cfg(not(con_ghostty_embedded_initial_output))]
const _: [(); 88] = [(); std::mem::size_of::<ghostty_surface_config_s>()];

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
    GHOSTTY_ACTION_NEW_WINDOW = 1,
    GHOSTTY_ACTION_NEW_TAB = 2,
    GHOSTTY_ACTION_CLOSE_TAB = 3,
    GHOSTTY_ACTION_NEW_SPLIT = 4,
    GHOSTTY_ACTION_CLOSE_ALL_WINDOWS = 5,
    GHOSTTY_ACTION_TOGGLE_MAXIMIZE = 6,
    GHOSTTY_ACTION_TOGGLE_FULLSCREEN = 7,
    GHOSTTY_ACTION_TOGGLE_TAB_OVERVIEW = 8,
    GHOSTTY_ACTION_TOGGLE_WINDOW_DECORATIONS = 9,
    GHOSTTY_ACTION_TOGGLE_QUICK_TERMINAL = 10,
    GHOSTTY_ACTION_TOGGLE_COMMAND_PALETTE = 11,
    GHOSTTY_ACTION_TOGGLE_VISIBILITY = 12,
    GHOSTTY_ACTION_TOGGLE_BACKGROUND_OPACITY = 13,
    GHOSTTY_ACTION_MOVE_TAB = 14,
    GHOSTTY_ACTION_GOTO_TAB = 15,
    GHOSTTY_ACTION_GOTO_SPLIT = 16,
    GHOSTTY_ACTION_GOTO_WINDOW = 17,
    GHOSTTY_ACTION_RESIZE_SPLIT = 18,
    GHOSTTY_ACTION_EQUALIZE_SPLITS = 19,
    GHOSTTY_ACTION_TOGGLE_SPLIT_ZOOM = 20,
    GHOSTTY_ACTION_PRESENT_TERMINAL = 21,
    GHOSTTY_ACTION_SIZE_LIMIT = 22,
    GHOSTTY_ACTION_RESET_WINDOW_SIZE = 23,
    GHOSTTY_ACTION_INITIAL_SIZE = 24,
    GHOSTTY_ACTION_CELL_SIZE = 25,
    GHOSTTY_ACTION_SCROLLBAR = 26,
    GHOSTTY_ACTION_RENDER = 27,
    GHOSTTY_ACTION_INSPECTOR = 28,
    GHOSTTY_ACTION_SHOW_GTK_INSPECTOR = 29,
    GHOSTTY_ACTION_RENDER_INSPECTOR = 30,
    GHOSTTY_ACTION_EXPORT_TERMINAL_IO = 31,
    GHOSTTY_ACTION_DESKTOP_NOTIFICATION = 32,
    GHOSTTY_ACTION_SET_TITLE = 33,
    GHOSTTY_ACTION_SET_TAB_TITLE = 34,
    GHOSTTY_ACTION_SET_WINDOW_TITLE = 35,
    GHOSTTY_ACTION_PROMPT_TITLE = 36,
    GHOSTTY_ACTION_PWD = 37,
    GHOSTTY_ACTION_MOUSE_SHAPE = 38,
    GHOSTTY_ACTION_MOUSE_VISIBILITY = 39,
    GHOSTTY_ACTION_MOUSE_OVER_LINK = 40,
    GHOSTTY_ACTION_RENDERER_HEALTH = 41,
    GHOSTTY_ACTION_OPEN_CONFIG = 42,
    GHOSTTY_ACTION_QUIT_TIMER = 43,
    GHOSTTY_ACTION_FLOAT_WINDOW = 44,
    GHOSTTY_ACTION_SECURE_INPUT = 45,
    GHOSTTY_ACTION_KEY_SEQUENCE = 46,
    GHOSTTY_ACTION_KEY_TABLE = 47,
    GHOSTTY_ACTION_COLOR_CHANGE = 48,
    GHOSTTY_ACTION_RELOAD_CONFIG = 49,
    GHOSTTY_ACTION_CONFIG_CHANGE = 50,
    GHOSTTY_ACTION_CLOSE_WINDOW = 51,
    GHOSTTY_ACTION_RING_BELL = 52,
    GHOSTTY_ACTION_SELECTION_CHANGED = 53,
    GHOSTTY_ACTION_UNDO = 54,
    GHOSTTY_ACTION_REDO = 55,
    GHOSTTY_ACTION_CHECK_FOR_UPDATES = 56,
    GHOSTTY_ACTION_OPEN_URL = 57,
    GHOSTTY_ACTION_SHOW_CHILD_EXITED = 58,
    GHOSTTY_ACTION_PROGRESS_REPORT = 59,
    GHOSTTY_ACTION_SHOW_ON_SCREEN_KEYBOARD = 60,
    GHOSTTY_ACTION_COMMAND_FINISHED = 61,
    GHOSTTY_ACTION_START_SEARCH = 62,
    GHOSTTY_ACTION_END_SEARCH = 63,
    GHOSTTY_ACTION_SEARCH_TOTAL = 64,
    GHOSTTY_ACTION_SEARCH_SELECTED = 65,
    GHOSTTY_ACTION_READONLY = 66,
    GHOSTTY_ACTION_COPY_TITLE_TO_CLIPBOARD = 67,
    GHOSTTY_ACTION_MOVE_TAB_TO_NEW_WINDOW = 68,
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
    GHOSTTY_ACTION_OPEN_URL_KIND_OSC8 = 3,
}

/// Action payload for OPEN_URL.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_action_open_url_s {
    pub kind: ghostty_action_open_url_kind_e,
    pub url: *const c_char,
    pub len: usize,
}

/// Action payload for SHOW_CHILD_EXITED.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ghostty_surface_message_childexited_s {
    pub exit_code: u32,
    /// Runtime in milliseconds. The pinned C header calls this `timetime_ms`.
    pub runtime_ms: u64,
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
    pub child_exited: ghostty_surface_message_childexited_s,
}

#[repr(C)]
pub struct ghostty_action_s {
    pub tag: ghostty_action_tag_e,
    pub action: ghostty_action_u,
}

// The action is passed by value to ghostty_runtime_action_cb, so these sizes
// must exactly match ghostty.h at the pinned Ghostty revision.
const _: [(); 16] = [(); std::mem::size_of::<ghostty_action_desktop_notification_s>()];
const _: [(); 16] = [(); std::mem::size_of::<ghostty_surface_message_childexited_s>()];
const _: [(); 24] = [(); std::mem::size_of::<ghostty_action_u>()];
const _: [(); 32] = [(); std::mem::size_of::<ghostty_action_s>()];

// ── Clipboard types ─────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_clipboard_e {
    GHOSTTY_CLIPBOARD_STANDARD = 0,
    GHOSTTY_CLIPBOARD_SELECTION = 1,
    GHOSTTY_CLIPBOARD_PRIMARY = 2,
}

#[repr(C)]
pub struct ghostty_clipboard_content_s {
    pub mime: *const c_char,
    pub data: *const c_char,
    pub len: usize,
}

#[repr(C)]
pub struct ghostty_clipboard_complete_s {
    pub contents: *const ghostty_clipboard_content_s,
    pub contents_len: usize,
    pub available: *const *const c_char,
    pub available_len: usize,
    pub confirmed: bool,
    pub remember: bool,
}

#[repr(C)]
pub struct ghostty_clipboard_confirm_s {
    pub contents: *const ghostty_clipboard_content_s,
    pub contents_len: usize,
    pub available: *const *const c_char,
    pub available_len: usize,
    pub name: *const c_char,
    pub can_remember: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_clipboard_request_e {
    GHOSTTY_CLIPBOARD_REQUEST_PASTE = 0,
    GHOSTTY_CLIPBOARD_REQUEST_OSC_52_READ = 1,
    GHOSTTY_CLIPBOARD_REQUEST_OSC_52_WRITE = 2,
    GHOSTTY_CLIPBOARD_REQUEST_KITTY_READ = 3,
    GHOSTTY_CLIPBOARD_REQUEST_KITTY_WRITE = 4,
    GHOSTTY_CLIPBOARD_REQUEST_LIST = 5,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ghostty_clipboard_read_result_e {
    GHOSTTY_CLIPBOARD_READ_STARTED = 0,
    GHOSTTY_CLIPBOARD_READ_UNAVAILABLE = 1,
    GHOSTTY_CLIPBOARD_READ_UNSUPPORTED = 2,
}

const _: [(); 24] = [(); std::mem::size_of::<ghostty_clipboard_content_s>()];
const _: [(); 40] = [(); std::mem::size_of::<ghostty_clipboard_complete_s>()];
const _: [(); 48] = [(); std::mem::size_of::<ghostty_clipboard_confirm_s>()];

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
        mime_types: *const *const c_char,
        mime_types_len: usize,
        needs_listing: bool,
    ) -> ghostty_clipboard_read_result_e,
>;

pub type ghostty_runtime_confirm_read_clipboard_cb = Option<
    unsafe extern "C" fn(
        userdata: *mut c_void,
        confirm: *const ghostty_clipboard_confirm_s,
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

const _: [(); 64] = [(); std::mem::size_of::<ghostty_runtime_config_s>()];

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
        complete: *const ghostty_clipboard_complete_s,
        request: *mut c_void,
    );
    pub fn ghostty_surface_deny_clipboard_request(surface: ghostty_surface_t, request: *mut c_void);

    // macOS-specific
    #[cfg(target_os = "macos")]
    pub fn ghostty_surface_set_display_id(surface: ghostty_surface_t, display_id: u32);
}
