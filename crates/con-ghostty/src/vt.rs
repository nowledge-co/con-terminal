//! Shared `libghostty-vt` FFI bindings + render-state snapshot.
//!
//! Rewritten to match the **actual** upstream API at GHOSTTY_REV
//! `5f5b988c5236facfe8d2439203d9ee9d5b636cf8` — `include/ghostty/vt/*.h`.
//! Key lifecycle:
//!
//! 1. `terminal = ghostty_terminal_new(NULL_alloc, cols, rows)`
//! 2. `state    = ghostty_render_state_new(NULL_alloc)`
//! 3. `iter     = ghostty_render_state_row_iterator_new(NULL_alloc)`
//! 4. `cells    = ghostty_render_state_row_cells_new(NULL_alloc)`
//!
//! Per-frame:
//!   - `ghostty_render_state_begin_update(state, terminal)` under the parser lock
//!   - release the parser lock, then call `ghostty_render_state_end_update(state)`
//!   - `ghostty_render_state_get_multi(...)` to read state metadata and bind the iterator
//!   - while `row_iterator_next_dirty(iter, &row)` is true:
//!       - `row_get(iter, CELLS, &cells)` to bind the dirty row
//!       - while `row_cells_next(cells)` is true:
//!           - `row_cells_get_multi(cells, RAW|STYLE|FG|BG, ...)`
//!   - `acknowledge_snapshot(generation)` cleans damage after renderer acceptance
//!
//! All `_next` functions return `bool`. The `_get` family uses an enum
//! key and writes to a typed `void*` out; key→type contract is per
//! upstream header comments.
//!
//! libghostty-vt terminal and render state access are serialized by
//! separate mutexes. The parser lock is held only through the begin
//! phase and terminal-owned metadata capture; render extraction and
//! snapshot cloning proceed without blocking parser feeds.

#![allow(non_camel_case_types, dead_code)]

use std::collections::HashMap;
use std::io::Cursor as IoCursor;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use crate::stub::GhosttyScrollbar;
use crate::{
    CLIPBOARD_WRITE_LIMIT_BYTES, ClipboardWritePolicy, DesktopNotification,
    DesktopNotificationPolicy, TERMINAL_PROGRESS_TIMEOUT, TerminalProgress,
};

use image::{ImageFormat, ImageReader, Limits};
use parking_lot::Mutex;

fn perf_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CON_GHOSTTY_PROFILE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

fn perf_trace_verbose() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CON_GHOSTTY_PROFILE_VERBOSE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

// ── Opaque types ───────────────────────────────────────────────────────

pub type GhosttyTerminal = *mut c_void;
pub type GhosttyRenderState = *mut c_void;
pub type GhosttyRowIterator = *mut c_void;
pub type GhosttyRowCells = *mut c_void;
pub type GhosttyKeyEncoder = *mut c_void;
pub type GhosttyKeyEvent = *mut c_void;
pub type GhosttyMouseEncoder = *mut c_void;
pub type GhosttyMouseEvent = *mut c_void;
pub type GhosttySelectionGesture = *mut c_void;
pub type GhosttySelectionGestureEvent = *mut c_void;
pub type GhosttyKittyGraphics = *mut c_void;
pub type GhosttyKittyGraphicsImage = *mut c_void;
pub type GhosttyKittyGraphicsPlacementIterator = *mut c_void;
pub type GhosttyAllocator = c_void;
pub type GhosttyResult = c_int;

const GHOSTTY_SUCCESS: GhosttyResult = 0;
const GHOSTTY_INVALID_VALUE: GhosttyResult = -2;
const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
const GHOSTTY_NO_VALUE: GhosttyResult = -4;
const GHOSTTY_IO_ERROR: GhosttyResult = -5;
const GHOSTTY_REJECTED: GhosttyResult = -7;

// Match libghostty's conservative embedded-library default. Keeping this
// explicit makes the PNG decoder's output cap and the terminal's retained
// image cap one contract instead of allowing a compressed payload to allocate
// far more memory than the terminal can retain.
const KITTY_IMAGE_STORAGE_LIMIT_BYTES: u64 = 10_000_000;
const KITTY_PNG_MAX_INPUT_BYTES: usize = 16 * 1024 * 1024;
const KITTY_PNG_MAX_DIMENSION: u32 = 10_000;
const KITTY_PNG_DECODER_MAX_ALLOC_BYTES: u64 = 32 * 1024 * 1024;
const TERMINAL_PASTE_LIMIT_BYTES: usize = 16 * 1024 * 1024;
// libghostty may split one paste into framing and content callbacks. Collect
// those callbacks before handing them to the bounded host queue so a queue
// rejection cannot leave a partial bracketed paste in the child process.
// Protocol framing should be tiny; the second input-sized allowance keeps a
// malformed upstream expansion bounded without constraining valid text.
const TERMINAL_PASTE_OUTPUT_LIMIT_BYTES: usize = TERMINAL_PASTE_LIMIT_BYTES * 2;
// A single tiny image can have an effectively unbounded number of explicit
// placements upstream. Bound each render snapshot so hostile terminal output
// cannot force either renderer to allocate and lay out millions of quads in
// one frame. This is intentionally far above a dense visible terminal grid.
const KITTY_PLACEMENT_SNAPSHOT_LIMIT: usize = 4_096;
// Rejected and virtual placements do not consume the render budget, but their
// iterator entries still cost FFI calls. Keep a separate traversal ceiling so
// invalid entries cannot turn one snapshot into unbounded parser work.
const KITTY_PLACEMENT_SCAN_LIMIT: usize = KITTY_PLACEMENT_SNAPSHOT_LIMIT * 4;

const GHOSTTY_MODS_SHIFT: u16 = 1 << 0;
const GHOSTTY_MODS_CTRL: u16 = 1 << 1;
const GHOSTTY_MODS_ALT: u16 = 1 << 2;
const GHOSTTY_MODS_SUPER: u16 = 1 << 3;
const GHOSTTY_KITTY_KEY_REPORT_EVENTS: u8 = 1 << 1;
const TEXT_PLAIN_MIME: &[u8] = b"text/plain";
const METADATA_DIRTY_TITLE: u8 = 1 << 0;
const METADATA_DIRTY_PWD: u8 = 1 << 1;
const UNKNOWN_SEQUENCE_LOG_TARGET: &str = "con_ghostty::vt::unknown_sequence";
const UNKNOWN_SEQUENCE_MAX_BYTES: usize = 256;
const UNKNOWN_SEQUENCE_LOG_LIMIT: u8 = 16;

// ── Enums (keys) ───────────────────────────────────────────────────────
//
// Integer values mirror `include/ghostty/vt/terminal.h` and `render.h`
// at the pinned revision. Keep in sync on GHOSTTY_REV bumps.

/// `GHOSTTY_TERMINAL_DATA_*` keys for `ghostty_terminal_get`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyTerminalData {
    Invalid = 0,
    Cols = 1,
    Rows = 2,
    CursorX = 3,
    CursorY = 4,
    CursorPendingWrap = 5,
    ActiveScreen = 6,
    CursorVisible = 7,
    KittyKeyboardFlags = 8,
    Scrollbar = 9,
    Title = 12,
    Pwd = 13,
    KittyImageStorageLimit = 26,
    KittyGraphics = 30,
    Selection = 31,
    ScrollbackMaxLines = 35,
    Mode = 37,
    CursorAtPrompt = 39,
    ClipboardWriteMaxBytes = 40,
}

/// `GhosttyTerminalScrollbar` — current viewport position in the full
/// active screen including scrollback.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyTerminalScrollbar {
    pub total: u64,
    pub offset: u64,
    pub len: u64,
}

/// `GHOSTTY_TERMINAL_SCREEN_*` values.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyTerminalScreen {
    Primary = 0,
    Alternate = 1,
}

/// `GHOSTTY_SCROLL_VIEWPORT_*` values.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyTerminalScrollViewportTag {
    Top = 0,
    Bottom = 1,
    Delta = 2,
}

/// Payload for `GHOSTTY_SCROLL_VIEWPORT_DELTA`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalScrollViewportValue {
    /// Row delta. Up is negative, down is positive.
    pub delta: isize,
    pub _padding: [u64; 2],
}

/// Tagged viewport scroll request.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalScrollViewport {
    pub tag: GhosttyTerminalScrollViewportTag,
    pub value: GhosttyTerminalScrollViewportValue,
}

/// `GHOSTTY_TERMINAL_OPT_*` keys for `ghostty_terminal_set`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyTerminalOption {
    Userdata = 0,
    WritePty = 1,
    Bell = 2,
    Enquiry = 3,
    Xtversion = 4,
    TitleChanged = 5,
    Size = 6,
    ColorScheme = 7,
    DeviceAttributes = 8,
    Title = 9,
    Pwd = 10,
    ColorForeground = 11,
    ColorBackground = 12,
    ColorCursor = 13,
    /// `GhosttyColorRgb[256]*` — full 256-entry palette.
    ColorPalette = 14,
    KittyImageStorageLimit = 15,
    Selection = 21,
    PwdChanged = 25,
    ClipboardWrite = 26,
    ScrollbackMaxLines = 28,
    DesktopNotification = 29,
    ProgressReport = 30,
    UnknownSequence = 35,
    UnknownMaxBytes = 36,
    ClipboardRead = 38,
    ClipboardWriteMaxBytes = 39,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyFormatterFormat {
    Plain = 0,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyPointTag {
    Viewport = 1,
}

#[repr(C)]
#[derive(Clone, Copy)]
union GhosttyPointValue {
    coordinate: GhosttyPointCoordinate,
    _padding: [u64; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GhosttyPoint {
    tag: GhosttyPointTag,
    value: GhosttyPointValue,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct GhosttyPointCoordinate {
    x: u16,
    y: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GhosttyGridRef {
    size: usize,
    node: *mut c_void,
    x: u16,
    y: u16,
}

impl Default for GhosttyGridRef {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            node: std::ptr::null_mut(),
            x: 0,
            y: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GhosttySelection {
    size: usize,
    start: GhosttyGridRef,
    end: GhosttyGridRef,
    rectangle: bool,
}

impl Default for GhosttySelection {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            start: GhosttyGridRef::default(),
            end: GhosttyGridRef::default(),
            rectangle: false,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GhosttyTerminalSelectionFormatOptions {
    size: usize,
    emit: GhosttyFormatterFormat,
    unwrap: bool,
    trim: bool,
    selection: *const GhosttySelection,
}

impl Default for GhosttyTerminalSelectionFormatOptions {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            emit: GhosttyFormatterFormat::Plain,
            unwrap: true,
            trim: true,
            selection: std::ptr::null(),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttySelectionGestureData {
    ClickCount = 0,
    Autoscroll = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttySelectionGestureEventType {
    Press = 0,
    Release = 1,
    Drag = 2,
    AutoscrollTick = 3,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttySelectionGestureEventOption {
    Ref = 0,
    Position = 1,
    RepeatDistance = 2,
    TimeNs = 3,
    RepeatIntervalNs = 4,
    Geometry = 8,
    Viewport = 9,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GhosttySurfacePosition {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct GhosttySelectionGestureGeometry {
    columns: u32,
    cell_width: u32,
    padding_left: u32,
    screen_height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttySelectionGestureAutoscroll {
    None = 0,
    Up = 1,
    Down = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyMouseAction {
    Press = 0,
    Release = 1,
    Motion = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyMouseButton {
    Left = 1,
    Right = 2,
    Middle = 3,
    Four = 4,
    Five = 5,
    Six = 6,
    Seven = 7,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyMouseEncoderOption {
    Size = 2,
    AnyButtonPressed = 3,
    TrackLastCell = 4,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
struct GhosttyMousePosition {
    x: f32,
    y: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GhosttyMouseEncoderSize {
    size: usize,
    screen_width: u32,
    screen_height: u32,
    cell_width: u32,
    cell_height: u32,
    padding_top: u32,
    padding_bottom: u32,
    padding_right: u32,
    padding_left: u32,
}

impl GhosttyMouseEncoderSize {
    fn new(screen_width: u32, screen_height: u32, cell_width: u32, cell_height: u32) -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            screen_width: screen_width.max(1),
            screen_height: screen_height.max(1),
            cell_width: cell_width.max(1),
            cell_height: cell_height.max(1),
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        }
    }
}

/// `GHOSTTY_SYS_OPT_*` keys for process-global optional services.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
enum GhosttySysOption {
    DecodePng = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
enum GhosttyKittyGraphicsData {
    PlacementIterator = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
enum GhosttyKittyGraphicsPlacementData {
    ImageId = 1,
    PlacementId = 2,
    IsVirtual = 3,
    XOffset = 4,
    YOffset = 5,
    Z = 12,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
enum GhosttyKittyGraphicsImageData {
    Width = 3,
    Height = 4,
    Format = 5,
    Compression = 6,
    DataPtr = 7,
    DataLen = 8,
    Generation = 9,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyKittyImageFormat {
    Rgb = 0,
    Rgba = 1,
    Png = 2,
    GrayAlpha = 3,
    Gray = 4,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GhosttyKittyImageCompression {
    None = 0,
    ZlibDeflate = 1,
}

/// `GHOSTTY_RENDER_STATE_DATA_*` keys for `ghostty_render_state_get`.
/// `RowIterator` (4) binds an existing row-iterator handle to this state.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyRenderStateData {
    Invalid = 0,
    Cols = 1,
    Rows = 2,
    Dirty = 3,
    RowIterator = 4,
    ColorBackground = 5,
    ColorForeground = 6,
    ColorCursor = 7,
    ColorCursorHasValue = 8,
    ColorPalette = 9,
    CursorVisualStyle = 10,
    CursorVisible = 11,
    CursorBlinking = 12,
    CursorPasswordInput = 13,
    CursorViewportHasValue = 14,
    CursorViewportX = 15,
    CursorViewportY = 16,
    CursorViewportWideTail = 17,
    Cursor = 18,
    Colors = 19,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyRenderStateDirty {
    False = 0,
    Partial = 1,
    Full = 2,
}

/// `GHOSTTY_RENDER_STATE_ROW_DATA_*` keys for `ghostty_render_state_row_get`.
/// `Cells` (3) binds a cells iterator to the current row.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyRenderStateRowData {
    Invalid = 0,
    Dirty = 1,
    Raw = 2,
    Cells = 3,
    Selection = 4,
    CellsRaw = 5,
}

/// `GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_*` keys.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyRenderStateRowCellsData {
    Invalid = 0,
    Raw = 1,
    Style = 2,
    GraphemesLen = 3,
    GraphemesBuf = 4,
    BgColor = 5,
    FgColor = 6,
    Selected = 7,
    HasStyling = 8,
    GraphemesUtf8 = 9,
}

/// `GHOSTTY_CELL_DATA_*` keys for `ghostty_cell_get`. Integer values
/// per `include/ghostty/vt/screen.h` at the pinned revision — the RAW
/// we get from row_cells is an **opaque `GhosttyCell` u64 handle**, not
/// a packed codepoint. Reading the codepoint requires this accessor.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub enum GhosttyCellData {
    Invalid = 0,
    Codepoint = 1,
    ContentTag = 2,
    Wide = 3,
    HasText = 4,
    HasStyling = 5,
    StyleId = 6,
    HasHyperlink = 7,
    Protected = 8,
    SemanticContent = 9,
    ColorPalette = 10,
    ColorRgb = 11,
}

/// Opaque cell snapshot returned by `row_cells_get(RAW, ...)`.
/// `typedef uint64_t GhosttyCell;` upstream.
pub type GhosttyCell = u64;

/// Packed 16-bit terminal mode — see `include/ghostty/vt/modes.h`.
/// Bits 0–14 hold the numeric mode value; bit 15 is the ANSI flag
/// (1 = ANSI, 0 = DEC private). Constructed via [`ghostty_mode`].
pub type GhosttyMode = u16;

/// Frozen-layout payload used with terminal mode get/set operations.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyTerminalModeConfig {
    pub mode: GhosttyMode,
    pub value: bool,
}

const _: [(); 4] = [(); std::mem::size_of::<GhosttyTerminalModeConfig>()];

/// Pack a mode value + ANSI flag into a [`GhosttyMode`]. Mirrors the
/// inline `ghostty_mode_new` helper the C header ships.
#[inline]
pub const fn ghostty_mode(value: u16, ansi: bool) -> GhosttyMode {
    (value & 0x7FFF) | ((ansi as u16) << 15)
}

// Pre-packed DEC private modes we care about on Windows. Keep the
// numeric values synced with `modes.h`.
pub const MODE_NORMAL_MOUSE: GhosttyMode = ghostty_mode(1000, false);
pub const MODE_BUTTON_MOUSE: GhosttyMode = ghostty_mode(1002, false);
pub const MODE_ANY_MOUSE: GhosttyMode = ghostty_mode(1003, false);
pub const MODE_X10_MOUSE: GhosttyMode = ghostty_mode(9, false);
pub const MODE_SGR_MOUSE: GhosttyMode = ghostty_mode(1006, false);
pub const MODE_ALT_SCROLL: GhosttyMode = ghostty_mode(1007, false);
/// DEC private mode 2004 — bracketed paste. Apps that set this want
/// pasted text wrapped in `ESC[200~ … ESC[201~` so the line editor can
/// distinguish typed-from-pasted input (e.g. to bypass auto-indent).
pub const MODE_BRACKETED_PASTE: GhosttyMode = ghostty_mode(2004, false);
/// DEC private mode 1 — DECCKM (cursor key mode). When set, arrow
/// keys send `ESC O [ABCD]` (application mode) instead of the default
/// `ESC [ [ABCD]` (cursor mode). Interactive readers like readline
/// and vim set this to distinguish their keymap lookup.
pub const MODE_DECCKM: GhosttyMode = ghostty_mode(1, false);

const GHOSTTY_DA_CONFORMANCE_LEVEL_2: u16 = 62;
const GHOSTTY_DA_FEATURE_SELECTIVE_ERASE: u16 = 6;
const GHOSTTY_DA_FEATURE_WINDOWING: u16 = 18;
const GHOSTTY_DA_FEATURE_ANSI_COLOR: u16 = 22;
const GHOSTTY_DA_FEATURE_RECTANGULAR_EDITING: u16 = 28;
const GHOSTTY_DA_FEATURE_CLIPBOARD: u16 = 52;
const GHOSTTY_DA_DEVICE_TYPE_VT220: u16 = 1;

/// `GhosttyColorRgb` — R,G,B bytes per upstream `color.h`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyRenderStateCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: c_int,
}

impl Default for GhosttyRenderStateCursor {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            viewport_has_value: false,
            viewport_x: 0,
            viewport_y: 0,
            wide_tail: false,
            visible: false,
            blinking: false,
            password_input: false,
            visual_style: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyRenderStateRowSelection {
    pub size: usize,
    pub start_x: u16,
    pub end_x: u16,
}

impl Default for GhosttyRenderStateRowSelection {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            start_x: 0,
            end_x: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyRenderStateColors {
    pub size: usize,
    pub background: GhosttyColorRgb,
    pub foreground: GhosttyColorRgb,
    pub cursor: GhosttyColorRgb,
    pub cursor_has_value: bool,
    pub palette: [GhosttyColorRgb; 256],
}

impl Default for GhosttyRenderStateColors {
    fn default() -> Self {
        Self {
            size: std::mem::size_of::<Self>(),
            background: GhosttyColorRgb::default(),
            foreground: GhosttyColorRgb::default(),
            cursor: GhosttyColorRgb::default(),
            cursor_has_value: false,
            palette: [GhosttyColorRgb::default(); 256],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyTerminalDesktopNotification {
    pub size: usize,
    pub title: GhosttyString,
    pub body: GhosttyString,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalProgressReport {
    pub size: usize,
    pub state: i32,
    pub progress: i8,
}

const GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_APC: i32 = 0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalUnknownStringSequence {
    pub truncated: bool,
    pub content: GhosttyString,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub union GhosttyTerminalUnknownSequenceValue {
    pub apc: GhosttyTerminalUnknownStringSequence,
    pub _padding: [u64; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyTerminalUnknownSequence {
    pub tag: i32,
    pub value: GhosttyTerminalUnknownSequenceValue,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyWriter {
    pub write: Option<unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool>,
    pub userdata: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyMimeReader {
    pub read: Option<unsafe extern "C" fn(*mut c_void, GhosttyString, GhosttyWriter) -> bool>,
    pub userdata: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyClipboardLocation {
    Standard = 0,
    Selection = 1,
    Primary = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyPasteSource {
    Clipboard = 0,
    Text = 1,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyPaste {
    pub size: usize,
    pub location: GhosttyClipboardLocation,
    pub source: GhosttyPasteSource,
    pub mimes: *const GhosttyString,
    pub mimes_len: usize,
    pub reader: GhosttyMimeReader,
    pub allow_unsafe: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyClipboardContent {
    pub mime: GhosttyString,
    pub data: GhosttyString,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyClipboardWriteResult {
    Success = 0,
    Denied = 1,
    Unsupported = 2,
    Busy = 3,
    InvalidData = 4,
    IoError = 5,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyClipboardWriteReply {
    pub size: usize,
    pub result: GhosttyClipboardWriteResult,
    pub remember: bool,
}

#[repr(C)]
pub struct GhosttyClipboardWrite {
    pub size: usize,
    pub location: GhosttyClipboardLocation,
    pub contents: *const GhosttyClipboardContent,
    pub contents_len: usize,
    pub name: GhosttyString,
    pub granted: bool,
    pub can_remember: bool,
    pub ctx: *const c_void,
    pub reply: Option<
        unsafe extern "C" fn(*const GhosttyClipboardWrite, *const GhosttyClipboardWriteReply),
    >,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyClipboardReadResult {
    Success = 0,
    Denied = 1,
    Unsupported = 2,
    Busy = 3,
    IoError = 4,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct GhosttyClipboardReadReply {
    pub size: usize,
    pub result: GhosttyClipboardReadResult,
    pub contents: *const GhosttyClipboardContent,
    pub contents_len: usize,
    pub available: *const GhosttyString,
    pub available_len: usize,
    pub remember: bool,
}

#[repr(C)]
pub struct GhosttyClipboardRead {
    pub size: usize,
    pub location: GhosttyClipboardLocation,
    pub mimes: *const GhosttyString,
    pub mimes_len: usize,
    pub list: bool,
    pub name: GhosttyString,
    pub granted: bool,
    pub can_remember: bool,
    pub ctx: *const c_void,
    pub reply:
        Option<unsafe extern "C" fn(*const GhosttyClipboardRead, *const GhosttyClipboardReadReply)>,
}

/// RGBA buffer returned by `GhosttySysDecodePngFn`. The pixel allocation is
/// transferred to libghostty and must come from the allocator supplied to the
/// callback, which is not necessarily Rust's global allocator on Windows.
#[repr(C)]
struct GhosttySysImage {
    width: u32,
    height: u32,
    data: *mut u8,
    data_len: usize,
}

#[repr(C)]
#[derive(Default)]
struct GhosttyKittyGraphicsPlacementRenderInfo {
    size: usize,
    pixel_width: u32,
    pixel_height: u32,
    grid_cols: u32,
    grid_rows: u32,
    viewport_col: i32,
    viewport_row: i32,
    viewport_visible: bool,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
}

type GhosttySysDecodePngFn = unsafe extern "C" fn(
    userdata: *mut c_void,
    allocator: *const GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut GhosttySysImage,
) -> bool;

const _: [(); 16] = [(); std::mem::size_of::<GhosttyWriter>()];
const _: [(); 16] = [(); std::mem::size_of::<GhosttyMimeReader>()];
const _: [(); 56] = [(); std::mem::size_of::<GhosttyPaste>()];
const _: [(); 32] = [(); std::mem::size_of::<GhosttyClipboardContent>()];
const _: [(); 16] = [(); std::mem::size_of::<GhosttyClipboardWriteReply>()];
const _: [(); 72] = [(); std::mem::size_of::<GhosttyClipboardWrite>()];
const _: [(); 56] = [(); std::mem::size_of::<GhosttyClipboardReadReply>()];
const _: [(); 80] = [(); std::mem::size_of::<GhosttyClipboardRead>()];
const _: [(); 40] = [(); std::mem::size_of::<GhosttyTerminalDesktopNotification>()];
const _: [(); 24] = [(); std::mem::size_of::<GhosttySysImage>()];
const _: [(); 56] = [(); std::mem::size_of::<GhosttyKittyGraphicsPlacementRenderInfo>()];

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhosttyColorScheme {
    Light = 0,
    Dark = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttySizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyDeviceAttributesPrimary {
    pub conformance_level: u16,
    pub features: [u16; 64],
    pub num_features: usize,
}

impl Default for GhosttyDeviceAttributesPrimary {
    fn default() -> Self {
        Self {
            conformance_level: 0,
            features: [0; 64],
            num_features: 0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyDeviceAttributesSecondary {
    pub device_type: u16,
    pub firmware_version: u16,
    pub rom_cartridge: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyDeviceAttributesTertiary {
    pub unit_id: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyDeviceAttributes {
    pub primary: GhosttyDeviceAttributesPrimary,
    pub secondary: GhosttyDeviceAttributesSecondary,
    pub tertiary: GhosttyDeviceAttributesTertiary,
}

// ── Style (`style.h`) ──────────────────────────────────────────────────
//
// `row_cells_get(STYLE, out)` writes a `GhosttyStyle` by value. Caller
// sets `.size = sizeof(GhosttyStyle)` first so the library knows how
// many bytes it may write (versioned-struct forward-compat pattern).

/// `GhosttyStyleColor` — tagged color (None | Palette | Rgb). The
/// union is laid out as `u64` here because we only care about the tag
/// for now; per-cell fg/bg also come in via the cheaper FG_COLOR /
/// BG_COLOR accessor on row_cells, so we don't need to decode the
/// union value at read-cell time.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyStyleColor {
    pub tag: u32,
    pub _pad: u32,
    pub value: u64,
}

/// `GhosttyStyle` — SGR-derived attributes for the current cell.
/// Layout matches `include/ghostty/vt/style.h` at GHOSTTY_REV; the
/// `size` prefix lets upstream add trailing fields without breaking
/// older callers.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyStyle {
    pub size: usize,
    pub fg_color: GhosttyStyleColor,
    pub bg_color: GhosttyStyleColor,
    pub underline_color: GhosttyStyleColor,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub underline: c_int,
}

impl GhosttyStyle {
    fn new() -> Self {
        Self {
            size: std::mem::size_of::<GhosttyStyle>(),
            fg_color: GhosttyStyleColor::default(),
            bg_color: GhosttyStyleColor::default(),
            underline_color: GhosttyStyleColor::default(),
            bold: false,
            italic: false,
            faint: false,
            blink: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            underline: 0,
        }
    }
}

// Attr bits packed into `Cell.attrs`. Kept in sync with the HLSL
// pixel shader's interpretation (bit 0 = bold, 1 = italic, 2 =
// underline, 3 = strike, 4 = inverse).
pub const ATTR_BOLD: u8 = 1 << 0;
pub const ATTR_ITALIC: u8 = 1 << 1;
pub const ATTR_UNDERLINE: u8 = 1 << 2;
pub const ATTR_STRIKE: u8 = 1 << 3;
pub const ATTR_INVERSE: u8 = 1 << 4;

// ── Raw FFI ────────────────────────────────────────────────────────────

unsafe extern "C" {
    // ABI manifest (`types.h`).
    pub fn ghostty_type_json() -> *const c_char;

    // Allocator + process-global optional services (`allocator.h`, `sys.h`).
    fn ghostty_alloc(allocator: *const GhosttyAllocator, len: usize) -> *mut u8;
    fn ghostty_free(allocator: *const GhosttyAllocator, ptr: *mut u8, len: usize);
    fn ghostty_sys_set(option: GhosttySysOption, value: *const c_void) -> GhosttyResult;

    // Terminal (`terminal.h`)
    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        out_terminal: *mut GhosttyTerminal,
        cols: u16,
        rows: u16,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GhosttyResult;
    /// `value` semantics depend on `option`:
    ///   - `Userdata`: opaque host pointer stored on the terminal
    ///   - `WritePty`: host callback function pointer
    ///   - color knobs (FG/BG/CURSOR): pointer to a single `GhosttyColorRgb`
    ///   - palette: pointer to `GhosttyColorRgb[256]`
    /// Passing `value = NULL` clears the override and restores the
    /// built-in defaults where supported.
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: GhosttyTerminalOption,
        value: *const c_void,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_scroll_viewport(
        terminal: GhosttyTerminal,
        behavior: GhosttyTerminalScrollViewport,
    );
    pub fn ghostty_terminal_get(
        terminal: GhosttyTerminal,
        key: GhosttyTerminalData,
        out: *mut c_void,
    ) -> GhosttyResult;
    fn ghostty_terminal_grid_ref(
        terminal: GhosttyTerminal,
        point: GhosttyPoint,
        out_ref: *mut GhosttyGridRef,
    ) -> GhosttyResult;
    pub fn ghostty_terminal_paste(
        terminal: GhosttyTerminal,
        paste: *const GhosttyPaste,
        out_written: *mut bool,
    ) -> GhosttyResult;

    // Selection gesture (`selection.h`). Handles and reusable events live
    // under the same mutex as the terminal because the gesture owns tracked
    // grid references into that terminal.
    fn ghostty_selection_gesture_new(
        allocator: *const GhosttyAllocator,
        out_gesture: *mut GhosttySelectionGesture,
    ) -> GhosttyResult;
    fn ghostty_selection_gesture_free(gesture: GhosttySelectionGesture, terminal: GhosttyTerminal);
    fn ghostty_selection_gesture_reset(gesture: GhosttySelectionGesture, terminal: GhosttyTerminal);
    fn ghostty_selection_gesture_get(
        gesture: GhosttySelectionGesture,
        terminal: GhosttyTerminal,
        data: GhosttySelectionGestureData,
        value: *mut c_void,
    ) -> GhosttyResult;
    fn ghostty_selection_gesture_event_new(
        allocator: *const GhosttyAllocator,
        out_event: *mut GhosttySelectionGestureEvent,
        event_type: GhosttySelectionGestureEventType,
    ) -> GhosttyResult;
    fn ghostty_selection_gesture_event_free(event: GhosttySelectionGestureEvent);
    fn ghostty_selection_gesture_event_set(
        event: GhosttySelectionGestureEvent,
        option: GhosttySelectionGestureEventOption,
        value: *const c_void,
    ) -> GhosttyResult;
    fn ghostty_selection_gesture_event(
        gesture: GhosttySelectionGesture,
        terminal: GhosttyTerminal,
        event: GhosttySelectionGestureEvent,
        out_selection: *mut GhosttySelection,
    ) -> GhosttyResult;
    fn ghostty_terminal_selection_equal(
        terminal: GhosttyTerminal,
        a: *const GhosttySelection,
        b: *const GhosttySelection,
        out_equal: *mut bool,
    ) -> GhosttyResult;
    fn ghostty_terminal_selection_format_alloc(
        terminal: GhosttyTerminal,
        allocator: *const GhosttyAllocator,
        options: GhosttyTerminalSelectionFormatOptions,
        out_ptr: *mut *mut u8,
        out_len: *mut usize,
    ) -> GhosttyResult;

    // Key encoder (`key/{encoder,event}.h`). The encoder and reusable
    // event live under `VtInner`'s mutex with the terminal whose mode
    // state they snapshot.
    pub fn ghostty_key_encoder_new(
        allocator: *const GhosttyAllocator,
        out_encoder: *mut GhosttyKeyEncoder,
    ) -> GhosttyResult;
    pub fn ghostty_key_encoder_free(encoder: GhosttyKeyEncoder);
    pub fn ghostty_key_encoder_setopt_from_terminal(
        encoder: GhosttyKeyEncoder,
        terminal: GhosttyTerminal,
    );
    pub fn ghostty_key_encoder_encode(
        encoder: GhosttyKeyEncoder,
        event: GhosttyKeyEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    pub fn ghostty_key_event_new(
        allocator: *const GhosttyAllocator,
        out_event: *mut GhosttyKeyEvent,
    ) -> GhosttyResult;
    pub fn ghostty_key_event_free(event: GhosttyKeyEvent);
    pub fn ghostty_key_event_set_action(event: GhosttyKeyEvent, action: c_int);
    pub fn ghostty_key_event_set_key(event: GhosttyKeyEvent, key: c_int);
    pub fn ghostty_key_event_set_mods(event: GhosttyKeyEvent, mods: u16);
    pub fn ghostty_key_event_set_consumed_mods(event: GhosttyKeyEvent, mods: u16);
    pub fn ghostty_key_event_set_composing(event: GhosttyKeyEvent, composing: bool);
    pub fn ghostty_key_event_set_utf8(event: GhosttyKeyEvent, utf8: *const c_char, len: usize);
    pub fn ghostty_key_event_set_unshifted_codepoint(event: GhosttyKeyEvent, codepoint: u32);

    // Mouse input (`mouse/{encoder,event}.h`). The encoder and reusable event
    // share `VtInner`'s mutex with the terminal mode state they encode against.
    fn ghostty_mouse_encoder_new(
        allocator: *const GhosttyAllocator,
        out_encoder: *mut GhosttyMouseEncoder,
    ) -> GhosttyResult;
    fn ghostty_mouse_encoder_free(encoder: GhosttyMouseEncoder);
    fn ghostty_mouse_encoder_setopt(
        encoder: GhosttyMouseEncoder,
        option: GhosttyMouseEncoderOption,
        value: *const c_void,
    );
    fn ghostty_mouse_encoder_setopt_from_terminal(
        encoder: GhosttyMouseEncoder,
        terminal: GhosttyTerminal,
    );
    fn ghostty_mouse_encoder_encode(
        encoder: GhosttyMouseEncoder,
        event: GhosttyMouseEvent,
        out_buf: *mut c_char,
        out_buf_size: usize,
        out_len: *mut usize,
    ) -> GhosttyResult;
    fn ghostty_mouse_event_new(
        allocator: *const GhosttyAllocator,
        out_event: *mut GhosttyMouseEvent,
    ) -> GhosttyResult;
    fn ghostty_mouse_event_free(event: GhosttyMouseEvent);
    fn ghostty_mouse_event_set_action(event: GhosttyMouseEvent, action: GhosttyMouseAction);
    fn ghostty_mouse_event_set_button(event: GhosttyMouseEvent, button: GhosttyMouseButton);
    fn ghostty_mouse_event_clear_button(event: GhosttyMouseEvent);
    fn ghostty_mouse_event_set_mods(event: GhosttyMouseEvent, mods: u16);
    fn ghostty_mouse_event_set_position(event: GhosttyMouseEvent, position: GhosttyMousePosition);

    // Kitty graphics storage (`kitty_graphics.h`). Handles are borrowed from
    // the terminal and remain valid only while its mutex is held.
    fn ghostty_kitty_graphics_get(
        graphics: GhosttyKittyGraphics,
        data: GhosttyKittyGraphicsData,
        out: *mut c_void,
    ) -> GhosttyResult;
    fn ghostty_kitty_graphics_image(
        graphics: GhosttyKittyGraphics,
        image_id: u32,
    ) -> GhosttyKittyGraphicsImage;
    fn ghostty_kitty_graphics_image_get(
        image: GhosttyKittyGraphicsImage,
        data: GhosttyKittyGraphicsImageData,
        out: *mut c_void,
    ) -> GhosttyResult;
    fn ghostty_kitty_graphics_image_get_multi(
        image: GhosttyKittyGraphicsImage,
        count: usize,
        keys: *const GhosttyKittyGraphicsImageData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GhosttyResult;
    fn ghostty_kitty_graphics_placement_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iterator: *mut GhosttyKittyGraphicsPlacementIterator,
    ) -> GhosttyResult;
    fn ghostty_kitty_graphics_placement_iterator_free(
        iterator: GhosttyKittyGraphicsPlacementIterator,
    );
    fn ghostty_kitty_graphics_placement_next(
        iterator: GhosttyKittyGraphicsPlacementIterator,
    ) -> bool;
    fn ghostty_kitty_graphics_placement_get_multi(
        iterator: GhosttyKittyGraphicsPlacementIterator,
        count: usize,
        keys: *const GhosttyKittyGraphicsPlacementData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GhosttyResult;
    fn ghostty_kitty_graphics_placement_render_info(
        iterator: GhosttyKittyGraphicsPlacementIterator,
        image: GhosttyKittyGraphicsImage,
        terminal: GhosttyTerminal,
        out_info: *mut GhosttyKittyGraphicsPlacementRenderInfo,
    ) -> GhosttyResult;

    // Render state (`render.h`)
    pub fn ghostty_render_state_new(
        allocator: *const GhosttyAllocator,
        out_state: *mut GhosttyRenderState,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_free(state: GhosttyRenderState);
    pub fn ghostty_render_state_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_begin_update(
        state: GhosttyRenderState,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_end_update(state: GhosttyRenderState) -> GhosttyResult;
    pub fn ghostty_render_state_clean(state: GhosttyRenderState) -> GhosttyResult;
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        key: GhosttyRenderStateData,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_get_multi(
        state: GhosttyRenderState,
        count: usize,
        keys: *const GhosttyRenderStateData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iter: *mut GhosttyRowIterator,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_free(iter: GhosttyRowIterator);
    /// Returns `bool` per upstream signature. Rust `bool` is 1 byte —
    /// matches MSVC/gcc/clang C99 `_Bool` layout.
    pub fn ghostty_render_state_row_iterator_next(iter: GhosttyRowIterator) -> bool;
    pub fn ghostty_render_state_row_iterator_next_dirty(
        iter: GhosttyRowIterator,
        out_y: *mut u16,
    ) -> bool;
    pub fn ghostty_render_state_row_get(
        iter: GhosttyRowIterator,
        key: GhosttyRenderStateRowData,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_get_multi(
        iter: GhosttyRowIterator,
        count: usize,
        keys: *const GhosttyRenderStateRowData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_row_cells_new(
        allocator: *const GhosttyAllocator,
        out_cells: *mut GhosttyRowCells,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_free(cells: GhosttyRowCells);
    pub fn ghostty_render_state_row_cells_next(cells: GhosttyRowCells) -> bool;
    pub fn ghostty_render_state_row_cells_get(
        cells: GhosttyRowCells,
        key: GhosttyRenderStateRowCellsData,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_cells_get_multi(
        cells: GhosttyRowCells,
        count: usize,
        keys: *const GhosttyRenderStateRowCellsData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GhosttyResult;

    // Cell accessor (`screen.h`). Decodes fields out of the opaque
    // `GhosttyCell` u64 we get from row_cells RAW.
    pub fn ghostty_cell_get(
        cell: GhosttyCell,
        key: GhosttyCellData,
        out: *mut c_void,
    ) -> GhosttyResult;
    pub fn ghostty_cell_get_multi(
        cell: GhosttyCell,
        count: usize,
        keys: *const GhosttyCellData,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GhosttyResult;
}

fn decode_png_rgba(data: &[u8]) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    if data.is_empty() || data.len() > KITTY_PNG_MAX_INPUT_BYTES {
        anyhow::bail!("PNG input exceeds the Kitty image limit");
    }

    let mut reader = ImageReader::with_format(IoCursor::new(data), ImageFormat::Png);
    let mut limits = Limits::default();
    limits.max_image_width = Some(KITTY_PNG_MAX_DIMENSION);
    limits.max_image_height = Some(KITTY_PNG_MAX_DIMENSION);
    limits.max_alloc = Some(KITTY_PNG_DECODER_MAX_ALLOC_BYTES);
    reader.limits(limits);
    let decoded = reader.decode()?;
    let width = decoded.width();
    let height = decoded.height();
    let rgba_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("PNG dimensions overflow the host address space"))?;
    if rgba_len > KITTY_IMAGE_STORAGE_LIMIT_BYTES as usize {
        anyhow::bail!("decoded PNG exceeds the Kitty image storage limit");
    }

    let rgba = decoded.into_rgba8().into_raw();
    if rgba.len() != rgba_len {
        anyhow::bail!("PNG decoder returned an inconsistent pixel buffer");
    }
    Ok((width, height, rgba))
}

unsafe extern "C" fn decode_png_callback(
    _userdata: *mut c_void,
    allocator: *const GhosttyAllocator,
    data: *const u8,
    data_len: usize,
    out: *mut GhosttySysImage,
) -> bool {
    if data.is_null() || out.is_null() {
        return false;
    }

    // No Rust panic may cross the C ABI boundary. Malformed or over-limit
    // terminal input is a normal protocol rejection, so all failures collapse
    // to false and libghostty emits the protocol-level error response.
    let decoded = std::panic::catch_unwind(|| {
        let bytes = unsafe { std::slice::from_raw_parts(data, data_len) };
        decode_png_rgba(bytes)
    });
    let Ok(Ok((width, height, rgba))) = decoded else {
        return false;
    };

    let allocation = unsafe { ghostty_alloc(allocator, rgba.len()) };
    if allocation.is_null() {
        return false;
    }
    unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), allocation, rgba.len()) };
    unsafe {
        out.write(GhosttySysImage {
            width,
            height,
            data: allocation,
            data_len: rgba.len(),
        });
    }
    true
}

fn install_ghostty_sys_services() -> anyhow::Result<()> {
    static RESULT: OnceLock<GhosttyResult> = OnceLock::new();
    let rc = *RESULT.get_or_init(|| {
        let decoder: GhosttySysDecodePngFn = decode_png_callback;
        unsafe {
            ghostty_sys_set(
                GhosttySysOption::DecodePng,
                decoder as *const () as *const c_void,
            )
        }
    });
    if rc != GHOSTTY_SUCCESS {
        anyhow::bail!("ghostty_sys_set(DECODE_PNG) failed: rc={rc}");
    }
    Ok(())
}

// ── Theme ──────────────────────────────────────────────────────────────

/// Default colors handed to libghostty via `ghostty_terminal_set`.
///
/// libghostty owns the SGR-color resolution: it looks up palette
/// indices, applies bold/bright remaps, and falls back to the default
/// fg/bg for unstyled cells. Pushing the user's theme in here means
/// `read_cell` doesn't need any special-casing — every cell's fg/bg
/// already arrives themed.
#[derive(Debug, Clone)]
pub struct ThemeColors {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    /// Full 256-entry palette. Indices 0–15 are the user's ANSI
    /// theme; 16–231 form the standard xterm 6×6×6 cube; 232–255 form
    /// the 24-step grayscale ramp.
    pub palette: [[u8; 3]; 256],
}

impl ThemeColors {
    /// Build a 256-color palette from a 16-entry ANSI base.
    pub fn from_ansi16(fg: [u8; 3], bg: [u8; 3], ansi16: [[u8; 3]; 16]) -> Self {
        let mut palette = [[0u8; 3]; 256];
        for i in 0..16 {
            palette[i] = ansi16[i];
        }
        let step = |x: u8| -> u8 { if x == 0 { 0 } else { 55 + 40 * x } };
        for i in 16..232 {
            let idx = (i - 16) as u8;
            let r = idx / 36;
            let g = (idx / 6) % 6;
            let b = idx % 6;
            palette[i] = [step(r), step(g), step(b)];
        }
        for i in 232..256 {
            let v = 8u8.saturating_add(((i - 232) as u8).saturating_mul(10));
            palette[i] = [v, v, v];
        }
        Self { fg, bg, palette }
    }
}

unsafe fn apply_theme_to_terminal(terminal: GhosttyTerminal, theme: &ThemeColors) {
    let fg = GhosttyColorRgb {
        r: theme.fg[0],
        g: theme.fg[1],
        b: theme.fg[2],
    };
    let bg = GhosttyColorRgb {
        r: theme.bg[0],
        g: theme.bg[1],
        b: theme.bg[2],
    };
    let palette: [GhosttyColorRgb; 256] = std::array::from_fn(|i| GhosttyColorRgb {
        r: theme.palette[i][0],
        g: theme.palette[i][1],
        b: theme.palette[i][2],
    });
    let check = |rc: GhosttyResult, what: &'static str| {
        if rc != 0 {
            log::warn!("ghostty_terminal_set({what}) failed: rc={rc}");
        }
    };
    unsafe {
        check(
            ghostty_terminal_set(
                terminal,
                GhosttyTerminalOption::ColorForeground,
                &fg as *const _ as *const c_void,
            ),
            "ColorForeground",
        );
        check(
            ghostty_terminal_set(
                terminal,
                GhosttyTerminalOption::ColorBackground,
                &bg as *const _ as *const c_void,
            ),
            "ColorBackground",
        );
        check(
            ghostty_terminal_set(
                terminal,
                GhosttyTerminalOption::ColorPalette,
                palette.as_ptr() as *const c_void,
            ),
            "ColorPalette",
        );
    }
}

// ── Snapshot (renderer's view) ─────────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cell {
    pub codepoint: u32,
    /// Foreground RGBA (0xRRGGBBAA).
    pub fg: u32,
    /// Background RGBA.
    pub bg: u32,
    pub attrs: u8,
    pub _pad: [u8; 3],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

/// Pointer location for a selection gesture. Grid coordinates identify the
/// viewport cell while surface coordinates preserve within-cell thresholds and
/// allow drag autoscroll to detect positions outside the viewport.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionPoint {
    pub col: u16,
    pub row: u16,
    pub surface_x_px: f64,
    pub surface_y_px: f64,
}

/// Physical geometry used by Ghostty's selection drag state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionGeometry {
    pub columns: u32,
    pub cell_width_px: u32,
    pub padding_left_px: u32,
    pub screen_height_px: u32,
}

impl SelectionGeometry {
    fn to_ffi(self) -> anyhow::Result<GhosttySelectionGestureGeometry> {
        if self.columns == 0 || self.cell_width_px == 0 || self.screen_height_px == 0 {
            anyhow::bail!("selection geometry dimensions must be non-zero");
        }
        Ok(GhosttySelectionGestureGeometry {
            columns: self.columns,
            cell_width: self.cell_width_px,
            padding_left: self.padding_left_px,
            screen_height: self.screen_height_px,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SelectionAutoscroll {
    #[default]
    None,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SelectionAutoscrollUpdate {
    pub direction: SelectionAutoscroll,
    pub changed: bool,
}

#[derive(Debug)]
pub struct KittyImage {
    pub id: u32,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    /// Straight-alpha RGBA8 pixels, normalized once when the upstream image
    /// generation changes and shared by every placement and renderer frame.
    pub rgba: Arc<[u8]>,
}

#[derive(Debug, Clone)]
pub struct KittyPlacement {
    pub image: Arc<KittyImage>,
    pub placement_id: u32,
    pub z: i32,
    pub viewport_col: i32,
    pub viewport_row: i32,
    pub cell_x_offset: u32,
    pub cell_y_offset: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ScreenSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    pub selection_ranges: Vec<Option<SelectionRange>>,
    pub kitty_placements: Arc<[KittyPlacement]>,
    pub dirty_rows: Vec<u16>,
    pub cursor: Cursor,
    pub alternate_screen: bool,
    pub scrollbar: Option<GhosttyScrollbar>,
    pub title: Option<String>,
    pub generation: u64,
}

/// Inclusive visible-grid column range selected on one snapshot row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    pub start: u16,
    pub end: u16,
}

impl SelectionRange {
    pub fn contains(self, col: u16) -> bool {
        col >= self.start && col <= self.end
    }
}

struct SnapshotMetadata {
    cols: u16,
    rows: u16,
    kitty_placements: Arc<[KittyPlacement]>,
    dirty_rows: Vec<u16>,
    cursor: Cursor,
    alternate_screen: bool,
    scrollbar: Option<GhosttyScrollbar>,
    generation: u64,
}

struct SnapshotEpoch {
    fallback_cols: u16,
    fallback_rows: u16,
    kitty_placements: Arc<[KittyPlacement]>,
    alternate_screen: bool,
    scrollbar: Option<GhosttyScrollbar>,
    generation: u64,
    required_full_snapshot_generation: u64,
}

impl SnapshotMetadata {
    fn snapshot(
        &self,
        cells: &[Cell],
        selection_ranges: &[Option<SelectionRange>],
    ) -> ScreenSnapshot {
        ScreenSnapshot {
            cols: self.cols,
            rows: self.rows,
            cells: cells.to_vec(),
            selection_ranges: selection_ranges.to_vec(),
            kitty_placements: self.kitty_placements.clone(),
            dirty_rows: self.dirty_rows.clone(),
            cursor: self.cursor,
            alternate_screen: self.alternate_screen,
            scrollbar: self.scrollbar,
            title: None,
            generation: self.generation,
        }
    }
}

// ── Safe wrapper ───────────────────────────────────────────────────────

pub struct VtScreen {
    inner: Arc<Mutex<VtInner>>,
    render: Mutex<VtRenderState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PtyWriteClass {
    Regular,
    ReservedControl,
}

pub type PtyWriteCallback =
    Arc<dyn Fn(&[u8], PtyWriteClass) -> std::io::Result<()> + Send + Sync + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtKeyAction {
    Release,
    Press,
    Repeat,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VtKeyModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub platform: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct VtKeyEvent<'a> {
    /// GPUI's normalized logical key name. Only unambiguous functional
    /// names are promoted to a physical Ghostty key; printable keys stay
    /// unidentified because GPUI doesn't expose a scan code/W3C code.
    pub key: &'a str,
    /// Layout-produced UTF-8 before Ctrl/Alt transformations.
    pub text: &'a str,
    pub unshifted_codepoint: Option<char>,
    pub action: VtKeyAction,
    pub modifiers: VtKeyModifiers,
    pub consumed_modifiers: VtKeyModifiers,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VtKeyOutcome {
    pub output_accepted: bool,
    pub report_releases: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtMouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtMouseButton {
    Left,
    Middle,
    Right,
    Button4,
    Button5,
    Button6,
    Button7,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VtMouseModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// Normalized mouse input in physical surface pixels. Ghostty converts this
/// position to cells or terminal-space pixels according to the active format.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VtMouseEvent {
    pub action: VtMouseAction,
    pub button: Option<VtMouseButton>,
    pub modifiers: VtMouseModifiers,
    pub surface_x_px: f32,
    pub surface_y_px: f32,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct VtMouseOutcome {
    /// True only when the active protocol encoded bytes and the PTY callback
    /// accepted them. A captured terminal event may legitimately produce none.
    pub output_written: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtPasteResult {
    Empty,
    Accepted,
    RequiresConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtPasteSource {
    Clipboard,
    Text,
}

struct VtPasteReader<'a> {
    text: &'a [u8],
    served: bool,
}

enum PendingPasteWrite {
    Buffer(Vec<u8>),
    TooLarge,
}

impl PendingPasteWrite {
    fn append(&mut self, bytes: &[u8]) {
        let Self::Buffer(buffer) = self else {
            return;
        };
        let Some(total) = buffer.len().checked_add(bytes.len()) else {
            *self = Self::TooLarge;
            return;
        };
        if total > TERMINAL_PASTE_OUTPUT_LIMIT_BYTES {
            *self = Self::TooLarge;
            return;
        }
        buffer.extend_from_slice(bytes);
    }
}

struct VtCallbackState {
    write_pty: Option<PtyWriteCallback>,
    /// Serializes host callback entry. Encoded input acquires this while it
    /// still owns the VT mutex, then releases the VT mutex before invoking the
    /// host; a parser reply therefore cannot overtake that input.
    write_order: Arc<Mutex<()>>,
    enquiry_response: Box<[u8]>,
    /// Last clipboard text the user explicitly chose to paste. Kitty grants
    /// are one-time capabilities, but multiple outstanding grants read this
    /// shared current value. Unsolicited OSC reads remain denied.
    clipboard_text: Mutex<Option<Arc<str>>>,
    /// A terminal-generated reply or release cannot be retried through the C
    /// callback ABI. If its reserved queue capacity is exhausted, fail the
    /// session explicitly rather than continuing with desynchronized state.
    write_failed: Arc<AtomicBool>,
    clipboard_write_enabled: AtomicBool,
    clipboard_write_policy: Arc<ClipboardWritePolicy>,
    pending_clipboard_write: Mutex<Option<String>>,
    /// Active only during `ghostty_terminal_paste`. The C callback cannot
    /// return an I/O result, so buffer the complete logical paste and perform
    /// one fallible host enqueue after libghostty returns.
    pending_paste_write: Mutex<Option<PendingPasteWrite>>,
    rows: AtomicU16,
    cols: AtomicU16,
    cell_width: AtomicU32,
    cell_height: AtomicU32,
    dark_mode: AtomicBool,
    device_attributes: GhosttyDeviceAttributes,
    metadata_dirty: AtomicU8,
    bell_pending: AtomicBool,
    desktop_notification_policy: Arc<DesktopNotificationPolicy>,
    progress_epoch: Instant,
    progress: AtomicU64,
    unknown_sequence_log_count: AtomicU8,
}

struct MouseEncoderState {
    encoder: GhosttyMouseEncoder,
    event: GhosttyMouseEvent,
    geometry: GhosttyMouseEncoderSize,
}

impl MouseEncoderState {
    fn new(terminal: GhosttyTerminal, cols: u16, rows: u16) -> anyhow::Result<Self> {
        let mut encoder = std::ptr::null_mut();
        let rc = unsafe { ghostty_mouse_encoder_new(std::ptr::null(), &mut encoder) };
        if rc != GHOSTTY_SUCCESS || encoder.is_null() {
            anyhow::bail!("ghostty_mouse_encoder_new failed: rc={rc}");
        }

        let mut event = std::ptr::null_mut();
        let rc = unsafe { ghostty_mouse_event_new(std::ptr::null(), &mut event) };
        if rc != GHOSTTY_SUCCESS || event.is_null() {
            unsafe { ghostty_mouse_encoder_free(encoder) };
            anyhow::bail!("ghostty_mouse_event_new failed: rc={rc}");
        }

        let geometry = GhosttyMouseEncoderSize::new(u32::from(cols), u32::from(rows), 1, 1);
        let track_last_cell = true;
        unsafe {
            ghostty_mouse_encoder_setopt(
                encoder,
                GhosttyMouseEncoderOption::Size,
                &geometry as *const _ as *const c_void,
            );
            ghostty_mouse_encoder_setopt(
                encoder,
                GhosttyMouseEncoderOption::TrackLastCell,
                &track_last_cell as *const _ as *const c_void,
            );
            ghostty_mouse_encoder_setopt_from_terminal(encoder, terminal);
        }

        Ok(Self {
            encoder,
            event,
            geometry,
        })
    }

    unsafe fn free(&mut self) {
        if !self.event.is_null() {
            unsafe { ghostty_mouse_event_free(self.event) };
            self.event = std::ptr::null_mut();
        }
        if !self.encoder.is_null() {
            unsafe { ghostty_mouse_encoder_free(self.encoder) };
            self.encoder = std::ptr::null_mut();
        }
    }
}

struct VtInner {
    terminal: GhosttyTerminal,
    key_encoder: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
    mouse: MouseEncoderState,
    selection_gesture: Option<SelectionGestureState>,
    kitty_placement_iter: GhosttyKittyGraphicsPlacementIterator,
    kitty_image_cache: HashMap<u32, Arc<KittyImage>>,
    kitty_placements: Arc<[KittyPlacement]>,
    kitty_snapshot_generation: u64,
    callback_state: Box<VtCallbackState>,
    title: Option<String>,
    title_reported: bool,
    current_dir: Option<String>,
    cols: u16,
    rows: u16,
    generation: u64,
    output_generation: u64,
    required_full_snapshot_generation: u64,
}

struct SelectionGestureState {
    gesture: GhosttySelectionGesture,
    press: GhosttySelectionGestureEvent,
    release: GhosttySelectionGestureEvent,
    drag: GhosttySelectionGestureEvent,
    autoscroll_tick: GhosttySelectionGestureEvent,
    clock: Instant,
}

impl SelectionGestureState {
    fn new() -> anyhow::Result<Self> {
        let mut state = Self {
            gesture: std::ptr::null_mut(),
            press: std::ptr::null_mut(),
            release: std::ptr::null_mut(),
            drag: std::ptr::null_mut(),
            autoscroll_tick: std::ptr::null_mut(),
            clock: Instant::now(),
        };

        let rc = unsafe { ghostty_selection_gesture_new(std::ptr::null(), &mut state.gesture) };
        if rc != GHOSTTY_SUCCESS || state.gesture.is_null() {
            if !state.gesture.is_null() {
                unsafe { state.free(std::ptr::null_mut()) };
            }
            anyhow::bail!("ghostty_selection_gesture_new failed: rc={rc}");
        }

        macro_rules! new_event {
            ($field:ident, $event_type:expr, $label:literal) => {{
                let rc = unsafe {
                    ghostty_selection_gesture_event_new(
                        std::ptr::null(),
                        &mut state.$field,
                        $event_type,
                    )
                };
                if rc != GHOSTTY_SUCCESS || state.$field.is_null() {
                    unsafe { state.free(std::ptr::null_mut()) };
                    anyhow::bail!(
                        "ghostty_selection_gesture_event_new({}) failed: rc={rc}",
                        $label
                    );
                }
            }};
        }
        new_event!(press, GhosttySelectionGestureEventType::Press, "PRESS");
        new_event!(
            release,
            GhosttySelectionGestureEventType::Release,
            "RELEASE"
        );
        new_event!(drag, GhosttySelectionGestureEventType::Drag, "DRAG");
        new_event!(
            autoscroll_tick,
            GhosttySelectionGestureEventType::AutoscrollTick,
            "AUTOSCROLL_TICK"
        );

        Ok(state)
    }

    unsafe fn free(&mut self, terminal: GhosttyTerminal) {
        for event in [
            &mut self.autoscroll_tick,
            &mut self.drag,
            &mut self.release,
            &mut self.press,
        ] {
            if !event.is_null() {
                unsafe { ghostty_selection_gesture_event_free(*event) };
                *event = std::ptr::null_mut();
            }
        }
        if !self.gesture.is_null() {
            unsafe { ghostty_selection_gesture_free(self.gesture, terminal) };
            self.gesture = std::ptr::null_mut();
        }
    }

    fn time_ns(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

struct VtRenderState {
    render_state: GhosttyRenderState,
    row_iter: GhosttyRowIterator,
    row_cells: GhosttyRowCells,
    applied_full_snapshot_generation: u64,
    force_full_snapshot: bool,
    scratch_cols: u16,
    scratch_rows: u16,
    scratch: Vec<Cell>,
    selection_ranges: Vec<Option<SelectionRange>>,
    last_cursor: Cursor,
    snapshot_metadata: Option<SnapshotMetadata>,
}

impl VtRenderState {
    fn ensure_render_state(&mut self) -> bool {
        if !self.render_state.is_null() {
            return true;
        }

        let mut render_state: GhosttyRenderState = std::ptr::null_mut();
        let rc = unsafe { ghostty_render_state_new(std::ptr::null(), &mut render_state) };
        if rc != GHOSTTY_SUCCESS || render_state.is_null() {
            log::warn!("ghostty_render_state_new retry rc={rc}");
            return false;
        }

        self.render_state = render_state;
        self.applied_full_snapshot_generation = u64::MAX;
        self.force_full_snapshot = true;
        self.last_cursor = Cursor::default();
        self.snapshot_metadata = None;
        true
    }

    fn invalidate_render_state(&mut self) {
        if !self.render_state.is_null() {
            unsafe { ghostty_render_state_free(self.render_state) };
            self.render_state = std::ptr::null_mut();
        }
        self.applied_full_snapshot_generation = u64::MAX;
        self.force_full_snapshot = true;
        self.last_cursor = Cursor::default();
        self.snapshot_metadata = None;
    }
}

unsafe impl Send for VtInner {}
unsafe impl Send for VtRenderState {}

#[repr(i32)]
#[derive(Debug, Clone, Copy)]
enum GhosttyKey {
    Unidentified = 0,
    AltLeft = 51,
    AltRight = 52,
    Backspace = 53,
    CapsLock = 54,
    ContextMenu = 55,
    ControlLeft = 56,
    ControlRight = 57,
    Enter = 58,
    MetaLeft = 59,
    MetaRight = 60,
    ShiftLeft = 61,
    ShiftRight = 62,
    Space = 63,
    Tab = 64,
    Delete = 68,
    End = 69,
    Help = 70,
    Home = 71,
    Insert = 72,
    PageDown = 73,
    PageUp = 74,
    ArrowDown = 75,
    ArrowLeft = 76,
    ArrowRight = 77,
    ArrowUp = 78,
    Escape = 120,
    F1 = 121,
    F2 = 122,
    F3 = 123,
    F4 = 124,
    F5 = 125,
    F6 = 126,
    F7 = 127,
    F8 = 128,
    F9 = 129,
    F10 = 130,
    F11 = 131,
    F12 = 132,
    F13 = 133,
    F14 = 134,
    F15 = 135,
    F16 = 136,
    F17 = 137,
    F18 = 138,
    F19 = 139,
    F20 = 140,
    F21 = 141,
    F22 = 142,
    F23 = 143,
    F24 = 144,
    F25 = 145,
    PrintScreen = 148,
    ScrollLock = 149,
    Pause = 150,
}

fn ghostty_key_action(action: VtKeyAction) -> c_int {
    match action {
        VtKeyAction::Release => 0,
        VtKeyAction::Press => 1,
        VtKeyAction::Repeat => 2,
    }
}

fn ghostty_modifiers(modifiers: VtKeyModifiers) -> u16 {
    (u16::from(modifiers.shift) * GHOSTTY_MODS_SHIFT)
        | (u16::from(modifiers.control) * GHOSTTY_MODS_CTRL)
        | (u16::from(modifiers.alt) * GHOSTTY_MODS_ALT)
        | (u16::from(modifiers.platform) * GHOSTTY_MODS_SUPER)
}

fn ghostty_mouse_action(action: VtMouseAction) -> GhosttyMouseAction {
    match action {
        VtMouseAction::Press => GhosttyMouseAction::Press,
        VtMouseAction::Release => GhosttyMouseAction::Release,
        VtMouseAction::Motion => GhosttyMouseAction::Motion,
    }
}

fn ghostty_mouse_button(button: VtMouseButton) -> GhosttyMouseButton {
    match button {
        VtMouseButton::Left => GhosttyMouseButton::Left,
        VtMouseButton::Middle => GhosttyMouseButton::Middle,
        VtMouseButton::Right => GhosttyMouseButton::Right,
        VtMouseButton::Button4 => GhosttyMouseButton::Four,
        VtMouseButton::Button5 => GhosttyMouseButton::Five,
        VtMouseButton::Button6 => GhosttyMouseButton::Six,
        VtMouseButton::Button7 => GhosttyMouseButton::Seven,
    }
}

fn ghostty_mouse_modifiers(modifiers: VtMouseModifiers) -> u16 {
    (u16::from(modifiers.shift) * GHOSTTY_MODS_SHIFT)
        | (u16::from(modifiers.control) * GHOSTTY_MODS_CTRL)
        | (u16::from(modifiers.alt) * GHOSTTY_MODS_ALT)
}

/// Map only key names whose physical identity is unambiguous in GPUI's
/// logical event model. Printable keys intentionally remain UNIDENTIFIED:
/// treating a layout-normalized `"a"` as the physical KeyA position would
/// corrupt Kitty alternate-key reporting on non-US layouts.
fn ghostty_key(key: &str) -> c_int {
    (match key {
        "alt" | "alt-left" => GhosttyKey::AltLeft,
        "alt-right" => GhosttyKey::AltRight,
        "backspace" => GhosttyKey::Backspace,
        "capslock" | "caps-lock" => GhosttyKey::CapsLock,
        "context-menu" => GhosttyKey::ContextMenu,
        "control" | "ctrl" | "control-left" => GhosttyKey::ControlLeft,
        "control-right" => GhosttyKey::ControlRight,
        "enter" | "return" => GhosttyKey::Enter,
        "meta" | "super" | "win" | "meta-left" => GhosttyKey::MetaLeft,
        "meta-right" => GhosttyKey::MetaRight,
        "shift" | "shift-left" => GhosttyKey::ShiftLeft,
        "shift-right" => GhosttyKey::ShiftRight,
        "space" => GhosttyKey::Space,
        "tab" => GhosttyKey::Tab,
        "delete" => GhosttyKey::Delete,
        "end" => GhosttyKey::End,
        "help" => GhosttyKey::Help,
        "home" => GhosttyKey::Home,
        "insert" => GhosttyKey::Insert,
        "pagedown" | "page-down" => GhosttyKey::PageDown,
        "pageup" | "page-up" => GhosttyKey::PageUp,
        "down" | "arrow-down" => GhosttyKey::ArrowDown,
        "left" | "arrow-left" => GhosttyKey::ArrowLeft,
        "right" | "arrow-right" => GhosttyKey::ArrowRight,
        "up" | "arrow-up" => GhosttyKey::ArrowUp,
        "escape" => GhosttyKey::Escape,
        "f1" => GhosttyKey::F1,
        "f2" => GhosttyKey::F2,
        "f3" => GhosttyKey::F3,
        "f4" => GhosttyKey::F4,
        "f5" => GhosttyKey::F5,
        "f6" => GhosttyKey::F6,
        "f7" => GhosttyKey::F7,
        "f8" => GhosttyKey::F8,
        "f9" => GhosttyKey::F9,
        "f10" => GhosttyKey::F10,
        "f11" => GhosttyKey::F11,
        "f12" => GhosttyKey::F12,
        "f13" => GhosttyKey::F13,
        "f14" => GhosttyKey::F14,
        "f15" => GhosttyKey::F15,
        "f16" => GhosttyKey::F16,
        "f17" => GhosttyKey::F17,
        "f18" => GhosttyKey::F18,
        "f19" => GhosttyKey::F19,
        "f20" => GhosttyKey::F20,
        "f21" => GhosttyKey::F21,
        "f22" => GhosttyKey::F22,
        "f23" => GhosttyKey::F23,
        "f24" => GhosttyKey::F24,
        "f25" => GhosttyKey::F25,
        "printscreen" | "print-screen" => GhosttyKey::PrintScreen,
        "scrolllock" | "scroll-lock" => GhosttyKey::ScrollLock,
        "pause" => GhosttyKey::Pause,
        _ => GhosttyKey::Unidentified,
    }) as c_int
}

/// Whether Con's GPUI bridge can identify this logical name as a physical
/// functional key without guessing a keyboard layout or key location.
pub fn is_supported_functional_key(key: &str) -> bool {
    ghostty_key(key) != GhosttyKey::Unidentified as c_int
}

fn default_device_attributes() -> GhosttyDeviceAttributes {
    let mut features = [0_u16; 64];
    features[0] = GHOSTTY_DA_FEATURE_SELECTIVE_ERASE;
    features[1] = GHOSTTY_DA_FEATURE_WINDOWING;
    features[2] = GHOSTTY_DA_FEATURE_ANSI_COLOR;
    features[3] = GHOSTTY_DA_FEATURE_RECTANGULAR_EDITING;
    features[4] = GHOSTTY_DA_FEATURE_CLIPBOARD;
    GhosttyDeviceAttributes {
        primary: GhosttyDeviceAttributesPrimary {
            conformance_level: GHOSTTY_DA_CONFORMANCE_LEVEL_2,
            features,
            num_features: 5,
        },
        secondary: GhosttyDeviceAttributesSecondary {
            device_type: GHOSTTY_DA_DEVICE_TYPE_VT220,
            firmware_version: 0,
            rom_cartridge: 0,
        },
        tertiary: GhosttyDeviceAttributesTertiary { unit_id: 0 },
    }
}

fn parse_osc7_cwd(url: &str) -> Option<String> {
    let url = url.trim();
    let path = url.strip_prefix("file://")?;
    if path.is_empty() {
        return None;
    }

    let decoded = percent_decode_lossy(path);
    if decoded.is_empty() {
        return None;
    }

    if let Some(without_slash) = decoded.strip_prefix('/')
        && is_windows_drive_path(without_slash)
    {
        return Some(without_slash.replace('/', "\\"));
    }

    if !decoded.starts_with('/') {
        let (host, rest) = decoded.split_once('/')?;
        if host.is_empty() {
            return Some(local_file_path_from_rest(rest));
        }
        if host.eq_ignore_ascii_case("localhost") {
            return Some(local_file_path_from_rest(rest));
        }
        #[cfg(not(target_os = "windows"))]
        {
            return Some(format!("/{rest}"));
        }
        #[cfg(target_os = "windows")]
        return Some(format!("\\\\{}\\{}", host, rest.replace('/', "\\")));
    }

    Some(decoded)
}

fn local_file_path_from_rest(rest: &str) -> String {
    if is_windows_drive_path(rest) {
        rest.replace('/', "\\")
    } else {
        format!("/{rest}")
    }
}

fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

fn percent_decode_lossy(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push((high << 4) | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

impl VtScreen {
    pub fn new(cols: u16, rows: u16, theme: Option<&ThemeColors>) -> anyhow::Result<Self> {
        Self::new_with_write_pty(cols, rows, theme, None)
    }

    pub fn new_with_write_pty(
        cols: u16,
        rows: u16,
        theme: Option<&ThemeColors>,
        write_pty: Option<PtyWriteCallback>,
    ) -> anyhow::Result<Self> {
        install_ghostty_sys_services()?;

        let mut terminal: GhosttyTerminal = std::ptr::null_mut();
        // SAFETY: out param; allocator NULL = upstream default.
        let rc = unsafe { ghostty_terminal_new(std::ptr::null(), &mut terminal, cols, rows) };
        if rc != 0 || terminal.is_null() {
            anyhow::bail!("ghostty_terminal_new failed: rc={rc}");
        }

        let max_scrollback_lines = 10_000_usize;
        let rc = unsafe {
            ghostty_terminal_set(
                terminal,
                GhosttyTerminalOption::ScrollbackMaxLines,
                &max_scrollback_lines as *const _ as *const c_void,
            )
        };
        if rc != 0 {
            unsafe { ghostty_terminal_free(terminal) };
            anyhow::bail!("ghostty_terminal_set(SCROLLBACK_MAX_LINES) failed: rc={rc}");
        }

        let rc = unsafe {
            ghostty_terminal_set(
                terminal,
                GhosttyTerminalOption::KittyImageStorageLimit,
                &KITTY_IMAGE_STORAGE_LIMIT_BYTES as *const _ as *const c_void,
            )
        };
        if rc != 0 {
            unsafe { ghostty_terminal_free(terminal) };
            anyhow::bail!("ghostty_terminal_set(KITTY_IMAGE_STORAGE_LIMIT) failed: rc={rc}");
        }

        let clipboard_write_max_bytes = 0_usize;
        let rc = unsafe {
            ghostty_terminal_set(
                terminal,
                GhosttyTerminalOption::ClipboardWriteMaxBytes,
                &clipboard_write_max_bytes as *const _ as *const c_void,
            )
        };
        if rc != 0 {
            unsafe { ghostty_terminal_free(terminal) };
            anyhow::bail!("ghostty_terminal_set(CLIPBOARD_WRITE_MAX_BYTES) failed: rc={rc}");
        }

        let mut callback_state = Box::new(VtCallbackState {
            write_pty,
            write_order: Arc::new(Mutex::new(())),
            enquiry_response: b"con".to_vec().into_boxed_slice(),
            clipboard_text: Mutex::new(None),
            write_failed: Arc::new(AtomicBool::new(false)),
            clipboard_write_enabled: AtomicBool::new(false),
            clipboard_write_policy: Arc::new(ClipboardWritePolicy::new(true)),
            pending_clipboard_write: Mutex::new(None),
            pending_paste_write: Mutex::new(None),
            rows: AtomicU16::new(rows),
            cols: AtomicU16::new(cols),
            cell_width: AtomicU32::new(1),
            cell_height: AtomicU32::new(1),
            dark_mode: AtomicBool::new(false),
            device_attributes: default_device_attributes(),
            metadata_dirty: AtomicU8::new(0),
            bell_pending: AtomicBool::new(false),
            desktop_notification_policy: Arc::new(DesktopNotificationPolicy::default()),
            progress_epoch: Instant::now(),
            progress: AtomicU64::new(0),
            unknown_sequence_log_count: AtomicU8::new(0),
        });
        let userdata = callback_state.as_mut() as *mut VtCallbackState as *mut c_void;
        let rc =
            unsafe { ghostty_terminal_set(terminal, GhosttyTerminalOption::Userdata, userdata) };
        if rc != 0 {
            unsafe { ghostty_terminal_free(terminal) };
            anyhow::bail!("ghostty_terminal_set(USERDATA) failed: rc={rc}");
        }

        if callback_state.write_pty.is_some() {
            let rc = unsafe {
                ghostty_terminal_set(
                    terminal,
                    GhosttyTerminalOption::WritePty,
                    vt_write_pty_callback as *const c_void,
                )
            };
            if rc != 0 {
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_terminal_set(WRITE_PTY) failed: rc={rc}");
            }

            let callback_options = [
                (
                    GhosttyTerminalOption::Enquiry,
                    vt_enquiry_callback as *const c_void,
                    "ENQUIRY",
                ),
                (
                    GhosttyTerminalOption::Size,
                    vt_size_callback as *const c_void,
                    "SIZE",
                ),
                (
                    GhosttyTerminalOption::ColorScheme,
                    vt_color_scheme_callback as *const c_void,
                    "COLOR_SCHEME",
                ),
                (
                    GhosttyTerminalOption::DeviceAttributes,
                    vt_device_attributes_callback as *const c_void,
                    "DEVICE_ATTRIBUTES",
                ),
                (
                    GhosttyTerminalOption::Xtversion,
                    vt_xtversion_callback as *const c_void,
                    "XTVERSION",
                ),
                (
                    GhosttyTerminalOption::ClipboardRead,
                    vt_clipboard_read_callback as *const c_void,
                    "CLIPBOARD_READ",
                ),
            ];

            for (option, callback, label) in callback_options {
                let rc = unsafe { ghostty_terminal_set(terminal, option, callback) };
                if rc != 0 {
                    unsafe { ghostty_terminal_free(terminal) };
                    anyhow::bail!("ghostty_terminal_set({label}) failed: rc={rc}");
                }
            }
        }

        let effect_callbacks = [
            (
                GhosttyTerminalOption::Bell,
                vt_bell_callback as *const c_void,
                "BELL",
            ),
            (
                GhosttyTerminalOption::TitleChanged,
                vt_title_changed_callback as *const c_void,
                "TITLE_CHANGED",
            ),
            (
                GhosttyTerminalOption::PwdChanged,
                vt_pwd_changed_callback as *const c_void,
                "PWD_CHANGED",
            ),
            (
                GhosttyTerminalOption::DesktopNotification,
                vt_desktop_notification_callback as *const c_void,
                "DESKTOP_NOTIFICATION",
            ),
            (
                GhosttyTerminalOption::ProgressReport,
                vt_progress_report_callback as *const c_void,
                "PROGRESS_REPORT",
            ),
        ];
        for (option, callback, label) in effect_callbacks {
            let rc = unsafe { ghostty_terminal_set(terminal, option, callback) };
            if rc != 0 {
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_terminal_set({label}) failed: rc={rc}");
            }
        }

        if log::log_enabled!(target: UNKNOWN_SEQUENCE_LOG_TARGET, log::Level::Debug) {
            let rc = unsafe {
                ghostty_terminal_set(
                    terminal,
                    GhosttyTerminalOption::UnknownMaxBytes,
                    &UNKNOWN_SEQUENCE_MAX_BYTES as *const _ as *const c_void,
                )
            };
            if rc != 0 {
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_terminal_set(UNKNOWN_MAX_BYTES) failed: rc={rc}");
            }

            let rc = unsafe {
                ghostty_terminal_set(
                    terminal,
                    GhosttyTerminalOption::UnknownSequence,
                    vt_unknown_sequence_callback as *const c_void,
                )
            };
            if rc != 0 {
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_terminal_set(UNKNOWN_SEQUENCE) failed: rc={rc}");
            }
        }

        let mut render_state: GhosttyRenderState = std::ptr::null_mut();
        let mut row_iter: GhosttyRowIterator = std::ptr::null_mut();
        let mut row_cells: GhosttyRowCells = std::ptr::null_mut();

        let enable_render_state = std::env::var("CON_GHOSTTY_VT_RENDER_STATE")
            .map(|s| matches!(s.as_str(), "0" | "false" | "no" | "off"))
            .map(|disabled| !disabled)
            .unwrap_or(true);

        if enable_render_state {
            // SAFETY: out param; allocator NULL = default.
            let rc = unsafe { ghostty_render_state_new(std::ptr::null(), &mut render_state) };
            if rc != 0 || render_state.is_null() {
                // SAFETY: terminal owned; free on partial init failure.
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_render_state_new failed: rc={rc}");
            }

            // SAFETY: out param.
            let rc =
                unsafe { ghostty_render_state_row_iterator_new(std::ptr::null(), &mut row_iter) };
            if rc != 0 || row_iter.is_null() {
                unsafe { ghostty_render_state_free(render_state) };
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_render_state_row_iterator_new failed: rc={rc}");
            }

            // SAFETY: out param.
            let rc =
                unsafe { ghostty_render_state_row_cells_new(std::ptr::null(), &mut row_cells) };
            if rc != 0 || row_cells.is_null() {
                unsafe { ghostty_render_state_row_iterator_free(row_iter) };
                unsafe { ghostty_render_state_free(render_state) };
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_render_state_row_cells_new failed: rc={rc}");
            }
        } else {
            log::warn!(
                "VtScreen::new: render_state disabled via \
                 CON_GHOSTTY_VT_RENDER_STATE=0 — terminal output will \
                 parse but cells won't render."
            );
        }

        let mut kitty_placement_iter: GhosttyKittyGraphicsPlacementIterator = std::ptr::null_mut();
        let rc = unsafe {
            ghostty_kitty_graphics_placement_iterator_new(
                std::ptr::null(),
                &mut kitty_placement_iter,
            )
        };
        if rc != GHOSTTY_SUCCESS || kitty_placement_iter.is_null() {
            unsafe {
                if !row_cells.is_null() {
                    ghostty_render_state_row_cells_free(row_cells);
                }
                if !row_iter.is_null() {
                    ghostty_render_state_row_iterator_free(row_iter);
                }
                if !render_state.is_null() {
                    ghostty_render_state_free(render_state);
                }
                ghostty_terminal_free(terminal);
            }
            anyhow::bail!("ghostty_kitty_graphics_placement_iterator_new failed: rc={rc}");
        }

        let mut key_encoder: GhosttyKeyEncoder = std::ptr::null_mut();
        let rc = unsafe { ghostty_key_encoder_new(std::ptr::null(), &mut key_encoder) };
        if rc != GHOSTTY_SUCCESS || key_encoder.is_null() {
            unsafe {
                ghostty_kitty_graphics_placement_iterator_free(kitty_placement_iter);
                if !row_cells.is_null() {
                    ghostty_render_state_row_cells_free(row_cells);
                }
                if !row_iter.is_null() {
                    ghostty_render_state_row_iterator_free(row_iter);
                }
                if !render_state.is_null() {
                    ghostty_render_state_free(render_state);
                }
                ghostty_terminal_free(terminal);
            }
            anyhow::bail!("ghostty_key_encoder_new failed: rc={rc}");
        }

        let mut key_event: GhosttyKeyEvent = std::ptr::null_mut();
        let rc = unsafe { ghostty_key_event_new(std::ptr::null(), &mut key_event) };
        if rc != GHOSTTY_SUCCESS || key_event.is_null() {
            unsafe {
                ghostty_key_encoder_free(key_encoder);
                ghostty_kitty_graphics_placement_iterator_free(kitty_placement_iter);
                if !row_cells.is_null() {
                    ghostty_render_state_row_cells_free(row_cells);
                }
                if !row_iter.is_null() {
                    ghostty_render_state_row_iterator_free(row_iter);
                }
                if !render_state.is_null() {
                    ghostty_render_state_free(render_state);
                }
                ghostty_terminal_free(terminal);
            }
            anyhow::bail!("ghostty_key_event_new failed: rc={rc}");
        }

        let mouse = match MouseEncoderState::new(terminal, cols, rows) {
            Ok(mouse) => mouse,
            Err(err) => {
                unsafe {
                    ghostty_key_event_free(key_event);
                    ghostty_key_encoder_free(key_encoder);
                    ghostty_kitty_graphics_placement_iterator_free(kitty_placement_iter);
                    if !row_cells.is_null() {
                        ghostty_render_state_row_cells_free(row_cells);
                    }
                    if !row_iter.is_null() {
                        ghostty_render_state_row_iterator_free(row_iter);
                    }
                    if !render_state.is_null() {
                        ghostty_render_state_free(render_state);
                    }
                    ghostty_terminal_free(terminal);
                }
                return Err(err);
            }
        };

        if let Some(theme) = theme {
            unsafe { apply_theme_to_terminal(terminal, theme) };
        }

        Ok(Self {
            inner: Arc::new(Mutex::new(VtInner {
                terminal,
                key_encoder,
                key_event,
                mouse,
                selection_gesture: None,
                kitty_placement_iter,
                kitty_image_cache: HashMap::new(),
                kitty_placements: Arc::from([]),
                kitty_snapshot_generation: u64::MAX,
                callback_state,
                title: None,
                title_reported: false,
                current_dir: None,
                cols,
                rows,
                generation: 0,
                output_generation: 0,
                required_full_snapshot_generation: 0,
            })),
            render: Mutex::new(VtRenderState {
                render_state,
                row_iter,
                row_cells,
                applied_full_snapshot_generation: u64::MAX,
                force_full_snapshot: false,
                scratch_cols: cols,
                scratch_rows: rows,
                scratch: Vec::with_capacity(cols as usize * rows as usize),
                selection_ranges: Vec::with_capacity(rows as usize),
                last_cursor: Cursor::default(),
                snapshot_metadata: None,
            }),
        })
    }

    pub fn set_clipboard_write_enabled(&self, enabled: bool) -> Result<(), String> {
        let inner = self.inner.lock();
        if enabled {
            let clipboard_write_max_bytes = CLIPBOARD_WRITE_LIMIT_BYTES;
            let rc = unsafe {
                ghostty_terminal_set(
                    inner.terminal,
                    GhosttyTerminalOption::ClipboardWriteMaxBytes,
                    &clipboard_write_max_bytes as *const _ as *const c_void,
                )
            };
            if rc != 0 {
                return Err(format!(
                    "ghostty_terminal_set(CLIPBOARD_WRITE_MAX_BYTES) failed: rc={rc}"
                ));
            }

            let rc = unsafe {
                ghostty_terminal_set(
                    inner.terminal,
                    GhosttyTerminalOption::ClipboardWrite,
                    vt_clipboard_write_callback as *const c_void,
                )
            };
            if rc != 0 {
                let disabled_limit = 0_usize;
                let _ = unsafe {
                    ghostty_terminal_set(
                        inner.terminal,
                        GhosttyTerminalOption::ClipboardWriteMaxBytes,
                        &disabled_limit as *const _ as *const c_void,
                    )
                };
                return Err(format!(
                    "ghostty_terminal_set(CLIPBOARD_WRITE) failed: rc={rc}"
                ));
            }
            inner
                .callback_state
                .clipboard_write_enabled
                .store(true, Ordering::Release);
            return Ok(());
        }

        inner
            .callback_state
            .clipboard_write_enabled
            .store(false, Ordering::Release);
        inner.callback_state.pending_clipboard_write.lock().take();
        let callback_rc = unsafe {
            ghostty_terminal_set(
                inner.terminal,
                GhosttyTerminalOption::ClipboardWrite,
                std::ptr::null(),
            )
        };
        let disabled_limit = 0_usize;
        let limit_rc = unsafe {
            ghostty_terminal_set(
                inner.terminal,
                GhosttyTerminalOption::ClipboardWriteMaxBytes,
                &disabled_limit as *const _ as *const c_void,
            )
        };
        if callback_rc != 0 {
            return Err(format!(
                "ghostty_terminal_set(CLIPBOARD_WRITE) failed: rc={callback_rc}"
            ));
        }
        if limit_rc != 0 {
            return Err(format!(
                "ghostty_terminal_set(CLIPBOARD_WRITE_MAX_BYTES) failed: rc={limit_rc}"
            ));
        }
        Ok(())
    }

    pub fn take_clipboard_write(&self) -> Option<String> {
        let inner = self.inner.lock();
        let state = &inner.callback_state;
        let pending = state.pending_clipboard_write.lock().take();
        if state.clipboard_write_policy.is_enabled() {
            pending
        } else {
            None
        }
    }

    pub(crate) fn set_clipboard_write_policy(&self, policy: Arc<ClipboardWritePolicy>) {
        let mut inner = self.inner.lock();
        if !policy.is_enabled() {
            inner.callback_state.pending_clipboard_write.lock().take();
        }
        inner.callback_state.clipboard_write_policy = policy;
    }

    pub fn take_desktop_notification(&self) -> Option<DesktopNotification> {
        self.inner
            .lock()
            .callback_state
            .desktop_notification_policy
            .take()
    }

    pub(crate) fn set_desktop_notification_policy(&self, policy: Arc<DesktopNotificationPolicy>) {
        self.inner.lock().callback_state.desktop_notification_policy = policy;
    }

    /// Replace the default fg/bg/palette. Bumps the snapshot
    /// generation so the next prepaint repaints with the new colors.
    pub fn set_theme(&self, theme: &ThemeColors) {
        let mut inner = self.inner.lock();
        unsafe { apply_theme_to_terminal(inner.terminal, theme) };
        inner.generation = inner.generation.wrapping_add(1);
        inner.required_full_snapshot_generation = inner.generation;
    }

    /// Force the next `snapshot()` to report a new generation so the
    /// renderer's `needs_draw` gate treats the frame as dirty even
    /// though no VT bytes or theme changes landed. Used by opacity-
    /// only appearance updates, where `config.background_opacity`
    /// changes on the renderer side but the VT screen itself is
    /// untouched.
    pub fn bump_generation(&self) {
        let mut inner = self.inner.lock();
        inner.generation = inner.generation.wrapping_add(1);
    }

    pub fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    /// Begin a local text-selection gesture at a viewport cell.
    ///
    /// `click_count` comes from the platform event so OS accessibility
    /// settings remain authoritative. A shifted single click extends an
    /// existing active gesture; otherwise Ghostty's default cell/word/line
    /// behavior is selected from the platform click count.
    pub fn selection_press(
        &self,
        point: SelectionPoint,
        geometry: SelectionGeometry,
        click_count: u8,
        extend: bool,
    ) -> anyhow::Result<()> {
        validate_selection_point(point)?;
        let geometry = geometry.to_ffi()?;
        let mut inner = self.inner.lock();
        let terminal = inner.terminal;

        if inner.selection_gesture.is_none() {
            inner.selection_gesture = Some(SelectionGestureState::new()?);
        }
        let can_extend = extend
            && click_count == 1
            && has_selection_locked(&inner)?
            && selection_gesture_click_count(&inner)? > 0;
        let grid_ref = selection_grid_ref(terminal, point.col, point.row)?;
        let state = inner
            .selection_gesture
            .as_ref()
            .expect("selection gesture initialized");
        let gesture = state.gesture;
        let position = selection_surface_position(point);

        if can_extend {
            let event = state.drag;
            selection_event_set(event, GhosttySelectionGestureEventOption::Ref, &grid_ref)?;
            selection_event_set(
                event,
                GhosttySelectionGestureEventOption::Position,
                &position,
            )?;
            selection_event_set(
                event,
                GhosttySelectionGestureEventOption::Geometry,
                &geometry,
            )?;
            if let Some(selection) = apply_selection_gesture_event(gesture, terminal, event)? {
                set_selection_locked(&mut inner, Some(&selection))?;
            }
            return Ok(());
        }

        // GPUI has already applied the platform's repeat interval and distance
        // policy. Replay the reported count from a clean gesture so Ghostty
        // selects the matching cell/word/line behavior without a second,
        // hard-coded timing policy disagreeing with the OS.
        unsafe { ghostty_selection_gesture_reset(gesture, terminal) };
        let event = state.press;
        let time_ns = state.time_ns();
        let repeat_distance = f64::from(geometry.cell_width);
        let repeat_interval_ns = u64::MAX;

        selection_event_set(event, GhosttySelectionGestureEventOption::Ref, &grid_ref)?;
        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::Position,
            &position,
        )?;
        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::RepeatDistance,
            &repeat_distance,
        )?;
        selection_event_set(event, GhosttySelectionGestureEventOption::TimeNs, &time_ns)?;
        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::RepeatIntervalNs,
            &repeat_interval_ns,
        )?;

        let mut selection = None;
        for _ in 0..click_count.clamp(1, 3) {
            selection = apply_selection_gesture_event(gesture, terminal, event)?;
        }
        if let Some(selection) = selection {
            set_selection_locked(&mut inner, Some(&selection))?;
        } else if selection_gesture_click_count(&inner)? == 1 {
            set_selection_locked(&mut inner, None)?;
        }
        Ok(())
    }

    /// Continue the active selection gesture. The returned direction tells the
    /// view whether it should run selection autoscroll ticks.
    pub fn selection_drag(
        &self,
        point: SelectionPoint,
        geometry: SelectionGeometry,
    ) -> anyhow::Result<SelectionAutoscroll> {
        validate_selection_point(point)?;
        let geometry = geometry.to_ffi()?;
        let mut inner = self.inner.lock();
        let terminal = inner.terminal;
        let Some(state) = inner.selection_gesture.as_ref() else {
            return Ok(SelectionAutoscroll::None);
        };
        let event = state.drag;
        let gesture = state.gesture;
        let grid_ref = selection_grid_ref(terminal, point.col, point.row)?;
        let position = selection_surface_position(point);

        selection_event_set(event, GhosttySelectionGestureEventOption::Ref, &grid_ref)?;
        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::Position,
            &position,
        )?;
        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::Geometry,
            &geometry,
        )?;

        let selection = apply_selection_gesture_event(gesture, terminal, event)?;
        let click_count = selection_gesture_click_count(&inner)?;
        if let Some(selection) = selection {
            set_selection_locked(&mut inner, Some(&selection))?;
        } else if click_count > 0 {
            set_selection_locked(&mut inner, None)?;
        }
        selection_gesture_autoscroll(&inner)
    }

    /// Advance an active selection autoscroll gesture by one row.
    pub fn selection_autoscroll_tick(
        &self,
        point: SelectionPoint,
        geometry: SelectionGeometry,
    ) -> anyhow::Result<SelectionAutoscrollUpdate> {
        validate_selection_point(point)?;
        let geometry = geometry.to_ffi()?;
        let mut inner = self.inner.lock();
        let terminal = inner.terminal;
        let Some(state) = inner.selection_gesture.as_ref() else {
            return Ok(SelectionAutoscrollUpdate::default());
        };
        let event = state.autoscroll_tick;
        let gesture = state.gesture;
        let viewport = GhosttyPointCoordinate {
            x: point.col,
            y: u32::from(point.row),
        };
        let position = selection_surface_position(point);
        let autoscroll_before = selection_gesture_autoscroll(&inner)?;
        if autoscroll_before == SelectionAutoscroll::None {
            return Ok(SelectionAutoscrollUpdate::default());
        }

        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::Viewport,
            &viewport,
        )?;
        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::Position,
            &position,
        )?;
        selection_event_set(
            event,
            GhosttySelectionGestureEventOption::Geometry,
            &geometry,
        )?;

        let generation_before = inner.generation;
        let offset_before = read_scrollbar(terminal).map(|scrollbar| scrollbar.offset);
        let selection = apply_selection_gesture_event(gesture, terminal, event)?;
        let offset_after = read_scrollbar(terminal).map(|scrollbar| scrollbar.offset);
        let viewport_changed = match (offset_before, offset_after) {
            (Some(before), Some(after)) => before != after,
            // Preserve correctness if scrollbar introspection itself fails.
            _ => true,
        };
        let result = (|| {
            let click_count = selection_gesture_click_count(&inner)?;
            let selection_changed = if let Some(selection) = selection {
                set_selection_locked(&mut inner, Some(&selection))?
            } else if click_count > 0 {
                set_selection_locked(&mut inner, None)?
            } else {
                false
            };
            Ok::<_, anyhow::Error>((selection_gesture_autoscroll(&inner)?, selection_changed))
        })();

        // A successful tick mutates the viewport inside Ghostty. Keep that
        // mutation and any selection install under this lock, but avoid a new
        // snapshot generation when both were clamped/equal at a scrollback
        // boundary. If a later query failed, still publish a real viewport
        // movement before returning the error.
        if viewport_changed && inner.generation == generation_before {
            inner.generation = inner.generation.wrapping_add(1);
        }
        let (direction, selection_changed) = result?;
        Ok(SelectionAutoscrollUpdate {
            direction,
            changed: viewport_changed || selection_changed,
        })
    }

    /// Finish a normal pointer gesture while retaining repeat-click state for
    /// a possible double or triple click.
    pub fn selection_release(&self, point: Option<(u16, u16)>) -> anyhow::Result<()> {
        let inner = self.inner.lock();
        let terminal = inner.terminal;
        let Some(state) = inner.selection_gesture.as_ref() else {
            return Ok(());
        };
        let event = state.release;
        let gesture = state.gesture;

        let grid_ref = point
            .map(|(col, row)| selection_grid_ref(terminal, col, row))
            .transpose()?;
        let value = grid_ref.as_ref().map_or(std::ptr::null(), |grid_ref| {
            grid_ref as *const _ as *const c_void
        });
        let rc = unsafe {
            ghostty_selection_gesture_event_set(
                event,
                GhosttySelectionGestureEventOption::Ref,
                value,
            )
        };
        if rc != GHOSTTY_SUCCESS {
            anyhow::bail!("ghostty_selection_gesture_event_set(RELEASE REF) failed: rc={rc}");
        }
        let rc = unsafe {
            ghostty_selection_gesture_event(gesture, terminal, event, std::ptr::null_mut())
        };
        if rc != GHOSTTY_NO_VALUE && rc != GHOSTTY_SUCCESS {
            anyhow::bail!("ghostty_selection_gesture_event(RELEASE) failed: rc={rc}");
        }
        Ok(())
    }

    /// Abandon the active pointer gesture without changing the installed
    /// terminal selection.
    pub fn selection_cancel_gesture(&self) {
        let inner = self.inner.lock();
        if let Some(state) = inner.selection_gesture.as_ref() {
            unsafe { ghostty_selection_gesture_reset(state.gesture, inner.terminal) };
        }
    }

    /// Read selection presence. FFI failures are logged and reported as false.
    pub fn has_selection(&self) -> bool {
        match has_selection_locked(&self.inner.lock()) {
            Ok(has_selection) => has_selection,
            Err(err) => {
                log::warn!("failed to read terminal selection: {err:#}");
                false
            }
        }
    }

    /// Format the active selection. FFI or UTF-8 failures are logged and
    /// reported as no selection.
    pub fn selection_text(&self) -> Option<String> {
        match selection_text_locked(&self.inner.lock()) {
            Ok(selection) => selection,
            Err(err) => {
                log::warn!("failed to format terminal selection: {err:#}");
                None
            }
        }
    }

    /// Atomically format and clear the active selection.
    ///
    /// An empty string is a real empty selection; `None` means there was no
    /// selection or formatting failed. Failures are logged and leave the
    /// selection installed so a later copy can retry.
    pub fn take_selection_text(&self) -> Option<String> {
        let mut inner = self.inner.lock();
        let result = (|| {
            let Some(text) = selection_text_locked(&inner)? else {
                return Ok(None);
            };
            if let Some(state) = inner.selection_gesture.as_ref() {
                unsafe { ghostty_selection_gesture_reset(state.gesture, inner.terminal) };
            }
            set_selection_locked(&mut inner, None)?;
            Ok::<_, anyhow::Error>(Some(text))
        })();
        match result {
            Ok(selection) => selection,
            Err(err) => {
                log::warn!("failed to take terminal selection: {err:#}");
                None
            }
        }
    }

    /// Clear the terminal-owned selection. Returns whether a selection was
    /// present and successfully cleared.
    pub fn clear_selection(&self) -> bool {
        let mut inner = self.inner.lock();
        if let Some(state) = inner.selection_gesture.as_ref() {
            unsafe { ghostty_selection_gesture_reset(state.gesture, inner.terminal) };
        }
        match set_selection_locked(&mut inner, None) {
            Ok(changed) => changed,
            Err(err) => {
                log::warn!("failed to clear terminal selection: {err:#}");
                false
            }
        }
    }

    pub(crate) fn acknowledge_snapshot(&self, generation: u64) {
        let mut render = self.render.lock();
        if !render
            .snapshot_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.generation == generation)
        {
            return;
        }

        // Newer terminal output remains terminal-dirty until the next begin;
        // only a newer render snapshot makes this acknowledgment stale.
        let rc = unsafe { ghostty_render_state_clean(render.render_state) };
        if rc != GHOSTTY_SUCCESS {
            log::warn!("ghostty_render_state_clean rc={rc} generation={generation}");
            return;
        }
        if let Some(metadata) = render.snapshot_metadata.as_mut() {
            metadata.dirty_rows.clear();
        }
    }

    /// Consume one or more bells coalesced since the previous drain.
    pub fn take_bell(&self) -> bool {
        self.inner
            .lock()
            .callback_state
            .bell_pending
            .swap(false, Ordering::Relaxed)
    }

    pub fn progress(&self) -> Option<TerminalProgress> {
        let inner = self.inner.lock();
        let state = &inner.callback_state;
        let encoded = state.progress.load(Ordering::Relaxed);
        if encoded == 0 {
            return None;
        }
        let progress =
            decode_timed_terminal_progress(encoded, terminal_progress_tick(&state.progress_epoch));
        if progress.is_none() {
            let _ =
                state
                    .progress
                    .compare_exchange(encoded, 0, Ordering::Relaxed, Ordering::Relaxed);
        }
        progress
    }

    pub fn is_write_desynchronized(&self) -> bool {
        self.inner
            .lock()
            .callback_state
            .write_failed
            .load(Ordering::Acquire)
    }

    /// Enqueue raw user input through the same ordering boundary as encoded
    /// keys and parser replies.
    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_bytes(bytes, PtyWriteClass::Regular)
    }

    /// Enqueue a state-balancing host report, such as a key or mouse release,
    /// using the queue capacity reserved for terminal control traffic.
    pub fn write_control(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.write_bytes(bytes, PtyWriteClass::ReservedControl)
    }

    fn write_bytes(&self, bytes: &[u8], class: PtyWriteClass) -> std::io::Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let inner = self.inner.lock();
        let callback_state = &inner.callback_state;
        let Some(write_pty) = callback_state.write_pty.clone() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "terminal input requires a WRITE_PTY callback",
            ));
        };
        let write_failed = callback_state.write_failed.clone();
        let write_order = callback_state.write_order.clone();
        let write_guard = write_order.lock();
        drop(inner);
        let result = write_pty(bytes, class);
        if let Err(err) = &result
            && class == PtyWriteClass::ReservedControl
        {
            mark_control_write_failed(&write_failed, err);
        }
        drop(write_guard);
        result
    }

    /// Feed bytes from the PTY into the parser. Non-reentrant per
    /// upstream: do not call from inside a registered callback.
    pub fn feed(&self, bytes: &[u8]) {
        let mut inner = self.inner.lock();
        // SAFETY: terminal valid; bytes live for the call.
        unsafe { ghostty_terminal_vt_write(inner.terminal, bytes.as_ptr(), bytes.len()) };
        // Tracking event and output format are last-write-wins terminal flags,
        // not a pure function of the DEC mode bitset. Synchronize from the
        // terminal after every parsed output chunk so repeated DECSET/DECRST
        // sequences cannot leave the encoder on a stale protocol.
        unsafe { ghostty_mouse_encoder_setopt_from_terminal(inner.mouse.encoder, inner.terminal) };
        refresh_terminal_metadata(&mut inner);
        inner.output_generation = inner.output_generation.wrapping_add(1);
        inner.generation = inner.generation.wrapping_add(1);
    }

    /// Update the renderer geometry used for cell and SGR-pixel mouse formats.
    /// Ghostty clears motion deduplication whenever this option is set, so
    /// compare the complete geometry before crossing the FFI boundary.
    pub fn set_mouse_geometry(
        &self,
        screen_width_px: u32,
        screen_height_px: u32,
        cell_width_px: u32,
        cell_height_px: u32,
    ) {
        let geometry = GhosttyMouseEncoderSize::new(
            screen_width_px,
            screen_height_px,
            cell_width_px,
            cell_height_px,
        );
        let mut inner = self.inner.lock();
        if inner.mouse.geometry == geometry {
            return;
        }
        unsafe {
            ghostty_mouse_encoder_setopt(
                inner.mouse.encoder,
                GhosttyMouseEncoderOption::Size,
                &geometry as *const _ as *const c_void,
            )
        };
        inner.mouse.geometry = geometry;
    }

    /// Encode one normalized pointer event with Ghostty's effective terminal
    /// mouse mode and synchronously write any resulting protocol bytes.
    pub fn send_mouse_event(&self, event: VtMouseEvent) -> anyhow::Result<VtMouseOutcome> {
        if !event.surface_x_px.is_finite() || !event.surface_y_px.is_finite() {
            anyhow::bail!("terminal mouse position must be finite");
        }
        if event.action != VtMouseAction::Motion && event.button.is_none() {
            anyhow::bail!("terminal mouse press and release events require a button");
        }

        let inner = self.inner.lock();
        let callback_state = &inner.callback_state;
        let Some(write_pty) = callback_state.write_pty.clone() else {
            anyhow::bail!("terminal mouse encoding requires a WRITE_PTY callback");
        };
        let write_failed = callback_state.write_failed.clone();
        let write_order = callback_state.write_order.clone();
        let any_button_pressed = event.action == VtMouseAction::Press
            || (event.action == VtMouseAction::Motion && event.button.is_some());

        unsafe {
            ghostty_mouse_encoder_setopt(
                inner.mouse.encoder,
                GhosttyMouseEncoderOption::AnyButtonPressed,
                &any_button_pressed as *const _ as *const c_void,
            );
            ghostty_mouse_event_set_action(inner.mouse.event, ghostty_mouse_action(event.action));
            if let Some(button) = event.button {
                ghostty_mouse_event_set_button(inner.mouse.event, ghostty_mouse_button(button));
            } else {
                ghostty_mouse_event_clear_button(inner.mouse.event);
            }
            ghostty_mouse_event_set_mods(
                inner.mouse.event,
                ghostty_mouse_modifiers(event.modifiers),
            );
            ghostty_mouse_event_set_position(
                inner.mouse.event,
                GhosttyMousePosition {
                    x: event.surface_x_px,
                    y: event.surface_y_px,
                },
            );
        }

        let mut inline = [0_u8; 128];
        let mut len = 0_usize;
        let mut rc = unsafe {
            ghostty_mouse_encoder_encode(
                inner.mouse.encoder,
                inner.mouse.event,
                inline.as_mut_ptr().cast(),
                inline.len(),
                &mut len,
            )
        };

        let mut overflow = Vec::new();
        let bytes = if rc == GHOSTTY_OUT_OF_SPACE {
            overflow.resize(len, 0);
            rc = unsafe {
                ghostty_mouse_encoder_encode(
                    inner.mouse.encoder,
                    inner.mouse.event,
                    overflow.as_mut_ptr().cast(),
                    overflow.len(),
                    &mut len,
                )
            };
            if rc != GHOSTTY_SUCCESS {
                anyhow::bail!("ghostty_mouse_encoder_encode retry failed: rc={rc}");
            }
            if len > overflow.len() {
                anyhow::bail!("ghostty mouse encoder exceeded its requested output size");
            }
            &overflow[..len]
        } else {
            if rc != GHOSTTY_SUCCESS {
                anyhow::bail!("ghostty_mouse_encoder_encode failed: rc={rc}");
            }
            if len > inline.len() {
                anyhow::bail!("ghostty mouse encoder returned an invalid output size");
            }
            &inline[..len]
        };

        if bytes.is_empty() {
            return Ok(VtMouseOutcome::default());
        }

        // Reserve this host-write position before releasing the mode snapshot.
        // Parser-generated replies cannot then overtake the encoded event.
        let write_guard = write_order.lock();
        drop(inner);
        let class = if event.action == VtMouseAction::Release {
            PtyWriteClass::ReservedControl
        } else {
            PtyWriteClass::Regular
        };
        let result = write_pty(bytes, class);
        if let Err(err) = &result
            && class == PtyWriteClass::ReservedControl
        {
            mark_control_write_failed(&write_failed, err);
        }
        drop(write_guard);
        result.map_err(|err| anyhow::anyhow!("failed to write encoded mouse event: {err}"))?;

        Ok(VtMouseOutcome {
            output_written: true,
        })
    }

    /// Encode one platform key event against the terminal's current modes
    /// and synchronously write the resulting bytes through WRITE_PTY.
    ///
    /// The mode snapshot, reusable encoder/event, and terminal are all
    /// protected by one mutex so DECCKM/modifyOtherKeys/Kitty state cannot
    /// change between parsing output and encoding the next key.
    pub fn send_key(&self, event: &VtKeyEvent<'_>) -> anyhow::Result<VtKeyOutcome> {
        let inner = self.inner.lock();
        let callback_state = &inner.callback_state;
        let Some(write_pty) = callback_state.write_pty.clone() else {
            anyhow::bail!("terminal key encoding requires a WRITE_PTY callback");
        };
        let write_failed = callback_state.write_failed.clone();
        let write_order = callback_state.write_order.clone();

        let mut kitty_flags = 0_u8;
        let kitty_flags_rc = unsafe {
            ghostty_terminal_get(
                inner.terminal,
                GhosttyTerminalData::KittyKeyboardFlags,
                &mut kitty_flags as *mut _ as *mut c_void,
            )
        };
        if kitty_flags_rc != GHOSTTY_SUCCESS {
            kitty_flags = 0;
        }

        unsafe {
            ghostty_key_encoder_setopt_from_terminal(inner.key_encoder, inner.terminal);
            ghostty_key_event_set_action(inner.key_event, ghostty_key_action(event.action));
            ghostty_key_event_set_key(inner.key_event, ghostty_key(event.key));
            ghostty_key_event_set_mods(inner.key_event, ghostty_modifiers(event.modifiers));
            ghostty_key_event_set_consumed_mods(
                inner.key_event,
                ghostty_modifiers(event.consumed_modifiers),
            );
            ghostty_key_event_set_composing(inner.key_event, false);
            ghostty_key_event_set_utf8(
                inner.key_event,
                if event.text.is_empty() {
                    std::ptr::null()
                } else {
                    event.text.as_ptr().cast()
                },
                event.text.len(),
            );
            ghostty_key_event_set_unshifted_codepoint(
                inner.key_event,
                event.unshifted_codepoint.map_or(0, u32::from),
            );
        }

        let mut inline = [0_u8; 128];
        let mut len = 0_usize;
        let mut rc = unsafe {
            ghostty_key_encoder_encode(
                inner.key_encoder,
                inner.key_event,
                inline.as_mut_ptr().cast(),
                inline.len(),
                &mut len,
            )
        };

        let mut overflow = Vec::new();
        let bytes = if rc == GHOSTTY_OUT_OF_SPACE {
            overflow.resize(len, 0);
            rc = unsafe {
                ghostty_key_encoder_encode(
                    inner.key_encoder,
                    inner.key_event,
                    overflow.as_mut_ptr().cast(),
                    overflow.len(),
                    &mut len,
                )
            };
            if rc != GHOSTTY_SUCCESS {
                anyhow::bail!("ghostty_key_encoder_encode retry failed: rc={rc}");
            }
            if len > overflow.len() {
                anyhow::bail!("ghostty key encoder exceeded its requested output size");
            }
            &overflow[..len]
        } else {
            if rc != GHOSTTY_SUCCESS {
                anyhow::bail!("ghostty_key_encoder_encode failed: rc={rc}");
            }
            if len > inline.len() {
                anyhow::bail!("ghostty key encoder returned an invalid output size");
            }
            &inline[..len]
        };

        if !bytes.is_empty() {
            // Reserve the next host-write position before releasing the VT
            // state. PTY feeds may then parse concurrently, but any generated
            // reply waits behind this key instead of overtaking it. The real
            // host callbacks enqueue into a non-blocking bounded queue.
            let write_guard = write_order.lock();
            drop(inner);
            let class = if event.action == VtKeyAction::Release {
                PtyWriteClass::ReservedControl
            } else {
                PtyWriteClass::Regular
            };
            if let Err(err) = write_pty(bytes, class) {
                if class == PtyWriteClass::ReservedControl {
                    mark_control_write_failed(&write_failed, &err);
                }
                return Err(anyhow::anyhow!("failed to write encoded key: {err}"));
            }
            drop(write_guard);
        }

        Ok(VtKeyOutcome {
            output_accepted: !bytes.is_empty(),
            report_releases: kitty_flags & GHOSTTY_KITTY_KEY_REPORT_EVENTS != 0,
        })
    }

    /// Paste user-selected clipboard text according to the terminal's active
    /// bracketed-paste and Kitty paste-event modes.
    ///
    /// Potential command injection is rejected before any PTY write. The UI
    /// may explicitly retry with `confirm_unsafe_paste` after showing the text
    /// to the user. A successful Kitty paste event caches only this user-approved
    /// text for the event's one-time granted clipboard read.
    pub fn paste_text(
        &self,
        text: &str,
        source: VtPasteSource,
        confirm_unsafe_paste: bool,
    ) -> anyhow::Result<VtPasteResult> {
        if text.len() > TERMINAL_PASTE_LIMIT_BYTES {
            anyhow::bail!(
                "terminal paste exceeds the {} byte safety limit",
                TERMINAL_PASTE_LIMIT_BYTES
            );
        }

        let inner = self.inner.lock();
        let callback_state = &inner.callback_state;
        let Some(write_pty) = callback_state.write_pty.as_ref() else {
            anyhow::bail!("terminal paste requires a WRITE_PTY callback");
        };
        let previous = callback_state
            .pending_paste_write
            .lock()
            .replace(PendingPasteWrite::Buffer(Vec::new()));
        debug_assert!(previous.is_none(), "paste writes must not nest");

        let mime = GhosttyString {
            ptr: TEXT_PLAIN_MIME.as_ptr(),
            len: TEXT_PLAIN_MIME.len(),
        };
        let mut reader = VtPasteReader {
            text: text.as_bytes(),
            served: false,
        };
        let paste = GhosttyPaste {
            size: std::mem::size_of::<GhosttyPaste>(),
            location: GhosttyClipboardLocation::Standard,
            source: match source {
                VtPasteSource::Clipboard => GhosttyPasteSource::Clipboard,
                VtPasteSource::Text => GhosttyPasteSource::Text,
            },
            mimes: &mime,
            mimes_len: 1,
            reader: GhosttyMimeReader {
                read: Some(vt_paste_read_callback),
                userdata: (&mut reader as *mut VtPasteReader<'_>).cast(),
            },
            allow_unsafe: confirm_unsafe_paste,
        };
        let mut written = false;
        let rc = unsafe { ghostty_terminal_paste(inner.terminal, &paste, &mut written) };
        let pending_write = callback_state
            .pending_paste_write
            .lock()
            .take()
            .expect("paste write capture initialized");
        match rc {
            GHOSTTY_SUCCESS => {
                let PendingPasteWrite::Buffer(output) = pending_write else {
                    anyhow::bail!("terminal paste produced too much PTY output");
                };
                if !output.is_empty() {
                    // The real host callback is a non-blocking bounded queue.
                    // Keep the VT lock through this single enqueue so the
                    // one-time Kitty grant is installed before a program can
                    // react to the event and request its clipboard payload.
                    let _write_guard = callback_state.write_order.lock();
                    write_pty(&output, PtyWriteClass::Regular).map_err(|err| {
                        anyhow::anyhow!("terminal paste failed to enqueue PTY bytes: {err}")
                    })?;
                }
                if written && !reader.served && source == VtPasteSource::Clipboard {
                    // Ghostty does not call the MIME reader for a Kitty paste
                    // event. Retain the exact user-selected text for the
                    // event's later one-time granted read.
                    *callback_state.clipboard_text.lock() = Some(Arc::from(text));
                }
                Ok(if written {
                    VtPasteResult::Accepted
                } else {
                    VtPasteResult::Empty
                })
            }
            GHOSTTY_REJECTED => Ok(VtPasteResult::RequiresConfirmation),
            GHOSTTY_IO_ERROR => {
                anyhow::bail!("ghostty_terminal_paste failed to read clipboard text")
            }
            _ => anyhow::bail!("ghostty_terminal_paste failed: rc={rc}"),
        }
    }

    pub fn clear_screen_and_scrollback(&self) {
        if self.is_alternate_screen() {
            return;
        }
        self.feed(b"\x1b[H\x1b[2J\x1b[3J");
    }

    pub fn resize(
        &self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> anyhow::Result<()> {
        let mut inner = self.inner.lock();
        // SAFETY: terminal valid.
        let rc = unsafe {
            ghostty_terminal_resize(inner.terminal, cols, rows, cell_width_px, cell_height_px)
        };
        if rc != 0 {
            anyhow::bail!("ghostty_terminal_resize failed: rc={rc}");
        }
        inner.cols = cols;
        inner.rows = rows;
        let state = &inner.callback_state;
        state.cols.store(cols, Ordering::Release);
        state.rows.store(rows, Ordering::Release);
        state
            .cell_width
            .store(cell_width_px.max(1), Ordering::Release);
        state
            .cell_height
            .store(cell_height_px.max(1), Ordering::Release);
        inner.generation = inner.generation.wrapping_add(1);
        inner.required_full_snapshot_generation = inner.generation;
        Ok(())
    }

    pub fn snapshot(&self) -> ScreenSnapshot {
        self.try_snapshot().unwrap_or_else(|| {
            let inner = self.inner.lock();
            empty_snapshot(inner.cols, inner.rows, inner.generation)
        })
    }

    pub(crate) fn try_snapshot(&self) -> Option<ScreenSnapshot> {
        let snapshot_started = perf_trace_enabled().then(Instant::now);
        let mut render = self.render.lock();
        if !render.ensure_render_state() {
            return None;
        }
        let epoch = {
            let mut inner = self.inner.lock();
            let fallback_cols = inner.cols;
            let fallback_rows = inner.rows;

            if render
                .snapshot_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.generation == inner.generation)
            {
                drop(inner);
                let metadata = render
                    .snapshot_metadata
                    .as_ref()
                    .expect("snapshot metadata checked above");
                return Some(metadata.snapshot(&render.scratch, &render.selection_ranges));
            }

            let mut active_screen = GhosttyTerminalScreen::Primary;
            let rc = unsafe {
                ghostty_terminal_get(
                    inner.terminal,
                    GhosttyTerminalData::ActiveScreen,
                    &mut active_screen as *mut _ as *mut c_void,
                )
            };
            if rc != GHOSTTY_SUCCESS {
                log::warn!("ghostty_terminal_get(ACTIVE_SCREEN) rc={rc}");
                render.force_full_snapshot = true;
                return None;
            }

            let kitty_placements = snapshot_kitty_placements(&mut inner);
            let epoch = SnapshotEpoch {
                fallback_cols,
                fallback_rows,
                kitty_placements,
                alternate_screen: active_screen == GhosttyTerminalScreen::Alternate,
                scrollbar: read_scrollbar(inner.terminal),
                generation: inner.generation,
                required_full_snapshot_generation: inner.required_full_snapshot_generation,
            };

            let rc =
                unsafe { ghostty_render_state_begin_update(render.render_state, inner.terminal) };
            if rc != GHOSTTY_SUCCESS {
                log::warn!("ghostty_render_state_begin_update rc={rc}");
                // begin_update may consume terminal dirty state before an
                // allocation fails. A new state forces the retry to rebuild
                // from the terminal instead of reusing a partial update.
                render.invalidate_render_state();
                return None;
            }
            epoch
        };

        let rc = unsafe { ghostty_render_state_end_update(render.render_state) };
        if rc != GHOSTTY_SUCCESS {
            log::warn!("ghostty_render_state_end_update rc={rc}");
            render.invalidate_render_state();
            return None;
        }

        let mut default_fg = GhosttyColorRgb {
            r: 0xCC,
            g: 0xCC,
            b: 0xCC,
        };
        let mut default_bg = GhosttyColorRgb::default();
        let mut cols = epoch.fallback_cols;
        let mut rows = epoch.fallback_rows;
        let mut state_dirty = GhosttyRenderStateDirty::False;
        let mut cursor_data = GhosttyRenderStateCursor::default();
        let render_state = render.render_state;
        let state_keys = [
            GhosttyRenderStateData::ColorForeground,
            GhosttyRenderStateData::ColorBackground,
            GhosttyRenderStateData::Cols,
            GhosttyRenderStateData::Rows,
            GhosttyRenderStateData::Dirty,
            GhosttyRenderStateData::RowIterator,
            GhosttyRenderStateData::Cursor,
        ];
        let mut state_values = [
            &mut default_fg as *mut _ as *mut c_void,
            &mut default_bg as *mut _ as *mut c_void,
            &mut cols as *mut _ as *mut c_void,
            &mut rows as *mut _ as *mut c_void,
            &mut state_dirty as *mut _ as *mut c_void,
            &mut render.row_iter as *mut _ as *mut c_void,
            &mut cursor_data as *mut _ as *mut c_void,
        ];
        let mut state_written = 0_usize;
        let rc = unsafe {
            ghostty_render_state_get_multi(
                render_state,
                state_keys.len(),
                state_keys.as_ptr(),
                state_values.as_mut_ptr(),
                &mut state_written,
            )
        };
        if rc != GHOSTTY_SUCCESS || state_written != state_keys.len() {
            log::warn!(
                "ghostty_render_state_get_multi rc={rc} written={state_written}/{}",
                state_keys.len()
            );
            render.force_full_snapshot = true;
            return None;
        }

        // Ghostty's render-state dimensions can lag the host resize by a
        // frame or two. Snapshot its actual geometry instead of inventing
        // blank tail rows from the requested host size.
        cols = cols.max(1);
        rows = rows.max(1);

        let mut full_redraw = render.force_full_snapshot
            || render.applied_full_snapshot_generation != epoch.required_full_snapshot_generation;
        if state_dirty == GhosttyRenderStateDirty::Full {
            full_redraw = true;
        }

        let total = cols as usize * rows as usize;
        if render.scratch.len() != total
            || render.scratch_cols != cols
            || render.scratch_rows != rows
        {
            render.scratch.clear();
            render.scratch.resize(total, Cell::default());
            render.selection_ranges.clear();
            render.selection_ranges.resize(rows as usize, None);
            render.scratch_cols = cols;
            render.scratch_rows = rows;
            full_redraw = true;
        }

        let mut dirty_rows: Vec<u16> = Vec::new();
        let mut next_full_row = 0_u16;
        loop {
            let row_idx = if full_redraw {
                if !unsafe { ghostty_render_state_row_iterator_next(render.row_iter) } {
                    break;
                }
                let row = next_full_row;
                next_full_row = next_full_row.saturating_add(1);
                row
            } else {
                let mut row = 0_u16;
                if !unsafe {
                    ghostty_render_state_row_iterator_next_dirty(render.row_iter, &mut row)
                } {
                    break;
                }
                row
            };
            if row_idx >= rows {
                if !full_redraw {
                    log::warn!(
                        "ghostty_render_state_row_iterator_next_dirty returned row {row_idx} for {rows} rows"
                    );
                    render.force_full_snapshot = true;
                    return None;
                }
                break;
            }

            let rc = unsafe {
                ghostty_render_state_row_get(
                    render.row_iter,
                    GhosttyRenderStateRowData::Cells,
                    &mut render.row_cells as *mut _ as *mut c_void,
                )
            };
            if rc != GHOSTTY_SUCCESS {
                log::warn!("ghostty_render_state_row_get(CELLS) rc={rc} at row {row_idx}");
                render.force_full_snapshot = true;
                return None;
            }

            let mut row_selection = GhosttyRenderStateRowSelection::default();
            let rc = unsafe {
                ghostty_render_state_row_get(
                    render.row_iter,
                    GhosttyRenderStateRowData::Selection,
                    &mut row_selection as *mut _ as *mut c_void,
                )
            };
            render.selection_ranges[row_idx as usize] = match rc {
                GHOSTTY_SUCCESS
                    if row_selection.start_x <= row_selection.end_x
                        && row_selection.end_x < cols =>
                {
                    Some(SelectionRange {
                        start: row_selection.start_x,
                        end: row_selection.end_x,
                    })
                }
                GHOSTTY_NO_VALUE => None,
                GHOSTTY_SUCCESS => {
                    log::warn!(
                        "invalid render selection range at row {row_idx}: {}..={} for {cols} cols",
                        row_selection.start_x,
                        row_selection.end_x
                    );
                    render.force_full_snapshot = true;
                    return None;
                }
                _ => {
                    log::warn!("ghostty_render_state_row_get(SELECTION) rc={rc} at row {row_idx}");
                    render.force_full_snapshot = true;
                    return None;
                }
            };

            let row_start = row_idx as usize * cols as usize;
            let mut col_idx: u16 = 0;
            // SAFETY: cells valid; `_next` returns bool.
            while unsafe { ghostty_render_state_row_cells_next(render.row_cells) } {
                if col_idx >= cols {
                    break;
                }
                let Some(cell) = read_cell(
                    render.row_cells,
                    default_fg,
                    default_bg,
                    epoch.alternate_screen,
                ) else {
                    log::warn!("failed to read render cell at row={row_idx} col={col_idx}");
                    render.force_full_snapshot = true;
                    return None;
                };
                let idx = row_start + col_idx as usize;
                render.scratch[idx] = cell;
                col_idx += 1;
            }
            if col_idx != cols {
                log::warn!(
                    "vt snapshot row ended early: row={row_idx} cells={col_idx} expected={cols}"
                );
                render.force_full_snapshot = true;
                return None;
            }

            dirty_rows.push(row_idx);
        }

        if full_redraw && next_full_row != rows {
            log::warn!(
                "vt snapshot full redraw ended early: iter_rows={next_full_row} expected_rows={rows} cols={cols}"
            );
            render.force_full_snapshot = true;
            return None;
        }

        render.force_full_snapshot = false;
        render.applied_full_snapshot_generation = epoch.required_full_snapshot_generation;

        let cursor = if cursor_data.viewport_has_value {
            Cursor {
                col: cursor_data.viewport_x,
                row: cursor_data.viewport_y,
                visible: cursor_data.visible,
            }
        } else {
            Cursor::default()
        };
        let previous_cursor = render.last_cursor;
        if previous_cursor != cursor {
            if previous_cursor.visible && previous_cursor.row < rows {
                push_unique_row(&mut dirty_rows, previous_cursor.row);
            }
            if cursor.visible && cursor.row < rows {
                push_unique_row(&mut dirty_rows, cursor.row);
            }
            render.last_cursor = cursor;
        }
        dirty_rows.sort_unstable();

        let metadata = SnapshotMetadata {
            cols,
            rows,
            kitty_placements: epoch.kitty_placements,
            dirty_rows,
            cursor,
            alternate_screen: epoch.alternate_screen,
            scrollbar: epoch.scrollbar,
            generation: epoch.generation,
        };
        let clone_started = perf_trace_enabled().then(Instant::now);
        let snapshot = metadata.snapshot(&render.scratch, &render.selection_ranges);
        let clone_elapsed_ms =
            clone_started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        render.snapshot_metadata = Some(metadata);

        if let Some(started) = snapshot_started {
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            static LAST_LOGGED_GENERATION: AtomicU64 = AtomicU64::new(u64::MAX);
            let previous = LAST_LOGGED_GENERATION.load(Ordering::Relaxed);
            let generation_changed = previous != snapshot.generation
                && LAST_LOGGED_GENERATION
                    .compare_exchange(
                        previous,
                        snapshot.generation,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .is_ok();
            let should_log = perf_trace_verbose() || generation_changed || total_ms >= 2.0;
            if should_log {
                log::info!(
                    target: "con::perf",
                    "vt_snapshot rows={} cols={} dirty_rows={} full_redraw={} cells={} clone_ms={:.3} total_ms={:.3}",
                    rows,
                    cols,
                    snapshot.dirty_rows.len(),
                    full_redraw,
                    snapshot.cells.len(),
                    clone_elapsed_ms.unwrap_or_default(),
                    total_ms
                );
            }
        }

        Some(snapshot)
    }

    pub fn size(&self) -> (u16, u16) {
        let inner = self.inner.lock();
        (inner.cols, inner.rows)
    }

    pub fn set_dark_mode(&self, dark: bool) {
        let inner = self.inner.lock();
        inner
            .callback_state
            .dark_mode
            .store(dark, Ordering::Release);
    }

    /// Returns `true` when at least one mouse-tracking mode is set
    /// (X10 / normal / button / any). Host-view mouse handlers gate
    /// mouse reporting on this so wheel / click / move don't leak
    /// escape sequences into shells that didn't ask for them.
    pub fn mouse_tracking_active(&self) -> bool {
        self.mode_active(MODE_NORMAL_MOUSE)
            || self.mode_active(MODE_BUTTON_MOUSE)
            || self.mode_active(MODE_ANY_MOUSE)
            || self.mode_active(MODE_X10_MOUSE)
    }

    /// Returns `true` when the application requested pointer motion reports.
    /// Normal (1000) and X10 tracking only receive button events; button-motion
    /// (1002) and any-motion (1003) receive drag updates.
    pub fn mouse_motion_tracking_active(&self) -> bool {
        self.mode_active(MODE_BUTTON_MOUSE) || self.mode_active(MODE_ANY_MOUSE)
    }

    /// SGR (1006) mouse format is the extended coord encoding.
    /// Callers use it to choose the report syntax; the default
    /// xterm legacy mouse report uses a different byte layout.
    pub fn is_sgr_mouse(&self) -> bool {
        self.mode_active(MODE_SGR_MOUSE)
    }

    /// Alt-screen scroll (1007): when set, mouse wheel in alt-screen
    /// apps is translated to arrow keys (up/down) rather than SGR
    /// reports. Apps like less / vim opt in.
    pub fn is_alt_scroll(&self) -> bool {
        self.mode_active(MODE_ALT_SCROLL)
    }

    /// Current viewport scrollbar state. Returns `None` when the C API
    /// query fails; callers should hide scrollbar UI in that case.
    pub fn scrollbar(&self) -> Option<GhosttyScrollbar> {
        let inner = self.inner.lock();
        if inner.terminal.is_null() {
            return None;
        }
        read_scrollbar(inner.terminal)
    }

    /// Current title reported by OSC 0/2.
    pub fn title(&self) -> Option<String> {
        self.inner.lock().title.clone()
    }

    /// Distinguishes no title report from an explicit title clear.
    pub(crate) fn reported_title(&self) -> Option<Option<String>> {
        let inner = self.inner.lock();
        inner.title_reported.then(|| inner.title.clone())
    }

    /// Current working directory reported by shell integration.
    ///
    /// OSC 7 file URIs are decoded when the callback fires; OSC 9 and OSC 1337
    /// paths are retained as reported. Returns `None` when the shell has not
    /// reported a cwd or explicitly clears it.
    pub fn current_dir(&self) -> Option<String> {
        self.inner.lock().current_dir.clone()
    }

    /// Returns `true` while the alternate screen buffer is active.
    pub fn is_alternate_screen(&self) -> bool {
        let inner = self.inner.lock();
        if inner.terminal.is_null() {
            return false;
        }
        let mut screen = GhosttyTerminalScreen::Primary;
        let rc = unsafe {
            ghostty_terminal_get(
                inner.terminal,
                GhosttyTerminalData::ActiveScreen,
                &mut screen as *mut _ as *mut c_void,
            )
        };
        rc == 0 && screen == GhosttyTerminalScreen::Alternate
    }

    pub fn prompt_state(&self) -> crate::TerminalPromptState {
        let inner = self.inner.lock();
        if inner.terminal.is_null() {
            return crate::TerminalPromptState::default();
        }
        let mut at_prompt = false;
        let rc = unsafe {
            ghostty_terminal_get(
                inner.terminal,
                GhosttyTerminalData::CursorAtPrompt,
                &mut at_prompt as *mut _ as *mut c_void,
            )
        };
        crate::TerminalPromptState {
            cursor_at_prompt: rc == GHOSTTY_SUCCESS && at_prompt,
            output_generation: inner.output_generation,
        }
    }

    /// Scroll the visible viewport by terminal rows. Negative is up;
    /// positive is down. Returns `true` when a scroll request was sent.
    pub fn scroll_viewport_delta(&self, delta_rows: isize) -> bool {
        if delta_rows == 0 {
            return false;
        }
        let mut inner = self.inner.lock();
        if inner.terminal.is_null() {
            return false;
        }
        let behavior = GhosttyTerminalScrollViewport {
            tag: GhosttyTerminalScrollViewportTag::Delta,
            value: GhosttyTerminalScrollViewportValue { delta: delta_rows },
        };
        unsafe { ghostty_terminal_scroll_viewport(inner.terminal, behavior) };
        inner.generation = inner.generation.wrapping_add(1);
        inner.required_full_snapshot_generation = inner.generation;
        true
    }

    /// Snap the visible viewport to the live tail. User input should
    /// always return a scrolled-back terminal to the prompt before it
    /// writes to the PTY, matching native terminal behavior.
    pub fn scroll_viewport_bottom(&self) -> bool {
        let mut inner = self.inner.lock();
        if inner.terminal.is_null() {
            return false;
        }
        let behavior = GhosttyTerminalScrollViewport {
            tag: GhosttyTerminalScrollViewportTag::Bottom,
            value: GhosttyTerminalScrollViewportValue { delta: 0 },
        };
        unsafe { ghostty_terminal_scroll_viewport(inner.terminal, behavior) };
        inner.generation = inner.generation.wrapping_add(1);
        inner.required_full_snapshot_generation = inner.generation;
        true
    }

    /// Bracketed-paste mode (2004). When `true`, paste operations
    /// should wrap the payload in `ESC[200~ … ESC[201~` so the shell
    /// can treat it as a single paste.
    pub fn is_bracketed_paste(&self) -> bool {
        self.mode_active(MODE_BRACKETED_PASTE)
    }

    /// DECCKM (mode 1). When `true`, arrow keys must be encoded in
    /// application-cursor form (`ESC O A/B/C/D`) rather than the
    /// default cursor form (`ESC [ A/B/C/D`).
    pub fn is_decckm(&self) -> bool {
        self.mode_active(MODE_DECCKM)
    }

    /// Generic mode query — returns `false` when the FFI call fails
    /// or the mode isn't set. Never panics.
    pub fn mode_active(&self, mode: GhosttyMode) -> bool {
        let inner = self.inner.lock();
        if inner.terminal.is_null() {
            return false;
        }
        let mut config = GhosttyTerminalModeConfig { mode, value: false };
        // SAFETY: terminal valid; `config` has the frozen C layout.
        let rc = unsafe {
            ghostty_terminal_get(
                inner.terminal,
                GhosttyTerminalData::Mode,
                &mut config as *mut _ as *mut c_void,
            )
        };
        rc == 0 && config.value
    }
}

fn refresh_terminal_metadata(inner: &mut VtInner) {
    let dirty = inner
        .callback_state
        .metadata_dirty
        .swap(0, Ordering::Relaxed);
    if dirty & METADATA_DIRTY_TITLE != 0 {
        let title = with_terminal_bytes(inner.terminal, GhosttyTerminalData::Title, |bytes| {
            (!bytes.is_empty()).then(|| String::from_utf8_lossy(bytes).into_owned())
        });
        if let Some(title) = title {
            inner.title_reported = true;
            inner.title = title;
        }
    }

    if dirty & METADATA_DIRTY_PWD != 0 {
        let current_dir = with_terminal_bytes(inner.terminal, GhosttyTerminalData::Pwd, |bytes| {
            let pwd = std::str::from_utf8(bytes).ok()?;
            if pwd.is_empty() {
                None
            } else if pwd.starts_with("file://") {
                parse_osc7_cwd(pwd)
            } else {
                Some(pwd.to_owned())
            }
        });
        if let Some(current_dir) = current_dir {
            inner.current_dir = current_dir;
        }
    }
}

fn with_terminal_bytes<R>(
    terminal: GhosttyTerminal,
    data: GhosttyTerminalData,
    read: impl FnOnce(&[u8]) -> R,
) -> Option<R> {
    let mut value = GhosttyString::default();
    let rc =
        unsafe { ghostty_terminal_get(terminal, data, (&mut value as *mut GhosttyString).cast()) };
    if rc != GHOSTTY_SUCCESS || (value.ptr.is_null() && value.len > 0) {
        return None;
    }
    if value.len == 0 {
        return Some(read(&[]));
    }
    Some(read(unsafe {
        std::slice::from_raw_parts(value.ptr, value.len)
    }))
}

unsafe extern "C" fn vt_paste_read_callback(
    userdata: *mut c_void,
    mime: GhosttyString,
    writer: GhosttyWriter,
) -> bool {
    if userdata.is_null() || !ghostty_string_eq(mime, TEXT_PLAIN_MIME) {
        return false;
    }
    let Some(write) = writer.write else {
        return false;
    };
    let reader = unsafe { &mut *(userdata as *mut VtPasteReader<'_>) };
    reader.served = true;
    if reader.text.is_empty() {
        return true;
    }
    unsafe { write(writer.userdata, reader.text.as_ptr(), reader.text.len()) }
}

unsafe extern "C" fn vt_clipboard_write_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    write: *const GhosttyClipboardWrite,
) {
    if userdata.is_null() || write.is_null() {
        return;
    }
    if unsafe { (*write).size } < std::mem::size_of::<GhosttyClipboardWrite>() {
        return;
    }

    let state = unsafe { &*(userdata as *const VtCallbackState) };
    let write = unsafe { &*write };
    if write.reply.is_none() {
        return;
    }

    let result = if !state.clipboard_write_enabled.load(Ordering::Acquire)
        || !state.clipboard_write_policy.is_enabled()
    {
        GhosttyClipboardWriteResult::Denied
    } else if write.location != GhosttyClipboardLocation::Standard {
        GhosttyClipboardWriteResult::Unsupported
    } else {
        let text = if write.contents_len == 0 {
            Ok("")
        } else if write.contents_len > 1 {
            Err(GhosttyClipboardWriteResult::Unsupported)
        } else if write.contents.is_null() {
            Err(GhosttyClipboardWriteResult::InvalidData)
        } else {
            let content = unsafe { &*write.contents };
            if !ghostty_string_is_supported_text(content.mime) {
                Err(GhosttyClipboardWriteResult::Unsupported)
            } else if content.data.len > CLIPBOARD_WRITE_LIMIT_BYTES
                || (content.data.ptr.is_null() && content.data.len > 0)
            {
                Err(GhosttyClipboardWriteResult::InvalidData)
            } else {
                let bytes = if content.data.len == 0 {
                    &[]
                } else {
                    unsafe { std::slice::from_raw_parts(content.data.ptr, content.data.len) }
                };
                std::str::from_utf8(bytes).map_err(|_| GhosttyClipboardWriteResult::InvalidData)
            }
        };
        match text {
            Err(result) => result,
            Ok(text) => {
                let mut pending = state.pending_clipboard_write.lock();
                if !state.clipboard_write_enabled.load(Ordering::Acquire)
                    || !state.clipboard_write_policy.is_enabled()
                {
                    GhosttyClipboardWriteResult::Denied
                } else {
                    *pending = Some(text.to_owned());
                    GhosttyClipboardWriteResult::Success
                }
            }
        }
    };

    reply_clipboard_write(write, result);
}

fn reply_clipboard_write(write: &GhosttyClipboardWrite, result: GhosttyClipboardWriteResult) {
    let Some(reply) = write.reply else {
        return;
    };
    let reply_value = GhosttyClipboardWriteReply {
        size: std::mem::size_of::<GhosttyClipboardWriteReply>(),
        result,
        remember: false,
    };
    unsafe { reply(write, &reply_value) };
}

/// Clipboard reads initiated by a running program are denied by default.
/// The only data-bearing read accepted here is a one-time Kitty paste grant
/// minted by `ghostty_terminal_paste` after an explicit local paste action.
/// This callback never enters GPUI or the system clipboard while the VT lock
/// is held, avoiding a reader-thread/UI-thread lock inversion.
unsafe extern "C" fn vt_clipboard_read_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    read: *const GhosttyClipboardRead,
) {
    if userdata.is_null() || read.is_null() {
        return;
    }
    if unsafe { (*read).size } < std::mem::size_of::<GhosttyClipboardRead>() {
        return;
    }
    let state = unsafe { &*(userdata as *const VtCallbackState) };
    let read = unsafe { &*read };
    let Some(reply) = read.reply else {
        return;
    };

    let mut result = GhosttyClipboardReadResult::Success;
    let mut available = GhosttyString::default();
    let mut available_len = 0;
    let mut content = GhosttyClipboardContent {
        mime: GhosttyString::default(),
        data: GhosttyString::default(),
    };
    let mut contents_len = 0;

    // Do not hold a Rust mutex across the foreign reply callback: libghostty
    // may synchronously advance another effect while consuming this payload.
    // Arc keeps the bytes stable for the callback without copying the paste.
    let clipboard_text = state.clipboard_text.lock().clone();
    let listing_only = read.list && read.mimes_len == 0;
    if read.location != GhosttyClipboardLocation::Standard {
        result = GhosttyClipboardReadResult::Unsupported;
    } else if !listing_only && !read.granted {
        result = GhosttyClipboardReadResult::Denied;
    } else if let Some(text) = clipboard_text.as_deref() {
        if read.list {
            available = GhosttyString {
                ptr: TEXT_PLAIN_MIME.as_ptr(),
                len: TEXT_PLAIN_MIME.len(),
            };
            available_len = 1;
        }

        if read.mimes_len > 0 {
            if read.mimes.is_null() {
                result = GhosttyClipboardReadResult::IoError;
            } else {
                let requested = unsafe { std::slice::from_raw_parts(read.mimes, read.mimes_len) };
                if let Some(mime) = requested
                    .iter()
                    .copied()
                    .find(|mime| ghostty_string_is_supported_text(*mime))
                {
                    content = GhosttyClipboardContent {
                        mime,
                        data: GhosttyString {
                            ptr: text.as_ptr(),
                            len: text.len(),
                        },
                    };
                    contents_len = 1;
                }
            }
        }
    }

    let reply_value = GhosttyClipboardReadReply {
        size: std::mem::size_of::<GhosttyClipboardReadReply>(),
        result,
        contents: if contents_len == 0 {
            std::ptr::null()
        } else {
            &content
        },
        contents_len,
        available: if available_len == 0 {
            std::ptr::null()
        } else {
            &available
        },
        available_len,
        remember: false,
    };
    unsafe { reply(read, &reply_value) };
}

fn ghostty_string_eq(value: GhosttyString, expected: &[u8]) -> bool {
    if value.len != expected.len() || (value.ptr.is_null() && value.len > 0) {
        return false;
    }
    let bytes = if value.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(value.ptr, value.len) }
    };
    bytes == expected
}

fn ghostty_string_is_supported_text(value: GhosttyString) -> bool {
    if value.ptr.is_null() && value.len > 0 {
        return false;
    }
    let bytes = if value.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(value.ptr, value.len) }
    };
    crate::clipboard_mime_is_text(bytes)
}

unsafe extern "C" fn vt_write_pty_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    if userdata.is_null() || data.is_null() || len == 0 {
        return;
    }

    let state = unsafe { &*(userdata as *const VtCallbackState) };
    let Some(write_pty) = state.write_pty.as_ref() else {
        return;
    };
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    {
        let mut pending = state.pending_paste_write.lock();
        if let Some(pending) = pending.as_mut() {
            pending.append(bytes);
            return;
        }
    }

    let _write_guard = state.write_order.lock();
    if let Err(err) = write_pty(bytes, PtyWriteClass::ReservedControl) {
        mark_control_write_failed(&state.write_failed, &err);
    }
}

unsafe extern "C" fn vt_bell_callback(_terminal: GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &*(userdata as *const VtCallbackState) };
    state.bell_pending.store(true, Ordering::Relaxed);
}

unsafe extern "C" fn vt_desktop_notification_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const GhosttyTerminalDesktopNotification,
) {
    if userdata.is_null() || notification.is_null() {
        return;
    }
    if unsafe { notification.cast::<usize>().read_unaligned() }
        < std::mem::size_of::<GhosttyTerminalDesktopNotification>()
    {
        return;
    }

    let notification = unsafe { &*notification };
    let Some(title) = (unsafe { ghostty_string_bytes(&notification.title) }) else {
        return;
    };
    let Some(body) = (unsafe { ghostty_string_bytes(&notification.body) }) else {
        return;
    };
    let state = unsafe { &*(userdata as *const VtCallbackState) };
    state.desktop_notification_policy.push(title, body);
}

unsafe fn ghostty_string_bytes(value: &GhosttyString) -> Option<&[u8]> {
    if value.ptr.is_null() && value.len > 0 {
        return None;
    }
    Some(if value.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(value.ptr, value.len) }
    })
}

unsafe extern "C" fn vt_progress_report_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    report: *const GhosttyTerminalProgressReport,
) {
    if userdata.is_null() || report.is_null() {
        return;
    }
    if unsafe { report.cast::<usize>().read_unaligned() }
        < std::mem::size_of::<GhosttyTerminalProgressReport>()
    {
        return;
    }
    let report = unsafe { &*report };
    let Some(progress) = TerminalProgress::from_ghostty_report(report.state, report.progress)
    else {
        return;
    };
    let state = unsafe { &*(userdata as *const VtCallbackState) };
    state.progress.store(
        encode_timed_terminal_progress(progress, terminal_progress_tick(&state.progress_epoch)),
        Ordering::Relaxed,
    );
}

fn encode_terminal_progress(progress: Option<TerminalProgress>) -> u16 {
    let (state, percent) = match progress {
        None => return 0,
        Some(TerminalProgress::Running(percent)) => (1, percent),
        Some(TerminalProgress::Error(percent)) => (2, percent),
        Some(TerminalProgress::Indeterminate) => (3, None),
        Some(TerminalProgress::Paused(percent)) => (4, percent),
    };
    (state << 8) | percent.map_or(0, |percent| u16::from(percent) + 1)
}

fn terminal_progress_tick(epoch: &Instant) -> u64 {
    const MAX_TICK: u64 = u64::MAX >> u16::BITS;
    u64::try_from(epoch.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .min(MAX_TICK - 1)
        + 1
}

fn encode_timed_terminal_progress(progress: Option<TerminalProgress>, tick: u64) -> u64 {
    let progress = encode_terminal_progress(progress);
    if progress == 0 {
        0
    } else {
        (tick << u16::BITS) | u64::from(progress)
    }
}

fn decode_timed_terminal_progress(value: u64, now: u64) -> Option<TerminalProgress> {
    let updated_at = value >> u16::BITS;
    if updated_at == 0
        || now.saturating_sub(updated_at)
            >= u64::try_from(TERMINAL_PROGRESS_TIMEOUT.as_millis()).unwrap_or(u64::MAX)
    {
        return None;
    }
    decode_terminal_progress(value as u16)
}

fn decode_terminal_progress(value: u16) -> Option<TerminalProgress> {
    let percent = match value & 0xff {
        0 => None,
        value => Some((value - 1) as u8),
    };
    match value >> 8 {
        1 => Some(TerminalProgress::Running(percent)),
        2 => Some(TerminalProgress::Error(percent)),
        3 => Some(TerminalProgress::Indeterminate),
        4 => Some(TerminalProgress::Paused(percent)),
        _ => None,
    }
}

unsafe extern "C" fn vt_unknown_sequence_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    sequence: *const GhosttyTerminalUnknownSequence,
) {
    if userdata.is_null()
        || sequence.is_null()
        || !log::log_enabled!(target: UNKNOWN_SEQUENCE_LOG_TARGET, log::Level::Debug)
    {
        return;
    }

    let state = unsafe { &*(userdata as *const VtCallbackState) };
    let Ok(log_index) = state.unknown_sequence_log_count.fetch_update(
        Ordering::Relaxed,
        Ordering::Relaxed,
        |count| (count <= UNKNOWN_SEQUENCE_LOG_LIMIT).then_some(count + 1),
    ) else {
        return;
    };
    if log_index == UNKNOWN_SEQUENCE_LOG_LIMIT {
        log::debug!(
            target: UNKNOWN_SEQUENCE_LOG_TARGET,
            "further unknown terminal sequences suppressed"
        );
        return;
    }

    let sequence = unsafe { &*sequence };
    if sequence.tag != GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_APC {
        log::debug!(
            target: UNKNOWN_SEQUENCE_LOG_TARGET,
            "unknown terminal sequence tag={}",
            sequence.tag
        );
        return;
    }

    let apc = unsafe { sequence.value.apc };
    if apc.content.ptr.is_null() && apc.content.len != 0 {
        log::debug!(
            target: UNKNOWN_SEQUENCE_LOG_TARGET,
            "unknown APC has invalid content pointer len={} truncated={}",
            apc.content.len,
            apc.truncated
        );
        return;
    }
    let content = if apc.content.len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(apc.content.ptr, apc.content.len) }
    };
    log::debug!(
        target: UNKNOWN_SEQUENCE_LOG_TARGET,
        "unknown APC len={} truncated={} content={content:02x?}",
        content.len(),
        apc.truncated
    );
}

unsafe extern "C" fn vt_title_changed_callback(_terminal: GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &*(userdata as *const VtCallbackState) };
    state
        .metadata_dirty
        .fetch_or(METADATA_DIRTY_TITLE, Ordering::Relaxed);
}

unsafe extern "C" fn vt_pwd_changed_callback(_terminal: GhosttyTerminal, userdata: *mut c_void) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &*(userdata as *const VtCallbackState) };
    state
        .metadata_dirty
        .fetch_or(METADATA_DIRTY_PWD, Ordering::Relaxed);
}

fn mark_control_write_failed(write_failed: &AtomicBool, err: &std::io::Error) {
    if !write_failed.swap(true, Ordering::AcqRel) {
        log::error!("terminal control write failed; closing the desynchronized session: {err}");
    }
}

unsafe extern "C" fn vt_enquiry_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
) -> GhosttyString {
    if userdata.is_null() {
        return GhosttyString {
            ptr: std::ptr::null(),
            len: 0,
        };
    }

    let state = unsafe { &*(userdata as *const VtCallbackState) };
    GhosttyString {
        ptr: state.enquiry_response.as_ptr(),
        len: state.enquiry_response.len(),
    }
}

unsafe extern "C" fn vt_size_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_size: *mut GhosttySizeReportSize,
) -> bool {
    if userdata.is_null() || out_size.is_null() {
        return false;
    }

    let state = unsafe { &*(userdata as *const VtCallbackState) };
    unsafe {
        *out_size = GhosttySizeReportSize {
            rows: state.rows.load(Ordering::Acquire).max(1),
            columns: state.cols.load(Ordering::Acquire).max(1),
            cell_width: state.cell_width.load(Ordering::Acquire).max(1),
            cell_height: state.cell_height.load(Ordering::Acquire).max(1),
        };
    }
    true
}

unsafe extern "C" fn vt_color_scheme_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_scheme: *mut GhosttyColorScheme,
) -> bool {
    if userdata.is_null() || out_scheme.is_null() {
        return false;
    }

    let state = unsafe { &*(userdata as *const VtCallbackState) };
    unsafe {
        *out_scheme = if state.dark_mode.load(Ordering::Acquire) {
            GhosttyColorScheme::Dark
        } else {
            GhosttyColorScheme::Light
        };
    }
    true
}

unsafe extern "C" fn vt_device_attributes_callback(
    _terminal: GhosttyTerminal,
    userdata: *mut c_void,
    out_attrs: *mut GhosttyDeviceAttributes,
) -> bool {
    if userdata.is_null() || out_attrs.is_null() {
        return false;
    }

    let state = unsafe { &*(userdata as *const VtCallbackState) };
    unsafe {
        *out_attrs = state.device_attributes;
    }
    true
}

unsafe extern "C" fn vt_xtversion_callback(
    _terminal: GhosttyTerminal,
    _userdata: *mut c_void,
) -> GhosttyString {
    GhosttyString {
        ptr: std::ptr::null(),
        len: 0,
    }
}

fn validate_selection_point(point: SelectionPoint) -> anyhow::Result<()> {
    if !point.surface_x_px.is_finite() || !point.surface_y_px.is_finite() {
        anyhow::bail!("selection surface position must be finite");
    }
    Ok(())
}

fn selection_surface_position(point: SelectionPoint) -> GhosttySurfacePosition {
    GhosttySurfacePosition {
        x: point.surface_x_px,
        y: point.surface_y_px,
    }
}

fn selection_grid_ref(
    terminal: GhosttyTerminal,
    col: u16,
    row: u16,
) -> anyhow::Result<GhosttyGridRef> {
    let point = GhosttyPoint {
        tag: GhosttyPointTag::Viewport,
        value: GhosttyPointValue {
            coordinate: GhosttyPointCoordinate {
                x: col,
                y: u32::from(row),
            },
        },
    };
    let mut grid_ref = GhosttyGridRef::default();
    let rc = unsafe { ghostty_terminal_grid_ref(terminal, point, &mut grid_ref) };
    if rc != GHOSTTY_SUCCESS {
        anyhow::bail!("ghostty_terminal_grid_ref(VIEWPORT {col},{row}) failed: rc={rc}");
    }
    Ok(grid_ref)
}

fn selection_event_set<T>(
    event: GhosttySelectionGestureEvent,
    option: GhosttySelectionGestureEventOption,
    value: &T,
) -> anyhow::Result<()> {
    let rc = unsafe {
        ghostty_selection_gesture_event_set(event, option, value as *const T as *const c_void)
    };
    if rc != GHOSTTY_SUCCESS {
        anyhow::bail!("ghostty_selection_gesture_event_set({option:?}) failed: rc={rc}");
    }
    Ok(())
}

fn apply_selection_gesture_event(
    gesture: GhosttySelectionGesture,
    terminal: GhosttyTerminal,
    event: GhosttySelectionGestureEvent,
) -> anyhow::Result<Option<GhosttySelection>> {
    let mut selection = GhosttySelection::default();
    let rc = unsafe { ghostty_selection_gesture_event(gesture, terminal, event, &mut selection) };
    match rc {
        GHOSTTY_SUCCESS => Ok(Some(selection)),
        GHOSTTY_NO_VALUE => Ok(None),
        _ => anyhow::bail!("ghostty_selection_gesture_event failed: rc={rc}"),
    }
}

fn selection_gesture_click_count(inner: &VtInner) -> anyhow::Result<u8> {
    let Some(state) = inner.selection_gesture.as_ref() else {
        return Ok(0);
    };
    let mut click_count = 0_u8;
    let rc = unsafe {
        ghostty_selection_gesture_get(
            state.gesture,
            inner.terminal,
            GhosttySelectionGestureData::ClickCount,
            &mut click_count as *mut _ as *mut c_void,
        )
    };
    if rc != GHOSTTY_SUCCESS {
        anyhow::bail!("ghostty_selection_gesture_get(CLICK_COUNT) failed: rc={rc}");
    }
    Ok(click_count)
}

fn selection_gesture_autoscroll(inner: &VtInner) -> anyhow::Result<SelectionAutoscroll> {
    let Some(state) = inner.selection_gesture.as_ref() else {
        return Ok(SelectionAutoscroll::None);
    };
    // Read C enum output through its integer representation. Writing an
    // upstream value added after our pinned revision directly into a Rust enum
    // would create an invalid discriminant before we could validate it.
    let mut autoscroll = GhosttySelectionGestureAutoscroll::None as c_int;
    let rc = unsafe {
        ghostty_selection_gesture_get(
            state.gesture,
            inner.terminal,
            GhosttySelectionGestureData::Autoscroll,
            &mut autoscroll as *mut _ as *mut c_void,
        )
    };
    if rc != GHOSTTY_SUCCESS {
        anyhow::bail!("ghostty_selection_gesture_get(AUTOSCROLL) failed: rc={rc}");
    }
    match autoscroll {
        value if value == GhosttySelectionGestureAutoscroll::None as c_int => {
            Ok(SelectionAutoscroll::None)
        }
        value if value == GhosttySelectionGestureAutoscroll::Up as c_int => {
            Ok(SelectionAutoscroll::Up)
        }
        value if value == GhosttySelectionGestureAutoscroll::Down as c_int => {
            Ok(SelectionAutoscroll::Down)
        }
        value => anyhow::bail!("unknown Ghostty selection autoscroll value: {value}"),
    }
}

fn selection_locked(inner: &VtInner) -> anyhow::Result<Option<GhosttySelection>> {
    let mut selection = GhosttySelection::default();
    let rc = unsafe {
        ghostty_terminal_get(
            inner.terminal,
            GhosttyTerminalData::Selection,
            &mut selection as *mut _ as *mut c_void,
        )
    };
    match rc {
        GHOSTTY_SUCCESS => Ok(Some(selection)),
        GHOSTTY_NO_VALUE => Ok(None),
        _ => anyhow::bail!("ghostty_terminal_get(SELECTION) failed: rc={rc}"),
    }
}

fn has_selection_locked(inner: &VtInner) -> anyhow::Result<bool> {
    Ok(selection_locked(inner)?.is_some())
}

fn set_selection_locked(
    inner: &mut VtInner,
    selection: Option<&GhosttySelection>,
) -> anyhow::Result<bool> {
    let current = selection_locked(inner)?;
    match (selection, current.as_ref()) {
        (None, None) => return Ok(false),
        (Some(selection), Some(current)) => {
            let mut equal = false;
            let rc = unsafe {
                ghostty_terminal_selection_equal(inner.terminal, selection, current, &mut equal)
            };
            if rc != GHOSTTY_SUCCESS {
                anyhow::bail!("ghostty_terminal_selection_equal failed: rc={rc}");
            }
            if equal {
                return Ok(false);
            }
        }
        _ => {}
    }
    let value = selection.map_or(std::ptr::null(), |selection| {
        selection as *const GhosttySelection as *const c_void
    });
    let rc =
        unsafe { ghostty_terminal_set(inner.terminal, GhosttyTerminalOption::Selection, value) };
    if rc != GHOSTTY_SUCCESS {
        anyhow::bail!("ghostty_terminal_set(SELECTION) failed: rc={rc}");
    }
    inner.generation = inner.generation.wrapping_add(1);
    Ok(true)
}

fn selection_text_locked(inner: &VtInner) -> anyhow::Result<Option<String>> {
    let options = GhosttyTerminalSelectionFormatOptions::default();
    let mut ptr = std::ptr::null_mut();
    let mut len = 0_usize;
    let rc = unsafe {
        ghostty_terminal_selection_format_alloc(
            inner.terminal,
            std::ptr::null(),
            options,
            &mut ptr,
            &mut len,
        )
    };
    match rc {
        GHOSTTY_NO_VALUE => return Ok(None),
        GHOSTTY_SUCCESS => {}
        _ => anyhow::bail!("ghostty_terminal_selection_format_alloc failed: rc={rc}"),
    }
    if ptr.is_null() && len != 0 {
        anyhow::bail!("ghostty selection formatter returned a null {len}-byte buffer");
    }
    let bytes = (|| {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len)?;
        if len != 0 {
            // SAFETY: format_alloc returned a readable `len`-byte allocation.
            // Copy it because Ghostty and Rust may use different heaps on
            // Windows; fallible reservation keeps the FFI buffer releasable if
            // Rust cannot allocate the destination.
            bytes.extend_from_slice(unsafe { std::slice::from_raw_parts(ptr, len) });
        }
        Ok::<_, anyhow::Error>(bytes)
    })();
    unsafe { ghostty_free(std::ptr::null(), ptr, len) };
    Ok(Some(String::from_utf8(bytes?)?))
}

fn empty_snapshot(cols: u16, rows: u16, generation: u64) -> ScreenSnapshot {
    ScreenSnapshot {
        cols,
        rows,
        cells: Vec::new(),
        selection_ranges: Vec::new(),
        kitty_placements: Arc::from([]),
        dirty_rows: Vec::new(),
        cursor: Cursor::default(),
        alternate_screen: false,
        scrollbar: None,
        title: None,
        generation,
    }
}

fn snapshot_kitty_placements(inner: &mut VtInner) -> Arc<[KittyPlacement]> {
    if inner.kitty_snapshot_generation == inner.generation {
        return inner.kitty_placements.clone();
    }

    let mut graphics: GhosttyKittyGraphics = std::ptr::null_mut();
    let rc = unsafe {
        ghostty_terminal_get(
            inner.terminal,
            GhosttyTerminalData::KittyGraphics,
            &mut graphics as *mut _ as *mut c_void,
        )
    };
    if rc != GHOSTTY_SUCCESS || inner.kitty_placement_iter.is_null() {
        log::debug!("could not read Kitty graphics state for the current terminal generation");
        return inner.kitty_placements.clone();
    }
    if graphics.is_null() {
        inner.kitty_image_cache.clear();
        inner.kitty_placements = Arc::from([]);
        inner.kitty_snapshot_generation = inner.generation;
        return inner.kitty_placements.clone();
    }

    let rc = unsafe {
        ghostty_kitty_graphics_get(
            graphics,
            GhosttyKittyGraphicsData::PlacementIterator,
            &mut inner.kitty_placement_iter as *mut _ as *mut c_void,
        )
    };
    if rc != GHOSTTY_SUCCESS {
        log::debug!("could not reset the Kitty placement iterator; retaining the prior snapshot");
        return inner.kitty_placements.clone();
    }

    let mut images: HashMap<u32, Arc<KittyImage>> = HashMap::new();
    let mut placements = Vec::new();
    let mut scanned = 0_usize;
    while unsafe { ghostty_kitty_graphics_placement_next(inner.kitty_placement_iter) } {
        if scanned >= KITTY_PLACEMENT_SCAN_LIMIT {
            log::debug!("stopping Kitty placement scan after {KITTY_PLACEMENT_SCAN_LIMIT} entries");
            break;
        }
        scanned += 1;

        let mut image_id = 0_u32;
        let mut placement_id = 0_u32;
        let mut is_virtual = false;
        let mut cell_x_offset = 0_u32;
        let mut cell_y_offset = 0_u32;
        let mut z = 0_i32;
        let keys = [
            GhosttyKittyGraphicsPlacementData::ImageId,
            GhosttyKittyGraphicsPlacementData::PlacementId,
            GhosttyKittyGraphicsPlacementData::IsVirtual,
            GhosttyKittyGraphicsPlacementData::XOffset,
            GhosttyKittyGraphicsPlacementData::YOffset,
            GhosttyKittyGraphicsPlacementData::Z,
        ];
        let mut values = [
            &mut image_id as *mut _ as *mut c_void,
            &mut placement_id as *mut _ as *mut c_void,
            &mut is_virtual as *mut _ as *mut c_void,
            &mut cell_x_offset as *mut _ as *mut c_void,
            &mut cell_y_offset as *mut _ as *mut c_void,
            &mut z as *mut _ as *mut c_void,
        ];
        let rc = unsafe {
            ghostty_kitty_graphics_placement_get_multi(
                inner.kitty_placement_iter,
                keys.len(),
                keys.as_ptr(),
                values.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if rc != GHOSTTY_SUCCESS || is_virtual {
            continue;
        }

        let image_handle = unsafe { ghostty_kitty_graphics_image(graphics, image_id) };
        if image_handle.is_null() {
            continue;
        }
        let mut info = GhosttyKittyGraphicsPlacementRenderInfo {
            size: std::mem::size_of::<GhosttyKittyGraphicsPlacementRenderInfo>(),
            ..GhosttyKittyGraphicsPlacementRenderInfo::default()
        };
        let rc = unsafe {
            ghostty_kitty_graphics_placement_render_info(
                inner.kitty_placement_iter,
                image_handle,
                inner.terminal,
                &mut info,
            )
        };
        if rc != GHOSTTY_SUCCESS
            || !info.viewport_visible
            || info.pixel_width == 0
            || info.pixel_height == 0
            || info.source_width == 0
            || info.source_height == 0
        {
            continue;
        }
        if placements.len() >= KITTY_PLACEMENT_SNAPSHOT_LIMIT {
            log::debug!(
                "truncating Kitty graphics snapshot at {KITTY_PLACEMENT_SNAPSHOT_LIMIT} visible placements"
            );
            break;
        }

        let image = if let Some(image) = images.get(&image_id) {
            image.clone()
        } else {
            let Some(image) = snapshot_kitty_image(
                image_handle,
                image_id,
                inner.kitty_image_cache.get(&image_id),
            ) else {
                continue;
            };
            images.insert(image_id, image.clone());
            image
        };
        placements.push(KittyPlacement {
            image,
            placement_id,
            z,
            viewport_col: info.viewport_col,
            viewport_row: info.viewport_row,
            cell_x_offset,
            cell_y_offset,
            pixel_width: info.pixel_width,
            pixel_height: info.pixel_height,
            source_x: info.source_x,
            source_y: info.source_y,
            source_width: info.source_width,
            source_height: info.source_height,
        });
    }

    // Draw lower z-index placements first. `sort_by_key` is stable, preserving
    // libghostty's iterator order for equal z values where the C API exposes no
    // additional ordering key.
    placements.sort_by_key(|placement| placement.z);
    inner.kitty_image_cache = images;
    inner.kitty_placements = placements.into();
    inner.kitty_snapshot_generation = inner.generation;
    inner.kitty_placements.clone()
}

fn snapshot_kitty_image(
    image: GhosttyKittyGraphicsImage,
    image_id: u32,
    cached: Option<&Arc<KittyImage>>,
) -> Option<Arc<KittyImage>> {
    let mut generation = 0_u64;
    let rc = unsafe {
        ghostty_kitty_graphics_image_get(
            image,
            GhosttyKittyGraphicsImageData::Generation,
            &mut generation as *mut _ as *mut c_void,
        )
    };
    if rc != GHOSTTY_SUCCESS || generation == 0 {
        return None;
    }
    if let Some(cached) = cached.filter(|cached| cached.generation == generation) {
        return Some(cached.clone());
    }

    let mut width = 0_u32;
    let mut height = 0_u32;
    let mut format_raw = GhosttyKittyImageFormat::Rgba as c_int;
    let mut compression_raw = GhosttyKittyImageCompression::None as c_int;
    let mut data_ptr: *const u8 = std::ptr::null();
    let mut data_len = 0_usize;
    let keys = [
        GhosttyKittyGraphicsImageData::Width,
        GhosttyKittyGraphicsImageData::Height,
        GhosttyKittyGraphicsImageData::Format,
        GhosttyKittyGraphicsImageData::Compression,
        GhosttyKittyGraphicsImageData::DataPtr,
        GhosttyKittyGraphicsImageData::DataLen,
    ];
    let mut values = [
        &mut width as *mut _ as *mut c_void,
        &mut height as *mut _ as *mut c_void,
        &mut format_raw as *mut _ as *mut c_void,
        &mut compression_raw as *mut _ as *mut c_void,
        &mut data_ptr as *mut _ as *mut c_void,
        &mut data_len as *mut _ as *mut c_void,
    ];
    let rc = unsafe {
        ghostty_kitty_graphics_image_get_multi(
            image,
            keys.len(),
            keys.as_ptr(),
            values.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if rc != GHOSTTY_SUCCESS
        || width == 0
        || height == 0
        || data_ptr.is_null()
        || compression_raw != GhosttyKittyImageCompression::None as c_int
    {
        return None;
    }

    // Read C enums through their integer representation so a future upstream
    // format cannot create an invalid Rust enum discriminant before the ABI
    // manifest check reports the drift.
    let format = match format_raw {
        value if value == GhosttyKittyImageFormat::Rgb as c_int => GhosttyKittyImageFormat::Rgb,
        value if value == GhosttyKittyImageFormat::Rgba as c_int => GhosttyKittyImageFormat::Rgba,
        value if value == GhosttyKittyImageFormat::GrayAlpha as c_int => {
            GhosttyKittyImageFormat::GrayAlpha
        }
        value if value == GhosttyKittyImageFormat::Gray as c_int => GhosttyKittyImageFormat::Gray,
        _ => return None,
    };

    let bytes_per_pixel = match format {
        GhosttyKittyImageFormat::Rgb => 3,
        GhosttyKittyImageFormat::Rgba => 4,
        GhosttyKittyImageFormat::GrayAlpha => 2,
        GhosttyKittyImageFormat::Gray => 1,
        GhosttyKittyImageFormat::Png => return None,
    };
    let expected_len = usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?
        .checked_mul(bytes_per_pixel)?;
    if data_len != expected_len || data_len > KITTY_IMAGE_STORAGE_LIMIT_BYTES as usize {
        return None;
    }
    let source = unsafe { std::slice::from_raw_parts(data_ptr, data_len) };
    let rgba: Arc<[u8]> = match format {
        GhosttyKittyImageFormat::Rgba => Arc::from(source),
        GhosttyKittyImageFormat::Rgb => {
            let mut rgba = Vec::with_capacity(source.len() / 3 * 4);
            for &[red, green, blue] in source.as_chunks::<3>().0 {
                rgba.extend_from_slice(&[red, green, blue, 0xFF]);
            }
            rgba.into()
        }
        GhosttyKittyImageFormat::GrayAlpha => {
            let mut rgba = Vec::with_capacity(source.len() / 2 * 4);
            for &[gray, alpha] in source.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[gray, gray, gray, alpha]);
            }
            rgba.into()
        }
        GhosttyKittyImageFormat::Gray => {
            let mut rgba = Vec::with_capacity(source.len() * 4);
            for gray in source {
                rgba.extend_from_slice(&[*gray, *gray, *gray, 0xFF]);
            }
            rgba.into()
        }
        GhosttyKittyImageFormat::Png => return None,
    };

    Some(Arc::new(KittyImage {
        id: image_id,
        generation,
        width,
        height,
        rgba,
    }))
}

fn read_scrollbar(terminal: GhosttyTerminal) -> Option<GhosttyScrollbar> {
    if terminal.is_null() {
        return None;
    }
    let mut scrollbar = GhosttyTerminalScrollbar::default();
    let rc = unsafe {
        ghostty_terminal_get(
            terminal,
            GhosttyTerminalData::Scrollbar,
            &mut scrollbar as *mut _ as *mut c_void,
        )
    };
    (rc == 0).then_some(GhosttyScrollbar {
        total: scrollbar.total,
        offset: scrollbar.offset,
        len: scrollbar.len,
    })
}

fn push_unique_row(rows: &mut Vec<u16>, row: u16) {
    if !rows.contains(&row) {
        rows.push(row);
    }
}

fn read_cell(
    cells: GhosttyRowCells,
    default_fg: GhosttyColorRgb,
    default_bg: GhosttyColorRgb,
    alternate_screen: bool,
) -> Option<Cell> {
    // RAW here is an **opaque `GhosttyCell` u64 snapshot**, not a packed
    // codepoint. Decode fields via `ghostty_cell_get(cell, KEY, &out)`
    // per `screen.h`. Previous code bitshifted RAW directly and produced
    // nonsense codepoints (U+015C etc. for the "PowerShell" banner).
    let mut raw: GhosttyCell = 0;
    let mut style = GhosttyStyle::new();
    let mut fg = default_fg;
    let mut bg = default_bg;
    let keys = [
        GhosttyRenderStateRowCellsData::Raw,
        GhosttyRenderStateRowCellsData::Style,
        GhosttyRenderStateRowCellsData::FgColor,
        GhosttyRenderStateRowCellsData::BgColor,
    ];
    let mut values = [
        &mut raw as *mut _ as *mut c_void,
        &mut style as *mut _ as *mut c_void,
        &mut fg as *mut _ as *mut c_void,
        &mut bg as *mut _ as *mut c_void,
    ];
    let mut written = 0_usize;
    let rc = unsafe {
        ghostty_render_state_row_cells_get_multi(
            cells,
            keys.len(),
            keys.as_ptr(),
            values.as_mut_ptr(),
            &mut written,
        )
    };
    let bg_was_default = match (rc, written) {
        (GHOSTTY_SUCCESS, 4) => false,
        (GHOSTTY_INVALID_VALUE, 2) => {
            let rc = unsafe {
                ghostty_render_state_row_cells_get(
                    cells,
                    GhosttyRenderStateRowCellsData::BgColor,
                    &mut bg as *mut _ as *mut c_void,
                )
            };
            match rc {
                GHOSTTY_SUCCESS => false,
                GHOSTTY_INVALID_VALUE => true,
                _ => return None,
            }
        }
        (GHOSTTY_INVALID_VALUE, 3) => true,
        _ => return None,
    };

    // Gate codepoint decode on HAS_TEXT — blank cells carry a bogus
    // grapheme-tag codepoint we'd otherwise rasterize.
    let mut has_text: bool = false;
    let mut codepoint: u32 = 0;
    let cell_keys = [GhosttyCellData::HasText, GhosttyCellData::Codepoint];
    let mut cell_values = [
        &mut has_text as *mut _ as *mut c_void,
        &mut codepoint as *mut _ as *mut c_void,
    ];
    let mut cell_written = 0_usize;
    let rc = unsafe {
        ghostty_cell_get_multi(
            raw,
            cell_keys.len(),
            cell_keys.as_ptr(),
            cell_values.as_mut_ptr(),
            &mut cell_written,
        )
    };
    if rc != GHOSTTY_SUCCESS || cell_written != cell_keys.len() {
        return None;
    }
    if !has_text {
        codepoint = 0;
    }

    const STYLE_COLOR_TAG_PALETTE: u32 = 1;
    const PALETTE_BLACK: u8 = 0;
    // Curses-style full-screen apps often paint their canvas with SGR
    // 40 instead of default background. For themes whose ANSI black is
    // a raised surface (Catppuccin, Nord, Everforest), that makes htop
    // and similar TUIs look like a detached slab. Normalize only
    // alternate-screen background palette index 0 to the terminal
    // canvas; normal shell output, foreground black, RGB black, and
    // xterm-cube black (palette index 16) remain untouched.
    let bg_is_alt_canvas_black = alternate_screen
        && style.bg_color.tag == STYLE_COLOR_TAG_PALETTE
        && style.bg_color.value as u8 == PALETTE_BLACK;
    let bg = if bg_was_default || bg_is_alt_canvas_black {
        default_bg
    } else {
        bg
    };

    // Pack RGB into the 0xRRGGBBAA u32 our HLSL `unpackRGBA` expects
    // (high byte = R, low byte = A). Default-bg cells carry alpha=0
    // as a sentinel so the renderer can apply pane background opacity
    // while explicit SGR backgrounds stay solid.
    let pack = |c: GhosttyColorRgb, a: u8| -> u32 {
        ((c.r as u32) << 24) | ((c.g as u32) << 16) | ((c.b as u32) << 8) | (a as u32)
    };
    let bg_alpha: u8 = if bg_was_default || bg_is_alt_canvas_black {
        0
    } else {
        0xFF
    };

    // Pack style flags into the attrs byte the renderer (and HLSL
    // pixel shader) interpret. Underline is an `int` upstream
    // (0 = none, 1 = single, 2 = double, 3 = curly, ...); any non-zero
    // value enables our single underline rendering for now.
    let mut attrs: u8 = 0;
    if style.bold {
        attrs |= ATTR_BOLD;
    }
    if style.italic {
        attrs |= ATTR_ITALIC;
    }
    if style.underline != 0 {
        attrs |= ATTR_UNDERLINE;
    }
    if style.strikethrough {
        attrs |= ATTR_STRIKE;
    }
    if style.inverse {
        attrs |= ATTR_INVERSE;
    }

    Some(Cell {
        codepoint,
        fg: pack(fg, 0xFF),
        bg: pack(bg, bg_alpha),
        attrs,
        _pad: [0; 3],
    })
}

impl Drop for VtScreen {
    fn drop(&mut self) {
        if let Some(mutex) = Arc::get_mut(&mut self.inner) {
            let inner = mutex.get_mut();
            let render = self.render.get_mut();
            // Free every object that can refer to the terminal before the
            // terminal itself.
            // SAFETY: unique owner via Arc::get_mut.
            unsafe { inner.mouse.free() };
            if !inner.key_event.is_null() {
                unsafe { ghostty_key_event_free(inner.key_event) };
                inner.key_event = std::ptr::null_mut();
            }
            if !inner.key_encoder.is_null() {
                unsafe { ghostty_key_encoder_free(inner.key_encoder) };
                inner.key_encoder = std::ptr::null_mut();
            }
            if !inner.kitty_placement_iter.is_null() {
                unsafe {
                    ghostty_kitty_graphics_placement_iterator_free(inner.kitty_placement_iter)
                };
                inner.kitty_placement_iter = std::ptr::null_mut();
            }
            if !render.row_cells.is_null() {
                unsafe { ghostty_render_state_row_cells_free(render.row_cells) };
                render.row_cells = std::ptr::null_mut();
            }
            if !render.row_iter.is_null() {
                unsafe { ghostty_render_state_row_iterator_free(render.row_iter) };
                render.row_iter = std::ptr::null_mut();
            }
            if !render.render_state.is_null() {
                unsafe { ghostty_render_state_free(render.render_state) };
                render.render_state = std::ptr::null_mut();
            }
            if let Some(mut selection_gesture) = inner.selection_gesture.take() {
                unsafe { selection_gesture.free(inner.terminal) };
            }
            if !inner.terminal.is_null() {
                unsafe { ghostty_terminal_free(inner.terminal) };
                inner.terminal = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn libghostty_vt_manifest_matches_handwritten_ffi() {
        let manifest = unsafe {
            let ptr = ghostty_type_json();
            assert!(!ptr.is_null(), "ghostty_type_json returned null");
            CStr::from_ptr(ptr).to_str().expect("manifest is utf-8")
        };
        let manifest: serde_json::Value =
            serde_json::from_str(manifest).expect("manifest is valid json");
        let types = &manifest["types"];

        assert_eq!(manifest["schema"].as_u64(), Some(1));
        assert_eq!(
            types["GhosttyCell"]["size"].as_u64(),
            Some(std::mem::size_of::<GhosttyCell>() as u64)
        );
        assert_eq!(
            types["GhosttyStyle"]["size"].as_u64(),
            Some(std::mem::size_of::<GhosttyStyle>() as u64)
        );
        assert_eq!(
            types["GhosttyTerminalModeConfig"]["size"].as_u64(),
            Some(std::mem::size_of::<GhosttyTerminalModeConfig>() as u64)
        );
        assert_eq!(
            types["GhosttyTerminalModeConfig"]["fields"]["mode"]["offset"].as_u64(),
            Some(0)
        );
        assert_eq!(
            types["GhosttyTerminalModeConfig"]["fields"]["value"]["offset"].as_u64(),
            Some(2)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["SCROLLBACK_MAX_LINES"].as_i64(),
            Some(GhosttyTerminalOption::ScrollbackMaxLines as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["KITTY_IMAGE_STORAGE_LIMIT"].as_i64(),
            Some(GhosttyTerminalOption::KittyImageStorageLimit as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["SELECTION"].as_i64(),
            Some(GhosttyTerminalOption::Selection as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["CLIPBOARD_READ"].as_i64(),
            Some(GhosttyTerminalOption::ClipboardRead as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["CLIPBOARD_WRITE"].as_i64(),
            Some(GhosttyTerminalOption::ClipboardWrite as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["CLIPBOARD_WRITE_MAX_BYTES"].as_i64(),
            Some(GhosttyTerminalOption::ClipboardWriteMaxBytes as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["BELL"].as_i64(),
            Some(GhosttyTerminalOption::Bell as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["TITLE_CHANGED"].as_i64(),
            Some(GhosttyTerminalOption::TitleChanged as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["PWD_CHANGED"].as_i64(),
            Some(GhosttyTerminalOption::PwdChanged as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["DESKTOP_NOTIFICATION"].as_i64(),
            Some(GhosttyTerminalOption::DesktopNotification as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["PROGRESS_REPORT"].as_i64(),
            Some(GhosttyTerminalOption::ProgressReport as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["UNKNOWN_SEQUENCE"].as_i64(),
            Some(GhosttyTerminalOption::UnknownSequence as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["UNKNOWN_MAX_BYTES"].as_i64(),
            Some(GhosttyTerminalOption::UnknownMaxBytes as i64)
        );
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["CLIPBOARD_WRITE_MAX_BYTES"].as_i64(),
            Some(GhosttyTerminalData::ClipboardWriteMaxBytes as i64)
        );
        for (name, size, align) in [
            (
                "GhosttyMousePosition",
                std::mem::size_of::<GhosttyMousePosition>(),
                std::mem::align_of::<GhosttyMousePosition>(),
            ),
            (
                "GhosttyMouseEncoderSize",
                std::mem::size_of::<GhosttyMouseEncoderSize>(),
                std::mem::align_of::<GhosttyMouseEncoderSize>(),
            ),
            (
                "GhosttyWriter",
                std::mem::size_of::<GhosttyWriter>(),
                std::mem::align_of::<GhosttyWriter>(),
            ),
            (
                "GhosttyMimeReader",
                std::mem::size_of::<GhosttyMimeReader>(),
                std::mem::align_of::<GhosttyMimeReader>(),
            ),
            (
                "GhosttyPaste",
                std::mem::size_of::<GhosttyPaste>(),
                std::mem::align_of::<GhosttyPaste>(),
            ),
            (
                "GhosttyClipboardContent",
                std::mem::size_of::<GhosttyClipboardContent>(),
                std::mem::align_of::<GhosttyClipboardContent>(),
            ),
            (
                "GhosttyClipboardWriteReply",
                std::mem::size_of::<GhosttyClipboardWriteReply>(),
                std::mem::align_of::<GhosttyClipboardWriteReply>(),
            ),
            (
                "GhosttyClipboardWrite",
                std::mem::size_of::<GhosttyClipboardWrite>(),
                std::mem::align_of::<GhosttyClipboardWrite>(),
            ),
            (
                "GhosttyClipboardReadReply",
                std::mem::size_of::<GhosttyClipboardReadReply>(),
                std::mem::align_of::<GhosttyClipboardReadReply>(),
            ),
            (
                "GhosttyClipboardRead",
                std::mem::size_of::<GhosttyClipboardRead>(),
                std::mem::align_of::<GhosttyClipboardRead>(),
            ),
            (
                "GhosttyTerminalDesktopNotification",
                std::mem::size_of::<GhosttyTerminalDesktopNotification>(),
                std::mem::align_of::<GhosttyTerminalDesktopNotification>(),
            ),
            (
                "GhosttyTerminalProgressReport",
                std::mem::size_of::<GhosttyTerminalProgressReport>(),
                std::mem::align_of::<GhosttyTerminalProgressReport>(),
            ),
            (
                "GhosttyTerminalUnknownStringSequence",
                std::mem::size_of::<GhosttyTerminalUnknownStringSequence>(),
                std::mem::align_of::<GhosttyTerminalUnknownStringSequence>(),
            ),
            (
                "GhosttyTerminalUnknownSequenceValue",
                std::mem::size_of::<GhosttyTerminalUnknownSequenceValue>(),
                std::mem::align_of::<GhosttyTerminalUnknownSequenceValue>(),
            ),
            (
                "GhosttyTerminalUnknownSequence",
                std::mem::size_of::<GhosttyTerminalUnknownSequence>(),
                std::mem::align_of::<GhosttyTerminalUnknownSequence>(),
            ),
            (
                "GhosttySysImage",
                std::mem::size_of::<GhosttySysImage>(),
                std::mem::align_of::<GhosttySysImage>(),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo",
                std::mem::size_of::<GhosttyKittyGraphicsPlacementRenderInfo>(),
                std::mem::align_of::<GhosttyKittyGraphicsPlacementRenderInfo>(),
            ),
            (
                "GhosttyRenderStateCursor",
                std::mem::size_of::<GhosttyRenderStateCursor>(),
                std::mem::align_of::<GhosttyRenderStateCursor>(),
            ),
            (
                "GhosttyRenderStateRowSelection",
                std::mem::size_of::<GhosttyRenderStateRowSelection>(),
                std::mem::align_of::<GhosttyRenderStateRowSelection>(),
            ),
            (
                "GhosttyRenderStateColors",
                std::mem::size_of::<GhosttyRenderStateColors>(),
                std::mem::align_of::<GhosttyRenderStateColors>(),
            ),
            (
                "GhosttyPointValue",
                std::mem::size_of::<GhosttyPointValue>(),
                std::mem::align_of::<GhosttyPointValue>(),
            ),
            (
                "GhosttyPoint",
                std::mem::size_of::<GhosttyPoint>(),
                std::mem::align_of::<GhosttyPoint>(),
            ),
            (
                "GhosttyPointCoordinate",
                std::mem::size_of::<GhosttyPointCoordinate>(),
                std::mem::align_of::<GhosttyPointCoordinate>(),
            ),
            (
                "GhosttyGridRef",
                std::mem::size_of::<GhosttyGridRef>(),
                std::mem::align_of::<GhosttyGridRef>(),
            ),
            (
                "GhosttySelection",
                std::mem::size_of::<GhosttySelection>(),
                std::mem::align_of::<GhosttySelection>(),
            ),
            (
                "GhosttyTerminalSelectionFormatOptions",
                std::mem::size_of::<GhosttyTerminalSelectionFormatOptions>(),
                std::mem::align_of::<GhosttyTerminalSelectionFormatOptions>(),
            ),
            (
                "GhosttySurfacePosition",
                std::mem::size_of::<GhosttySurfacePosition>(),
                std::mem::align_of::<GhosttySurfacePosition>(),
            ),
            (
                "GhosttySelectionGestureGeometry",
                std::mem::size_of::<GhosttySelectionGestureGeometry>(),
                std::mem::align_of::<GhosttySelectionGestureGeometry>(),
            ),
        ] {
            assert_eq!(
                types[name]["size"].as_u64(),
                Some(size as u64),
                "{name} size"
            );
            assert_eq!(
                types[name]["align"].as_u64(),
                Some(align as u64),
                "{name} alignment"
            );
        }
        let cursor_fields = &types["GhosttyRenderStateCursor"]["fields"];
        for (name, offset) in [
            ("size", std::mem::offset_of!(GhosttyRenderStateCursor, size)),
            (
                "viewport_has_value",
                std::mem::offset_of!(GhosttyRenderStateCursor, viewport_has_value),
            ),
            (
                "viewport_x",
                std::mem::offset_of!(GhosttyRenderStateCursor, viewport_x),
            ),
            (
                "viewport_y",
                std::mem::offset_of!(GhosttyRenderStateCursor, viewport_y),
            ),
            (
                "wide_tail",
                std::mem::offset_of!(GhosttyRenderStateCursor, wide_tail),
            ),
            (
                "visible",
                std::mem::offset_of!(GhosttyRenderStateCursor, visible),
            ),
            (
                "blinking",
                std::mem::offset_of!(GhosttyRenderStateCursor, blinking),
            ),
            (
                "password_input",
                std::mem::offset_of!(GhosttyRenderStateCursor, password_input),
            ),
            (
                "visual_style",
                std::mem::offset_of!(GhosttyRenderStateCursor, visual_style),
            ),
        ] {
            assert_eq!(
                cursor_fields[name]["offset"].as_u64(),
                Some(offset as u64),
                "GhosttyRenderStateCursor::{name} offset"
            );
        }
        let row_selection_fields = &types["GhosttyRenderStateRowSelection"]["fields"];
        for (name, offset) in [
            (
                "size",
                std::mem::offset_of!(GhosttyRenderStateRowSelection, size),
            ),
            (
                "start_x",
                std::mem::offset_of!(GhosttyRenderStateRowSelection, start_x),
            ),
            (
                "end_x",
                std::mem::offset_of!(GhosttyRenderStateRowSelection, end_x),
            ),
        ] {
            assert_eq!(
                row_selection_fields[name]["offset"].as_u64(),
                Some(offset as u64),
                "GhosttyRenderStateRowSelection::{name} offset"
            );
        }
        let colors_fields = &types["GhosttyRenderStateColors"]["fields"];
        for (name, offset) in [
            ("size", std::mem::offset_of!(GhosttyRenderStateColors, size)),
            (
                "background",
                std::mem::offset_of!(GhosttyRenderStateColors, background),
            ),
            (
                "foreground",
                std::mem::offset_of!(GhosttyRenderStateColors, foreground),
            ),
            (
                "cursor",
                std::mem::offset_of!(GhosttyRenderStateColors, cursor),
            ),
            (
                "cursor_has_value",
                std::mem::offset_of!(GhosttyRenderStateColors, cursor_has_value),
            ),
            (
                "palette",
                std::mem::offset_of!(GhosttyRenderStateColors, palette),
            ),
        ] {
            assert_eq!(
                colors_fields[name]["offset"].as_u64(),
                Some(offset as u64),
                "GhosttyRenderStateColors::{name} offset"
            );
        }
        for (type_name, fields) in [
            (
                "GhosttyPoint",
                &[
                    ("tag", std::mem::offset_of!(GhosttyPoint, tag)),
                    ("value", std::mem::offset_of!(GhosttyPoint, value)),
                ][..],
            ),
            (
                "GhosttyPointCoordinate",
                &[
                    ("x", std::mem::offset_of!(GhosttyPointCoordinate, x)),
                    ("y", std::mem::offset_of!(GhosttyPointCoordinate, y)),
                ],
            ),
            (
                "GhosttyGridRef",
                &[
                    ("size", std::mem::offset_of!(GhosttyGridRef, size)),
                    ("node", std::mem::offset_of!(GhosttyGridRef, node)),
                    ("x", std::mem::offset_of!(GhosttyGridRef, x)),
                    ("y", std::mem::offset_of!(GhosttyGridRef, y)),
                ],
            ),
            (
                "GhosttySelection",
                &[
                    ("size", std::mem::offset_of!(GhosttySelection, size)),
                    ("start", std::mem::offset_of!(GhosttySelection, start)),
                    ("end", std::mem::offset_of!(GhosttySelection, end)),
                    (
                        "rectangle",
                        std::mem::offset_of!(GhosttySelection, rectangle),
                    ),
                ],
            ),
            (
                "GhosttyTerminalSelectionFormatOptions",
                &[
                    (
                        "size",
                        std::mem::offset_of!(GhosttyTerminalSelectionFormatOptions, size),
                    ),
                    (
                        "emit",
                        std::mem::offset_of!(GhosttyTerminalSelectionFormatOptions, emit),
                    ),
                    (
                        "unwrap",
                        std::mem::offset_of!(GhosttyTerminalSelectionFormatOptions, unwrap),
                    ),
                    (
                        "trim",
                        std::mem::offset_of!(GhosttyTerminalSelectionFormatOptions, trim),
                    ),
                    (
                        "selection",
                        std::mem::offset_of!(GhosttyTerminalSelectionFormatOptions, selection),
                    ),
                ],
            ),
            (
                "GhosttySurfacePosition",
                &[
                    ("x", std::mem::offset_of!(GhosttySurfacePosition, x)),
                    ("y", std::mem::offset_of!(GhosttySurfacePosition, y)),
                ],
            ),
            (
                "GhosttySelectionGestureGeometry",
                &[
                    (
                        "columns",
                        std::mem::offset_of!(GhosttySelectionGestureGeometry, columns),
                    ),
                    (
                        "cell_width",
                        std::mem::offset_of!(GhosttySelectionGestureGeometry, cell_width),
                    ),
                    (
                        "padding_left",
                        std::mem::offset_of!(GhosttySelectionGestureGeometry, padding_left),
                    ),
                    (
                        "screen_height",
                        std::mem::offset_of!(GhosttySelectionGestureGeometry, screen_height),
                    ),
                ],
            ),
        ] {
            for (field_name, offset) in fields {
                assert_eq!(
                    types[type_name]["fields"][field_name]["offset"].as_u64(),
                    Some(*offset as u64),
                    "{type_name}::{field_name} offset"
                );
            }
        }
        for (name, value) in [
            ("NONE", GhosttySelectionGestureAutoscroll::None),
            ("UP", GhosttySelectionGestureAutoscroll::Up),
            ("DOWN", GhosttySelectionGestureAutoscroll::Down),
        ] {
            assert_eq!(
                types["GhosttySelectionGestureAutoscroll"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttySelectionGestureAutoscroll::{name}"
            );
        }
        for (name, value) in [
            ("CLICK_COUNT", GhosttySelectionGestureData::ClickCount),
            ("AUTOSCROLL", GhosttySelectionGestureData::Autoscroll),
        ] {
            assert_eq!(
                types["GhosttySelectionGestureData"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttySelectionGestureData::{name}"
            );
        }
        for (name, value) in [
            ("PRESS", GhosttySelectionGestureEventType::Press),
            ("RELEASE", GhosttySelectionGestureEventType::Release),
            ("DRAG", GhosttySelectionGestureEventType::Drag),
            (
                "AUTOSCROLL_TICK",
                GhosttySelectionGestureEventType::AutoscrollTick,
            ),
        ] {
            assert_eq!(
                types["GhosttySelectionGestureEventType"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttySelectionGestureEventType::{name}"
            );
        }
        for (name, value) in [
            ("REF", GhosttySelectionGestureEventOption::Ref),
            ("POSITION", GhosttySelectionGestureEventOption::Position),
            (
                "REPEAT_DISTANCE",
                GhosttySelectionGestureEventOption::RepeatDistance,
            ),
            ("TIME_NS", GhosttySelectionGestureEventOption::TimeNs),
            (
                "REPEAT_INTERVAL_NS",
                GhosttySelectionGestureEventOption::RepeatIntervalNs,
            ),
            ("GEOMETRY", GhosttySelectionGestureEventOption::Geometry),
            ("VIEWPORT", GhosttySelectionGestureEventOption::Viewport),
        ] {
            assert_eq!(
                types["GhosttySelectionGestureEventOption"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttySelectionGestureEventOption::{name}"
            );
        }
        assert_eq!(
            types["GhosttyFormatterFormat"]["values"]["PLAIN"].as_i64(),
            Some(GhosttyFormatterFormat::Plain as i64)
        );
        assert_eq!(
            types["GhosttyPointTag"]["values"]["VIEWPORT"].as_i64(),
            Some(GhosttyPointTag::Viewport as i64)
        );
        for (name, value) in [
            ("STANDARD", GhosttyClipboardLocation::Standard),
            ("SELECTION", GhosttyClipboardLocation::Selection),
            ("PRIMARY", GhosttyClipboardLocation::Primary),
        ] {
            assert_eq!(
                types["GhosttyClipboardLocation"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyClipboardLocation::{name}"
            );
        }
        for (name, value) in [
            ("CLIPBOARD", GhosttyPasteSource::Clipboard),
            ("TEXT", GhosttyPasteSource::Text),
        ] {
            assert_eq!(
                types["GhosttyPasteSource"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyPasteSource::{name}"
            );
        }
        for (name, value) in [
            ("SUCCESS", GhosttyClipboardWriteResult::Success),
            ("DENIED", GhosttyClipboardWriteResult::Denied),
            ("UNSUPPORTED", GhosttyClipboardWriteResult::Unsupported),
            ("BUSY", GhosttyClipboardWriteResult::Busy),
            ("INVALID_DATA", GhosttyClipboardWriteResult::InvalidData),
            ("IO_ERROR", GhosttyClipboardWriteResult::IoError),
        ] {
            assert_eq!(
                types["GhosttyClipboardWriteResult"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyClipboardWriteResult::{name}"
            );
        }
        for (name, value) in [
            ("SUCCESS", GhosttyClipboardReadResult::Success),
            ("DENIED", GhosttyClipboardReadResult::Denied),
            ("UNSUPPORTED", GhosttyClipboardReadResult::Unsupported),
            ("BUSY", GhosttyClipboardReadResult::Busy),
            ("IO_ERROR", GhosttyClipboardReadResult::IoError),
        ] {
            assert_eq!(
                types["GhosttyClipboardReadResult"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyClipboardReadResult::{name}"
            );
        }
        assert_eq!(
            types["GhosttyResult"]["values"]["IO_ERROR"].as_i64(),
            Some(GHOSTTY_IO_ERROR as i64)
        );
        assert_eq!(
            types["GhosttyResult"]["values"]["INVALID_VALUE"].as_i64(),
            Some(GHOSTTY_INVALID_VALUE as i64)
        );
        assert_eq!(
            types["GhosttyResult"]["values"]["REJECTED"].as_i64(),
            Some(GHOSTTY_REJECTED as i64)
        );
        assert_eq!(
            types["GhosttyResult"]["values"]["NO_VALUE"].as_i64(),
            Some(GHOSTTY_NO_VALUE as i64)
        );
        for (name, value) in [
            ("CURSOR", GhosttyRenderStateData::Cursor),
            ("COLORS", GhosttyRenderStateData::Colors),
        ] {
            assert_eq!(
                types["GhosttyRenderStateData"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyRenderStateData::{name}"
            );
        }
        for (name, value) in [
            ("SELECTION", GhosttyRenderStateRowData::Selection),
            ("CELLS_RAW", GhosttyRenderStateRowData::CellsRaw),
        ] {
            assert_eq!(
                types["GhosttyRenderStateRowData"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyRenderStateRowData::{name}"
            );
        }
        for (name, value) in [
            ("SELECTED", GhosttyRenderStateRowCellsData::Selected),
            ("HAS_STYLING", GhosttyRenderStateRowCellsData::HasStyling),
            (
                "GRAPHEMES_UTF8",
                GhosttyRenderStateRowCellsData::GraphemesUtf8,
            ),
        ] {
            assert_eq!(
                types["GhosttyRenderStateRowCellsData"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyRenderStateRowCellsData::{name}"
            );
        }
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["MODE"].as_i64(),
            Some(GhosttyTerminalData::Mode as i64)
        );
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["CURSOR_AT_PROMPT"].as_i64(),
            Some(GhosttyTerminalData::CursorAtPrompt as i64)
        );
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["KITTY_KEYBOARD_FLAGS"].as_i64(),
            Some(GhosttyTerminalData::KittyKeyboardFlags as i64)
        );
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["KITTY_IMAGE_STORAGE_LIMIT"].as_i64(),
            Some(GhosttyTerminalData::KittyImageStorageLimit as i64)
        );
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["KITTY_GRAPHICS"].as_i64(),
            Some(GhosttyTerminalData::KittyGraphics as i64)
        );
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["SELECTION"].as_i64(),
            Some(GhosttyTerminalData::Selection as i64)
        );
        assert_eq!(
            types["GhosttySysOption"]["values"]["DECODE_PNG"].as_i64(),
            Some(GhosttySysOption::DecodePng as i64)
        );
        assert_eq!(
            types["GhosttyKittyGraphicsData"]["values"]["PLACEMENT_ITERATOR"].as_i64(),
            Some(GhosttyKittyGraphicsData::PlacementIterator as i64)
        );
        for (name, value) in [
            ("IMAGE_ID", GhosttyKittyGraphicsPlacementData::ImageId),
            (
                "PLACEMENT_ID",
                GhosttyKittyGraphicsPlacementData::PlacementId,
            ),
            ("IS_VIRTUAL", GhosttyKittyGraphicsPlacementData::IsVirtual),
            ("X_OFFSET", GhosttyKittyGraphicsPlacementData::XOffset),
            ("Y_OFFSET", GhosttyKittyGraphicsPlacementData::YOffset),
            ("Z", GhosttyKittyGraphicsPlacementData::Z),
        ] {
            assert_eq!(
                types["GhosttyKittyGraphicsPlacementData"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyKittyGraphicsPlacementData::{name}"
            );
        }
        for (name, value) in [
            ("WIDTH", GhosttyKittyGraphicsImageData::Width),
            ("HEIGHT", GhosttyKittyGraphicsImageData::Height),
            ("FORMAT", GhosttyKittyGraphicsImageData::Format),
            ("COMPRESSION", GhosttyKittyGraphicsImageData::Compression),
            ("DATA_PTR", GhosttyKittyGraphicsImageData::DataPtr),
            ("DATA_LEN", GhosttyKittyGraphicsImageData::DataLen),
            ("GENERATION", GhosttyKittyGraphicsImageData::Generation),
        ] {
            assert_eq!(
                types["GhosttyKittyGraphicsImageData"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyKittyGraphicsImageData::{name}"
            );
        }
        for (name, value) in [
            ("RGB", GhosttyKittyImageFormat::Rgb),
            ("RGBA", GhosttyKittyImageFormat::Rgba),
            ("PNG", GhosttyKittyImageFormat::Png),
            ("GRAY_ALPHA", GhosttyKittyImageFormat::GrayAlpha),
            ("GRAY", GhosttyKittyImageFormat::Gray),
        ] {
            assert_eq!(
                types["GhosttyKittyImageFormat"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyKittyImageFormat::{name}"
            );
        }
        for (name, value) in [
            ("NONE", GhosttyKittyImageCompression::None),
            ("ZLIB_DEFLATE", GhosttyKittyImageCompression::ZlibDeflate),
        ] {
            assert_eq!(
                types["GhosttyKittyImageCompression"]["values"][name].as_i64(),
                Some(value as i64),
                "GhosttyKittyImageCompression::{name}"
            );
        }
        let render_info = &types["GhosttyKittyGraphicsPlacementRenderInfo"]["fields"];
        for (name, offset) in [
            ("size", 0),
            ("pixel_width", 8),
            ("viewport_visible", 32),
            ("source_x", 36),
            ("source_height", 48),
        ] {
            assert_eq!(
                render_info[name]["offset"].as_u64(),
                Some(offset),
                "GhosttyKittyGraphicsPlacementRenderInfo::{name} offset"
            );
        }
        assert_eq!(
            types["GhosttyKittyKeyFlags"]["size"].as_u64(),
            Some(std::mem::size_of::<u8>() as u64)
        );
        assert_eq!(
            types["GhosttyKeyAction"]["values"]["RELEASE"].as_i64(),
            Some(ghostty_key_action(VtKeyAction::Release) as i64)
        );
        assert_eq!(
            types["GhosttyKeyAction"]["values"]["PRESS"].as_i64(),
            Some(ghostty_key_action(VtKeyAction::Press) as i64)
        );
        assert_eq!(
            types["GhosttyKeyAction"]["values"]["REPEAT"].as_i64(),
            Some(ghostty_key_action(VtKeyAction::Repeat) as i64)
        );

        for (name, action) in [
            ("PRESS", GhosttyMouseAction::Press),
            ("RELEASE", GhosttyMouseAction::Release),
            ("MOTION", GhosttyMouseAction::Motion),
        ] {
            assert_eq!(
                types["GhosttyMouseAction"]["values"][name].as_i64(),
                Some(action as i64),
                "GhosttyMouseAction::{name}"
            );
        }
        for (name, button) in [
            ("LEFT", GhosttyMouseButton::Left),
            ("RIGHT", GhosttyMouseButton::Right),
            ("MIDDLE", GhosttyMouseButton::Middle),
            ("FOUR", GhosttyMouseButton::Four),
            ("FIVE", GhosttyMouseButton::Five),
            ("SIX", GhosttyMouseButton::Six),
            ("SEVEN", GhosttyMouseButton::Seven),
        ] {
            assert_eq!(
                types["GhosttyMouseButton"]["values"][name].as_i64(),
                Some(button as i64),
                "GhosttyMouseButton::{name}"
            );
        }
        for (name, option) in [
            ("SIZE", GhosttyMouseEncoderOption::Size),
            (
                "ANY_BUTTON_PRESSED",
                GhosttyMouseEncoderOption::AnyButtonPressed,
            ),
            ("TRACK_LAST_CELL", GhosttyMouseEncoderOption::TrackLastCell),
        ] {
            assert_eq!(
                types["GhosttyMouseEncoderOption"]["values"][name].as_i64(),
                Some(option as i64),
                "GhosttyMouseEncoderOption::{name}"
            );
        }
        for (name, offset) in [
            ("x", std::mem::offset_of!(GhosttyMousePosition, x)),
            ("y", std::mem::offset_of!(GhosttyMousePosition, y)),
        ] {
            assert_eq!(
                types["GhosttyMousePosition"]["fields"][name]["offset"].as_u64(),
                Some(offset as u64),
                "GhosttyMousePosition::{name} offset"
            );
        }
        for (name, offset) in [
            ("size", std::mem::offset_of!(GhosttyMouseEncoderSize, size)),
            (
                "screen_width",
                std::mem::offset_of!(GhosttyMouseEncoderSize, screen_width),
            ),
            (
                "screen_height",
                std::mem::offset_of!(GhosttyMouseEncoderSize, screen_height),
            ),
            (
                "cell_width",
                std::mem::offset_of!(GhosttyMouseEncoderSize, cell_width),
            ),
            (
                "cell_height",
                std::mem::offset_of!(GhosttyMouseEncoderSize, cell_height),
            ),
            (
                "padding_top",
                std::mem::offset_of!(GhosttyMouseEncoderSize, padding_top),
            ),
            (
                "padding_bottom",
                std::mem::offset_of!(GhosttyMouseEncoderSize, padding_bottom),
            ),
            (
                "padding_right",
                std::mem::offset_of!(GhosttyMouseEncoderSize, padding_right),
            ),
            (
                "padding_left",
                std::mem::offset_of!(GhosttyMouseEncoderSize, padding_left),
            ),
        ] {
            assert_eq!(
                types["GhosttyMouseEncoderSize"]["fields"][name]["offset"].as_u64(),
                Some(offset as u64),
                "GhosttyMouseEncoderSize::{name} offset"
            );
        }

        let keys = &types["GhosttyKey"]["values"];
        for (name, key) in [
            ("UNIDENTIFIED", GhosttyKey::Unidentified),
            ("ALT_LEFT", GhosttyKey::AltLeft),
            ("ALT_RIGHT", GhosttyKey::AltRight),
            ("BACKSPACE", GhosttyKey::Backspace),
            ("CAPS_LOCK", GhosttyKey::CapsLock),
            ("CONTEXT_MENU", GhosttyKey::ContextMenu),
            ("CONTROL_LEFT", GhosttyKey::ControlLeft),
            ("CONTROL_RIGHT", GhosttyKey::ControlRight),
            ("ENTER", GhosttyKey::Enter),
            ("META_LEFT", GhosttyKey::MetaLeft),
            ("META_RIGHT", GhosttyKey::MetaRight),
            ("SHIFT_LEFT", GhosttyKey::ShiftLeft),
            ("SHIFT_RIGHT", GhosttyKey::ShiftRight),
            ("SPACE", GhosttyKey::Space),
            ("TAB", GhosttyKey::Tab),
            ("DELETE", GhosttyKey::Delete),
            ("END", GhosttyKey::End),
            ("HELP", GhosttyKey::Help),
            ("HOME", GhosttyKey::Home),
            ("INSERT", GhosttyKey::Insert),
            ("PAGE_DOWN", GhosttyKey::PageDown),
            ("PAGE_UP", GhosttyKey::PageUp),
            ("ARROW_DOWN", GhosttyKey::ArrowDown),
            ("ARROW_LEFT", GhosttyKey::ArrowLeft),
            ("ARROW_RIGHT", GhosttyKey::ArrowRight),
            ("ARROW_UP", GhosttyKey::ArrowUp),
            ("ESCAPE", GhosttyKey::Escape),
            ("F1", GhosttyKey::F1),
            ("F2", GhosttyKey::F2),
            ("F3", GhosttyKey::F3),
            ("F4", GhosttyKey::F4),
            ("F5", GhosttyKey::F5),
            ("F6", GhosttyKey::F6),
            ("F7", GhosttyKey::F7),
            ("F8", GhosttyKey::F8),
            ("F9", GhosttyKey::F9),
            ("F10", GhosttyKey::F10),
            ("F11", GhosttyKey::F11),
            ("F12", GhosttyKey::F12),
            ("F13", GhosttyKey::F13),
            ("F14", GhosttyKey::F14),
            ("F15", GhosttyKey::F15),
            ("F16", GhosttyKey::F16),
            ("F17", GhosttyKey::F17),
            ("F18", GhosttyKey::F18),
            ("F19", GhosttyKey::F19),
            ("F20", GhosttyKey::F20),
            ("F21", GhosttyKey::F21),
            ("F22", GhosttyKey::F22),
            ("F23", GhosttyKey::F23),
            ("F24", GhosttyKey::F24),
            ("F25", GhosttyKey::F25),
            ("PRINT_SCREEN", GhosttyKey::PrintScreen),
            ("SCROLL_LOCK", GhosttyKey::ScrollLock),
            ("PAUSE", GhosttyKey::Pause),
        ] {
            assert_eq!(keys[name].as_i64(), Some(key as i64), "GhosttyKey::{key:?}");
        }
    }

    fn test_selection_geometry(columns: u32, rows: u32) -> SelectionGeometry {
        SelectionGeometry {
            columns,
            cell_width_px: 10,
            padding_left_px: 0,
            screen_height_px: rows * 20,
        }
    }

    fn test_selection_point(col: u16, row: u16, x_offset_px: f64) -> SelectionPoint {
        SelectionPoint {
            col,
            row,
            surface_x_px: f64::from(col) * 10.0 + x_offset_px,
            surface_y_px: f64::from(row) * 20.0 + 10.0,
        }
    }

    #[test]
    fn terminal_selection_formats_text_and_advances_generation() {
        let screen = VtScreen::new(20, 3, None).expect("create vt screen");
        screen.feed(b"alpha beta\r\nsecond line");
        let geometry = test_selection_geometry(20, 3);
        let baseline_generation = screen.generation();

        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("press selection anchor");
        assert!(!screen.has_selection());
        assert_eq!(screen.generation(), baseline_generation);

        screen
            .selection_drag(test_selection_point(4, 0, 9.0), geometry)
            .expect("drag selection");
        assert!(screen.has_selection());
        assert_eq!(screen.selection_text().as_deref(), Some("alpha"));
        assert_eq!(screen.generation(), baseline_generation.wrapping_add(1));

        let selected_generation = screen.generation();
        screen
            .selection_drag(test_selection_point(4, 0, 9.0), geometry)
            .expect("repeat drag in selected cell");
        assert_eq!(screen.generation(), selected_generation);

        screen
            .selection_release(Some((4, 0)))
            .expect("release selection");
        assert!(screen.clear_selection());
        assert!(!screen.has_selection());
        assert_eq!(screen.selection_text(), None);
        assert_eq!(screen.generation(), baseline_generation.wrapping_add(2));

        let cleared_generation = screen.generation();
        assert!(!screen.clear_selection());
        assert_eq!(screen.generation(), cleared_generation);
    }

    #[test]
    fn snapshots_cache_terminal_selection_ranges_and_clear_stale_rows() {
        let screen = VtScreen::new(6, 3, None).expect("create vt screen");
        screen.feed(b"abcdef\r\nghijkl\x1b[?25l");
        let geometry = test_selection_geometry(6, 3);
        let baseline = screen.snapshot();
        assert_eq!(baseline.selection_ranges, vec![None; 3]);
        screen.acknowledge_snapshot(baseline.generation);

        screen
            .selection_press(test_selection_point(2, 0, 1.0), geometry, 1, false)
            .expect("press selection anchor");
        screen
            .selection_drag(test_selection_point(3, 1, 9.0), geometry)
            .expect("drag selection across rows");

        let selected = screen.snapshot();
        assert_eq!(
            selected.selection_ranges,
            vec![
                Some(SelectionRange { start: 2, end: 5 }),
                Some(SelectionRange { start: 0, end: 3 }),
                None,
            ]
        );
        assert!(selected.generation > baseline.generation);
        assert_eq!(selected.dirty_rows, vec![0, 1, 2]);
        assert!(selected.selection_ranges[0].is_some_and(|range| range.contains(2)));
        assert!(!selected.selection_ranges[0].is_some_and(|range| range.contains(1)));

        let cached = screen.snapshot();
        assert_eq!(cached.generation, selected.generation);
        assert_eq!(cached.selection_ranges, selected.selection_ranges);
        screen.acknowledge_snapshot(selected.generation);

        screen.feed(b"\x1b[3;1HX");
        let partial = screen.snapshot();
        assert_eq!(partial.selection_ranges, selected.selection_ranges);
        assert_eq!(partial.dirty_rows, vec![1, 2]);
        screen.acknowledge_snapshot(partial.generation);

        assert!(screen.clear_selection());
        let cleared = screen.snapshot();
        assert!(cleared.generation > partial.generation);
        assert_eq!(cleared.selection_ranges, vec![None; 3]);
        assert_eq!(cleared.dirty_rows, vec![0, 1, 2]);
    }

    #[test]
    fn repeat_clicks_select_word_then_line() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        screen.feed(b"alpha beta");
        let geometry = test_selection_geometry(20, 2);
        let point = test_selection_point(1, 0, 5.0);

        screen
            .selection_press(point, geometry, 1, false)
            .expect("first click");
        screen
            .selection_release(Some((point.col, point.row)))
            .expect("first release");
        assert_eq!(screen.selection_text(), None);

        screen
            .selection_press(point, geometry, 2, false)
            .expect("second click");
        assert_eq!(screen.selection_text().as_deref(), Some("alpha"));
        screen
            .selection_release(Some((point.col, point.row)))
            .expect("second release");

        screen
            .selection_press(point, geometry, 3, false)
            .expect("third click");
        assert_eq!(screen.selection_text().as_deref(), Some("alpha beta"));
    }

    #[test]
    fn shifted_single_click_falls_back_to_a_fresh_press_after_gesture_reset() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        screen.feed(b"alpha beta");
        let geometry = test_selection_geometry(20, 2);

        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("press initial anchor");
        screen
            .selection_drag(test_selection_point(4, 0, 9.0), geometry)
            .expect("select alpha");
        screen
            .selection_release(Some((4, 0)))
            .expect("release initial selection");
        screen.selection_cancel_gesture();

        screen
            .selection_press(test_selection_point(6, 0, 1.0), geometry, 1, true)
            .expect("shifted press after reset");
        screen
            .selection_drag(test_selection_point(9, 0, 9.0), geometry)
            .expect("drag from fallback anchor");
        assert_eq!(screen.selection_text().as_deref(), Some("beta"));
    }

    #[test]
    fn shifted_repeat_click_keeps_platform_word_behavior() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        screen.feed(b"alpha beta");
        let geometry = test_selection_geometry(20, 2);

        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("first click");
        screen
            .selection_release(Some((0, 0)))
            .expect("first release");
        screen
            .selection_press(test_selection_point(7, 0, 5.0), geometry, 2, true)
            .expect("shifted double click");

        assert_eq!(screen.selection_text().as_deref(), Some("beta"));
    }

    #[test]
    fn clearing_selection_resets_repeat_click_state() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        screen.feed(b"alpha beta");
        let geometry = test_selection_geometry(20, 2);
        let point = test_selection_point(1, 0, 5.0);

        screen
            .selection_press(point, geometry, 1, false)
            .expect("first click");
        screen
            .selection_release(Some((point.col, point.row)))
            .expect("first release");
        assert!(!screen.clear_selection());

        screen
            .selection_press(point, geometry, 1, false)
            .expect("click after clear");
        assert_eq!(screen.selection_text(), None);
    }

    #[test]
    fn drag_after_release_extends_existing_selection() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        screen.feed(b"alpha beta");
        let geometry = test_selection_geometry(20, 2);

        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("press selection anchor");
        screen
            .selection_drag(test_selection_point(4, 0, 9.0), geometry)
            .expect("drag initial selection");
        screen
            .selection_release(Some((4, 0)))
            .expect("release initial selection");

        screen
            .selection_drag(test_selection_point(9, 0, 9.0), geometry)
            .expect("extend released selection");
        assert_eq!(screen.selection_text().as_deref(), Some("alpha beta"));
    }

    #[test]
    fn mouse_motion_tracking_excludes_press_only_modes() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        assert!(!screen.mouse_motion_tracking_active());

        screen.feed(b"\x1b[?1000h");
        assert!(screen.mouse_tracking_active());
        assert!(!screen.mouse_motion_tracking_active());

        screen.feed(b"\x1b[?1000l\x1b[?1002h");
        assert!(screen.mouse_motion_tracking_active());

        screen.feed(b"\x1b[?1002l\x1b[?1003h");
        assert!(screen.mouse_motion_tracking_active());

        screen.feed(b"\x1b[?1003l\x1b[?9h");
        assert!(screen.mouse_tracking_active());
        assert!(!screen.mouse_motion_tracking_active());
    }

    #[test]
    fn terminal_selection_survives_reflow_and_clears_across_screen_switches() {
        let screen = VtScreen::new(20, 3, None).expect("create vt screen");
        screen.feed(b"alpha beta");
        let geometry = test_selection_geometry(20, 3);
        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("press selection anchor");
        screen
            .selection_drag(test_selection_point(4, 0, 9.0), geometry)
            .expect("drag selection");
        screen
            .selection_release(Some((4, 0)))
            .expect("release selection");

        screen.resize(8, 3, 10, 20).expect("reflow terminal");
        assert_eq!(screen.selection_text().as_deref(), Some("alpha"));

        screen.feed(b"\x1b[?1049h");
        assert!(!screen.has_selection());
        assert_eq!(screen.selection_text(), None);

        screen.feed(b"\x1b[?1049l");
        assert!(!screen.has_selection());
        assert_eq!(screen.selection_text(), None);
    }

    #[test]
    fn terminal_selection_preserves_graphemes_across_soft_wraps() {
        let screen = VtScreen::new(5, 3, None).expect("create vt screen");
        screen.feed("cafe\u{301}xy".as_bytes());
        let geometry = test_selection_geometry(5, 3);

        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("press selection anchor");
        screen
            .selection_drag(test_selection_point(0, 1, 9.0), geometry)
            .expect("drag selection across soft wrap");

        assert_eq!(screen.selection_text().as_deref(), Some("cafe\u{301}xy"));
    }

    #[test]
    fn terminal_selection_tracks_text_into_scrollback() {
        let screen = VtScreen::new(10, 2, None).expect("create vt screen");
        screen.feed(b"alpha\r\nbeta");
        let geometry = test_selection_geometry(10, 2);

        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("press selection anchor");
        screen
            .selection_drag(test_selection_point(4, 0, 9.0), geometry)
            .expect("drag selection");
        screen
            .selection_release(Some((4, 0)))
            .expect("release selection");

        screen.feed(b"\r\ngamma");

        assert_eq!(screen.selection_text().as_deref(), Some("alpha"));
    }

    #[test]
    fn terminal_selection_autoscroll_tick_extends_into_scrollback() {
        let screen = VtScreen::new(10, 2, None).expect("create vt screen");
        screen.feed(b"one\r\ntwo\r\nthree");
        let geometry = test_selection_geometry(10, 2);
        let mut point_above_viewport = test_selection_point(0, 0, 1.0);
        point_above_viewport.surface_y_px = -1.0;
        assert_eq!(screen.scrollbar().map(|state| state.offset), Some(1));

        screen
            .selection_press(test_selection_point(4, 1, 9.0), geometry, 1, false)
            .expect("press selection anchor");
        assert_eq!(
            screen
                .selection_drag(point_above_viewport, geometry)
                .expect("drag above viewport"),
            SelectionAutoscroll::Up
        );
        let generation_before_tick = screen.generation();

        let update = screen
            .selection_autoscroll_tick(point_above_viewport, geometry)
            .expect("autoscroll selection");
        assert_eq!(update.direction, SelectionAutoscroll::Up);
        assert!(update.changed);

        assert!(screen.generation() > generation_before_tick);
        assert_eq!(screen.scrollbar().map(|state| state.offset), Some(0));
        assert_eq!(screen.selection_text().as_deref(), Some("one\ntwo\nthree"));

        let generation_at_boundary = screen.generation();
        let update = screen
            .selection_autoscroll_tick(point_above_viewport, geometry)
            .expect("clamped autoscroll selection");
        assert_eq!(update.direction, SelectionAutoscroll::Up);
        assert!(!update.changed);
        assert_eq!(screen.generation(), generation_at_boundary);
    }

    #[test]
    fn taking_selection_formats_and_clears_in_one_generation() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        screen.feed(b"alpha beta");
        let geometry = test_selection_geometry(20, 2);
        screen
            .selection_press(test_selection_point(0, 0, 1.0), geometry, 1, false)
            .expect("press selection anchor");
        screen
            .selection_drag(test_selection_point(4, 0, 9.0), geometry)
            .expect("select alpha");
        let generation = screen.generation();

        assert_eq!(screen.take_selection_text().as_deref(), Some("alpha"));
        assert!(!screen.has_selection());
        assert_eq!(screen.generation(), generation.wrapping_add(1));
        assert_eq!(screen.take_selection_text(), None);
    }

    #[test]
    fn selection_rejects_invalid_view_geometry_without_mutation() {
        let screen = VtScreen::new(20, 2, None).expect("create vt screen");
        let generation = screen.generation();
        let mut point = test_selection_point(0, 0, 1.0);
        point.surface_x_px = f64::NAN;

        assert!(
            screen
                .selection_press(point, test_selection_geometry(20, 2), 1, false)
                .is_err()
        );
        assert!(
            screen
                .selection_press(
                    test_selection_point(0, 0, 1.0),
                    SelectionGeometry {
                        columns: 20,
                        cell_width_px: 0,
                        padding_left_px: 0,
                        screen_height_px: 40,
                    },
                    1,
                    false,
                )
                .is_err()
        );
        assert!(!screen.has_selection());
        assert_eq!(screen.generation(), generation);
    }

    #[test]
    fn snapshot_damage_tracks_render_acknowledgment() {
        let screen = VtScreen::new(4, 3, None).expect("create vt screen");

        screen.feed(b"\x1b[?25l");
        let baseline = screen.snapshot();
        screen.acknowledge_snapshot(baseline.generation);

        screen.feed(b"A\x1b[3;1H");
        let first = screen.snapshot();
        assert!(first.dirty_rows.contains(&0));
        assert_eq!(screen.snapshot().dirty_rows, first.dirty_rows);

        screen.feed(b"B");
        screen.acknowledge_snapshot(first.generation);
        let second = screen.snapshot();
        assert_eq!(second.dirty_rows, vec![2]);

        screen.feed(b"\x1b[2;1HC");
        let combined = screen.snapshot();
        assert_eq!(combined.dirty_rows, vec![1, 2]);
        screen.acknowledge_snapshot(second.generation);
        assert_eq!(screen.snapshot().dirty_rows, combined.dirty_rows);

        screen.acknowledge_snapshot(combined.generation);
        assert!(screen.snapshot().dirty_rows.is_empty());
    }

    #[test]
    fn discarded_render_state_rebuilds_from_clean_terminal() {
        let screen = VtScreen::new(4, 3, None).expect("create vt screen");

        screen.feed(b"A");
        let first = screen.try_snapshot().expect("extract initial snapshot");
        assert_eq!(first.cells[0].codepoint, u32::from('A'));

        screen.render.lock().invalidate_render_state();

        let recovered = screen.try_snapshot().expect("rebuild render state");
        assert_eq!(recovered.generation, first.generation);
        assert_eq!(recovered.cells[0].codepoint, u32::from('A'));
        assert_eq!(recovered.cells.len(), 12);
    }

    #[test]
    fn kitty_graphics_snapshot_decodes_png_and_reuses_unchanged_pixels() {
        let screen = VtScreen::new_with_write_pty(80, 24, None, Some(Arc::new(|_, _| Ok(()))))
            .expect("create vt screen");
        screen.resize(80, 24, 8, 16).expect("set cell size");
        screen.feed(
            b"\x1b_Ga=T,f=100,q=2;\
              iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAA\
              DUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErkJggg==\
              \x1b\\",
        );

        let first = screen.snapshot();
        assert_eq!(first.kitty_placements.len(), 1);
        let placement = &first.kitty_placements[0];
        assert_eq!((placement.image.width, placement.image.height), (1, 1));
        assert_eq!(placement.image.rgba.as_ref(), &[0xFF, 0, 0, 0xFF]);
        assert_eq!(
            (
                placement.source_x,
                placement.source_y,
                placement.source_width,
                placement.source_height,
            ),
            (0, 0, 1, 1)
        );

        let pixels = placement.image.clone();
        screen.bump_generation();
        let second = screen.snapshot();
        assert_eq!(second.kitty_placements.len(), 1);
        assert!(Arc::ptr_eq(&pixels, &second.kitty_placements[0].image));
    }

    type MouseWrites = Arc<Mutex<Vec<(Vec<u8>, PtyWriteClass)>>>;

    fn mouse_test_screen(cols: u16, rows: u16) -> (VtScreen, MouseWrites) {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let callback_writes = writes.clone();
        let screen = VtScreen::new_with_write_pty(
            cols,
            rows,
            None,
            Some(Arc::new(move |bytes, class| {
                callback_writes.lock().push((bytes.to_vec(), class));
                Ok(())
            })),
        )
        .expect("create mouse test screen");
        screen.set_mouse_geometry(u32::from(cols) * 10, u32::from(rows) * 20, 10, 20);
        (screen, writes)
    }

    fn test_mouse_event(
        action: VtMouseAction,
        button: Option<VtMouseButton>,
        x: f32,
        y: f32,
    ) -> VtMouseEvent {
        VtMouseEvent {
            action,
            button,
            modifiers: VtMouseModifiers::default(),
            surface_x_px: x,
            surface_y_px: y,
        }
    }

    #[test]
    fn mouse_encoder_emits_legacy_utf8_sgr_urxvt_and_pixel_formats() {
        let cases = [
            (
                b"\x1b[?1000h".as_slice(),
                test_mouse_event(VtMouseAction::Press, Some(VtMouseButton::Left), 15.0, 10.0),
                b"\x1b[M \"!".as_slice(),
                80,
            ),
            (
                b"\x1b[?1000h\x1b[?1005h".as_slice(),
                test_mouse_event(
                    VtMouseAction::Press,
                    Some(VtMouseButton::Left),
                    2235.0,
                    10.0,
                ),
                b"\x1b[M \xc4\x80!".as_slice(),
                300,
            ),
            (
                b"\x1b[?1000h\x1b[?1006h".as_slice(),
                VtMouseEvent {
                    modifiers: VtMouseModifiers {
                        shift: true,
                        control: true,
                        alt: true,
                    },
                    ..test_mouse_event(
                        VtMouseAction::Release,
                        Some(VtMouseButton::Right),
                        15.0,
                        10.0,
                    )
                },
                b"\x1b[<30;2;1m".as_slice(),
                80,
            ),
            (
                b"\x1b[?1000h\x1b[?1015h".as_slice(),
                test_mouse_event(
                    VtMouseAction::Release,
                    Some(VtMouseButton::Right),
                    15.0,
                    10.0,
                ),
                b"\x1b[35;2;1M".as_slice(),
                80,
            ),
            (
                b"\x1b[?1000h\x1b[?1016h".as_slice(),
                test_mouse_event(
                    VtMouseAction::Press,
                    Some(VtMouseButton::Left),
                    15.25,
                    10.75,
                ),
                b"\x1b[<0;15;11M".as_slice(),
                80,
            ),
        ];

        for (modes, event, expected, cols) in cases {
            let (screen, writes) = mouse_test_screen(cols, 24);
            screen.feed(modes);
            let outcome = screen.send_mouse_event(event).expect("encode mouse event");
            assert!(outcome.output_written, "modes={modes:?}");
            let writes = writes.lock();
            assert_eq!(writes.len(), 1, "modes={modes:?}");
            assert_eq!(writes[0].0.as_slice(), expected, "modes={modes:?}");
            assert_eq!(
                writes[0].1,
                if event.action == VtMouseAction::Release {
                    PtyWriteClass::ReservedControl
                } else {
                    PtyWriteClass::Regular
                }
            );
        }
    }

    #[test]
    fn mouse_encoder_maps_buttons_without_swapping_middle_and_right() {
        let (screen, writes) = mouse_test_screen(80, 24);
        screen.feed(b"\x1b[?1000h\x1b[?1006h");

        for (button, code) in [
            (VtMouseButton::Left, 0),
            (VtMouseButton::Middle, 1),
            (VtMouseButton::Right, 2),
            (VtMouseButton::Button4, 64),
            (VtMouseButton::Button5, 65),
        ] {
            assert!(
                screen
                    .send_mouse_event(test_mouse_event(
                        VtMouseAction::Press,
                        Some(button),
                        5.0,
                        5.0,
                    ))
                    .expect("encode button press")
                    .output_written
            );
            assert_eq!(
                writes
                    .lock()
                    .last()
                    .expect("captured button press")
                    .0
                    .as_slice(),
                format!("\x1b[<{code};1;1M").into_bytes()
            );
        }

        screen
            .send_mouse_event(test_mouse_event(
                VtMouseAction::Release,
                Some(VtMouseButton::Left),
                5.0,
                5.0,
            ))
            .expect("encode release");
        assert_eq!(
            writes.lock().last().expect("captured release").1,
            PtyWriteClass::ReservedControl
        );
    }

    #[test]
    fn mouse_encoder_applies_x10_gating_and_effective_terminal_state() {
        let (screen, writes) = mouse_test_screen(300, 24);
        screen.feed(b"\x1b[?9h");
        let event = VtMouseEvent {
            modifiers: VtMouseModifiers {
                shift: true,
                control: true,
                alt: true,
            },
            ..test_mouse_event(VtMouseAction::Press, Some(VtMouseButton::Left), 15.0, 10.0)
        };
        assert!(screen.send_mouse_event(event).unwrap().output_written);
        assert_eq!(writes.lock()[0].0.as_slice(), b"\x1b[M \"!");

        assert!(
            !screen
                .send_mouse_event(test_mouse_event(
                    VtMouseAction::Release,
                    Some(VtMouseButton::Left),
                    15.0,
                    10.0,
                ))
                .unwrap()
                .output_written
        );
        assert!(
            !screen
                .send_mouse_event(test_mouse_event(
                    VtMouseAction::Press,
                    Some(VtMouseButton::Left),
                    2235.0,
                    10.0,
                ))
                .unwrap()
                .output_written
        );
        assert_eq!(writes.lock().len(), 1);

        // The public bitset route still sees 1002, but DECRST 1000 made the
        // terminal's last-write-wins effective event mode `none`.
        screen.feed(b"\x1b[?9l\x1b[?1000h\x1b[?1002h\x1b[?1000l");
        assert!(screen.mouse_tracking_active());
        assert!(
            !screen
                .send_mouse_event(test_mouse_event(
                    VtMouseAction::Press,
                    Some(VtMouseButton::Left),
                    5.0,
                    5.0,
                ))
                .unwrap()
                .output_written
        );
        assert_eq!(writes.lock().len(), 1);
    }

    #[test]
    fn mouse_encoder_deduplicates_motion_until_terminal_output_resynchronizes() {
        let (screen, writes) = mouse_test_screen(80, 24);
        screen.feed(b"\x1b[?1002h\x1b[?1006h");
        let drag = test_mouse_event(VtMouseAction::Motion, Some(VtMouseButton::Left), 5.0, 5.0);

        assert!(screen.send_mouse_event(drag).unwrap().output_written);
        assert!(!screen.send_mouse_event(drag).unwrap().output_written);
        screen.set_mouse_geometry(800, 480, 10, 20);
        assert!(!screen.send_mouse_event(drag).unwrap().output_written);

        // The pinned setopt_from_terminal implementation clears last-cell
        // state on each feed, even when the parsed bytes do not change modes.
        screen.feed(b"x");
        assert!(screen.send_mouse_event(drag).unwrap().output_written);
        assert_eq!(writes.lock().len(), 2);
    }

    #[test]
    fn mouse_encoder_reports_no_button_motion_in_any_event_mode() {
        let (screen, writes) = mouse_test_screen(80, 24);
        screen.feed(b"\x1b[?1003h\x1b[?1006h");

        assert!(
            screen
                .send_mouse_event(test_mouse_event(VtMouseAction::Motion, None, 5.0, 5.0))
                .unwrap()
                .output_written
        );
        assert_eq!(writes.lock()[0].0.as_slice(), b"\x1b[<35;1;1M");
    }

    #[test]
    fn key_encoder_tracks_legacy_terminal_modes() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let output_for_callback = output.clone();
        let screen = VtScreen::new_with_write_pty(
            80,
            24,
            None,
            Some(Arc::new(move |bytes, _| {
                output_for_callback.lock().extend_from_slice(bytes);
                Ok(())
            })),
        )
        .expect("create vt screen");

        let ctrl_c = VtKeyEvent {
            key: "c",
            text: "c",
            unshifted_codepoint: Some('c'),
            action: VtKeyAction::Press,
            modifiers: VtKeyModifiers {
                control: true,
                ..VtKeyModifiers::default()
            },
            consumed_modifiers: VtKeyModifiers::default(),
        };
        assert!(
            screen
                .send_key(&ctrl_c)
                .expect("encode Ctrl-C")
                .output_accepted
        );
        assert_eq!(output.lock().as_slice(), b"\x03");

        output.lock().clear();
        screen.feed(b"\x1b[?1h");
        let up = VtKeyEvent {
            key: "up",
            text: "",
            unshifted_codepoint: None,
            action: VtKeyAction::Press,
            modifiers: VtKeyModifiers::default(),
            consumed_modifiers: VtKeyModifiers::default(),
        };
        assert!(
            screen
                .send_key(&up)
                .expect("encode DECCKM up")
                .output_accepted
        );
        assert_eq!(output.lock().as_slice(), b"\x1bOA");

        output.lock().clear();
        let alt_backspace = VtKeyEvent {
            key: "backspace",
            text: "",
            unshifted_codepoint: None,
            action: VtKeyAction::Press,
            modifiers: VtKeyModifiers {
                alt: true,
                ..VtKeyModifiers::default()
            },
            consumed_modifiers: VtKeyModifiers::default(),
        };
        assert!(
            screen
                .send_key(&alt_backspace)
                .expect("encode Alt-Backspace")
                .output_accepted
        );
        assert_eq!(output.lock().as_slice(), b"\x1b\x7f");

        output.lock().clear();
        let shift_tab = VtKeyEvent {
            key: "tab",
            text: "",
            unshifted_codepoint: None,
            action: VtKeyAction::Press,
            modifiers: VtKeyModifiers {
                shift: true,
                ..VtKeyModifiers::default()
            },
            consumed_modifiers: VtKeyModifiers::default(),
        };
        assert!(
            screen
                .send_key(&shift_tab)
                .expect("encode Shift-Tab")
                .output_accepted
        );
        assert_eq!(output.lock().as_slice(), b"\x1b[Z");

        output.lock().clear();
        let mut space = VtKeyEvent {
            key: "space",
            text: " ",
            unshifted_codepoint: Some(' '),
            action: VtKeyAction::Press,
            modifiers: VtKeyModifiers::default(),
            consumed_modifiers: VtKeyModifiers::default(),
        };
        assert!(
            screen
                .send_key(&space)
                .expect("encode Space")
                .output_accepted
        );
        assert_eq!(output.lock().as_slice(), b" ");

        output.lock().clear();
        space.modifiers.control = true;
        assert!(
            screen
                .send_key(&space)
                .expect("encode Ctrl-Space")
                .output_accepted
        );
        assert_eq!(output.lock().as_slice(), b"\x00");
    }

    #[test]
    fn key_encoder_handles_kitty_press_repeat_and_release() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let output_for_callback = output.clone();
        let screen = VtScreen::new_with_write_pty(
            80,
            24,
            None,
            Some(Arc::new(move |bytes, _| {
                output_for_callback.lock().extend_from_slice(bytes);
                Ok(())
            })),
        )
        .expect("create vt screen");
        screen.feed(b"\x1b[>3u");

        for (action, expected) in [
            (VtKeyAction::Press, b"a".as_slice()),
            (VtKeyAction::Repeat, b"a".as_slice()),
            (VtKeyAction::Release, b"\x1b[97;1:3u".as_slice()),
        ] {
            output.lock().clear();
            let event = VtKeyEvent {
                key: "a",
                text: "a",
                unshifted_codepoint: Some('a'),
                action,
                modifiers: VtKeyModifiers::default(),
                consumed_modifiers: VtKeyModifiers::default(),
            };
            let outcome = screen.send_key(&event).expect("encode Kitty key event");
            assert!(outcome.output_accepted);
            assert!(outcome.report_releases);
            assert_eq!(output.lock().as_slice(), expected);
        }
    }

    #[test]
    fn key_encoder_propagates_pty_write_failures() {
        let screen = VtScreen::new_with_write_pty(
            80,
            24,
            None,
            Some(Arc::new(|_, _| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "test PTY is closed",
                ))
            })),
        )
        .expect("create vt screen");
        let event = VtKeyEvent {
            key: "c",
            text: "c",
            unshifted_codepoint: Some('c'),
            action: VtKeyAction::Press,
            modifiers: VtKeyModifiers::default(),
            consumed_modifiers: VtKeyModifiers::default(),
        };

        let err = screen
            .send_key(&event)
            .expect_err("surface write must fail");
        assert!(err.to_string().contains("test PTY is closed"));
    }

    #[test]
    fn raw_input_cannot_be_overtaken_by_a_terminal_reply() {
        let gate = Arc::new((Mutex::new(false), parking_lot::Condvar::new()));
        let callback_gate = gate.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let callback_writes = writes.clone();
        let screen = Arc::new(
            VtScreen::new_with_write_pty(
                80,
                24,
                None,
                Some(Arc::new(move |bytes, priority| {
                    if bytes == b"user" {
                        entered_tx.send(()).expect("signal raw input callback");
                        let (open, ready) = &*callback_gate;
                        let mut open = open.lock();
                        while !*open {
                            ready.wait(&mut open);
                        }
                    }
                    callback_writes.lock().push((bytes.to_vec(), priority));
                    Ok(())
                })),
            )
            .expect("create vt screen"),
        );

        let input_screen = screen.clone();
        let input =
            std::thread::spawn(move || input_screen.write_input(b"user").expect("write raw input"));
        entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("raw input callback must start");

        let reply_screen = screen.clone();
        let reply = std::thread::spawn(move || reply_screen.feed(b"\x1b[5n"));
        let deadline = Instant::now() + Duration::from_secs(2);
        while screen.inner.try_lock().is_some() {
            assert!(
                Instant::now() < deadline,
                "terminal reply did not reach the ordering gate"
            );
            std::thread::yield_now();
        }

        let (open, ready) = &*gate;
        *open.lock() = true;
        ready.notify_all();
        input.join().expect("raw input thread");
        reply.join().expect("terminal reply thread");

        let writes = writes.lock();
        assert_eq!(writes[0], (b"user".to_vec(), PtyWriteClass::Regular));
        assert_eq!(writes[1].1, PtyWriteClass::ReservedControl);
        assert!(writes[1].0.starts_with(b"\x1b[0n"));
    }

    #[test]
    fn failed_terminal_reply_marks_the_session_desynchronized() {
        let screen = VtScreen::new_with_write_pty(
            80,
            24,
            None,
            Some(Arc::new(|_, class| {
                assert_eq!(class, PtyWriteClass::ReservedControl);
                Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "reserved PTY capacity exhausted",
                ))
            })),
        )
        .expect("create vt screen");

        screen.feed(b"\x1b[5n");

        assert!(screen.is_write_desynchronized());
    }

    #[test]
    fn vt_screen_configures_line_scrollback_limit() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");
        let inner = screen.inner.lock();
        let mut max_lines = 0_usize;
        let rc = unsafe {
            ghostty_terminal_get(
                inner.terminal,
                GhosttyTerminalData::ScrollbackMaxLines,
                &mut max_lines as *mut _ as *mut c_void,
            )
        };

        assert_eq!(rc, 0);
        assert_eq!(max_lines, 10_000);
    }

    #[test]
    fn terminal_clipboard_writes_require_opt_in_and_coalesce_pending_text() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");
        let policy = Arc::new(ClipboardWritePolicy::new(true));
        screen.set_clipboard_write_policy(policy.clone());
        let clipboard_write_limit = || {
            let inner = screen.inner.lock();
            let mut limit = usize::MAX;
            let rc = unsafe {
                ghostty_terminal_get(
                    inner.terminal,
                    GhosttyTerminalData::ClipboardWriteMaxBytes,
                    &mut limit as *mut _ as *mut c_void,
                )
            };
            assert_eq!(rc, 0);
            limit
        };

        assert_eq!(clipboard_write_limit(), 0);
        screen.feed(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(screen.take_clipboard_write(), None);

        screen
            .set_clipboard_write_enabled(true)
            .expect("enable clipboard writes");
        assert_eq!(clipboard_write_limit(), CLIPBOARD_WRITE_LIMIT_BYTES);
        screen.feed(b"\x1b]52;c;aGVsbG8=\x07");
        screen.feed(b"\x1b]52;c;d29ybGQ=\x07");
        assert_eq!(screen.take_clipboard_write().as_deref(), Some("world"));
        assert_eq!(screen.take_clipboard_write(), None);

        screen.feed(b"\x1b]52;c;/w==\x07");
        assert_eq!(screen.take_clipboard_write(), None);

        screen.feed(b"\x1b]52;c;\x07");
        assert_eq!(screen.take_clipboard_write().as_deref(), Some(""));

        screen.feed(b"\x1b]52;c;YmxvY2tlZA==\x07");
        policy.set_enabled(false);
        assert_eq!(screen.take_clipboard_write(), None);
        screen.feed(b"\x1b]52;c;ZGVuaWVk\x07");
        policy.set_enabled(true);
        assert_eq!(screen.take_clipboard_write(), None);

        screen.feed(b"\x1b]52;c;YWdhaW4=\x07");
        screen
            .set_clipboard_write_enabled(false)
            .expect("disable clipboard writes");
        assert_eq!(clipboard_write_limit(), 0);
        assert_eq!(screen.take_clipboard_write(), None);
    }

    #[test]
    fn terminal_desktop_notifications_are_bounded_on_utf8_boundaries() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");
        let title = format!("{}é", "a".repeat(62));
        let sequence = format!("\x1b]777;notify;{title};Needs attention\x07");

        screen.feed(sequence.as_bytes());

        let notification = screen
            .take_desktop_notification()
            .expect("desktop notification");
        assert_eq!(notification.title, "a".repeat(62));
        assert_eq!(notification.body, "Needs attention");
        assert_eq!(screen.take_desktop_notification(), None);
    }

    #[test]
    fn vt_screen_queries_modes_through_terminal_data() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        assert!(!screen.is_bracketed_paste());
        screen.feed(b"\x1b[?2004h");
        assert!(screen.is_bracketed_paste());
        screen.feed(b"\x1b[?2004l");
        assert!(!screen.is_bracketed_paste());
    }

    #[test]
    fn vt_screen_queries_semantic_prompt_state() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        assert_eq!(screen.prompt_state(), crate::TerminalPromptState::default());
        screen.feed(b"\x1b]133;A\x07$ \x1b]133;B\x07");
        assert_eq!(
            screen.prompt_state(),
            crate::TerminalPromptState {
                cursor_at_prompt: true,
                output_generation: 1,
            }
        );

        screen.feed(b"echo test\r\n\x1b]133;C\x07");
        assert_eq!(
            screen.prompt_state(),
            crate::TerminalPromptState {
                cursor_at_prompt: false,
                output_generation: 2,
            }
        );
        screen.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07");
        assert_eq!(
            screen.prompt_state(),
            crate::TerminalPromptState {
                cursor_at_prompt: true,
                output_generation: 3,
            }
        );

        screen.feed(b"\x1b[?1049h");
        assert_eq!(
            screen.prompt_state(),
            crate::TerminalPromptState {
                cursor_at_prompt: false,
                output_generation: 4,
            }
        );
    }

    #[test]
    fn paste_applies_terminal_modes_and_requires_confirmation_for_command_injection() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let output_for_callback = output.clone();
        let callback_calls = Arc::new(Mutex::new(0_usize));
        let callback_calls_for_callback = callback_calls.clone();
        let screen = VtScreen::new_with_write_pty(
            80,
            24,
            None,
            Some(Arc::new(move |bytes, _| {
                *callback_calls_for_callback.lock() += 1;
                output_for_callback.lock().extend_from_slice(bytes);
                Ok(())
            })),
        )
        .expect("create vt screen");

        assert_eq!(
            screen
                .paste_text("echo safe", VtPasteSource::Clipboard, false)
                .expect("safe paste"),
            VtPasteResult::Accepted
        );
        assert_eq!(output.lock().as_slice(), b"echo safe");

        output.lock().clear();
        let unsafe_text = "echo first\necho second";
        assert_eq!(
            screen
                .paste_text(unsafe_text, VtPasteSource::Clipboard, false)
                .expect("check unsafe paste"),
            VtPasteResult::RequiresConfirmation
        );
        assert!(output.lock().is_empty(), "rejected paste wrote to the PTY");

        assert_eq!(
            screen
                .paste_text(unsafe_text, VtPasteSource::Clipboard, true)
                .expect("confirmed paste"),
            VtPasteResult::Accepted
        );
        assert_eq!(output.lock().as_slice(), b"echo first\recho second");

        output.lock().clear();
        *callback_calls.lock() = 0;
        screen.feed(b"\x1b[?2004h");
        assert_eq!(
            screen
                .paste_text("first\nsecond", VtPasteSource::Clipboard, false)
                .expect("bracketed paste"),
            VtPasteResult::Accepted
        );
        assert_eq!(output.lock().as_slice(), b"\x1b[200~first\nsecond\x1b[201~");
        assert_eq!(
            *callback_calls.lock(),
            1,
            "one logical paste must reach the bounded host queue atomically"
        );
    }

    #[test]
    fn kitty_paste_grants_share_the_current_clipboard_without_becoming_reusable() {
        fn grant_read(event: &[u8]) -> Vec<u8> {
            let password_prefix = b"\x1b]5522;type=read:status=OK:pw=";
            assert!(event.starts_with(password_prefix));
            let password_end = event[password_prefix.len()..]
                .windows(2)
                .position(|window| window == b"\x1b\\")
                .expect("paste event password terminator")
                + password_prefix.len();

            let mut read = b"\x1b]5522;type=read:pw=".to_vec();
            read.extend_from_slice(&event[password_prefix.len()..password_end]);
            read.extend_from_slice(b":name=UGFzdGUgZXZlbnQ=;dGV4dC9wbGFpbg==\x1b\\");
            read
        }

        fn verify_redemption_order(order: [usize; 2]) {
            let output = Arc::new(Mutex::new(Vec::new()));
            let output_for_callback = output.clone();
            let screen = VtScreen::new_with_write_pty(
                80,
                24,
                None,
                Some(Arc::new(move |bytes, _| {
                    output_for_callback.lock().extend_from_slice(bytes);
                    Ok(())
                })),
            )
            .expect("create vt screen");
            screen.feed(b"\x1b[?5522h");

            assert_eq!(
                screen
                    .paste_text("dropped path", VtPasteSource::Text, false)
                    .expect("text insertion with Kitty paste events enabled"),
                VtPasteResult::Accepted
            );
            assert_eq!(output.lock().as_slice(), b"dropped path");

            let mut reads = Vec::new();
            for text in ["first clipboard", "current clipboard"] {
                output.lock().clear();
                assert_eq!(
                    screen
                        .paste_text(text, VtPasteSource::Clipboard, false)
                        .expect("Kitty paste event"),
                    VtPasteResult::Accepted
                );
                let event = output.lock().clone();
                assert!(
                    !event
                        .windows(text.len())
                        .any(|window| window == text.as_bytes())
                );
                reads.push(grant_read(&event));
            }

            for index in order {
                output.lock().clear();
                screen.feed(&reads[index]);
                let response = output.lock().clone();
                assert!(
                    response
                        .windows(b"Y3VycmVudCBjbGlwYm9hcmQ=".len())
                        .any(|window| window == b"Y3VycmVudCBjbGlwYm9hcmQ="),
                    "valid paste grant did not expose the current clipboard"
                );

                output.lock().clear();
                screen.feed(&reads[index]);
                assert!(
                    output
                        .lock()
                        .windows(b"status=EPERM".len())
                        .any(|window| window == b"status=EPERM"),
                    "one-time paste grant was reusable"
                );
            }

            assert_eq!(
                screen
                    .inner
                    .lock()
                    .callback_state
                    .clipboard_text
                    .lock()
                    .as_deref(),
                Some("current clipboard"),
                "grant redemption discarded the current clipboard"
            );
        }

        verify_redemption_order([0, 1]);
        verify_redemption_order([1, 0]);
    }

    #[test]
    fn terminal_paste_rejects_payloads_over_the_memory_limit() {
        let screen = VtScreen::new_with_write_pty(
            80,
            24,
            None,
            Some(Arc::new(|_, _| {
                panic!("oversized paste reached the PTY callback")
            })),
        )
        .expect("create vt screen");
        let oversized = "x".repeat(TERMINAL_PASTE_LIMIT_BYTES + 1);

        let err = screen
            .paste_text(&oversized, VtPasteSource::Clipboard, false)
            .expect_err("oversized paste must be rejected");

        assert!(err.to_string().contains("safety limit"));
    }

    #[test]
    fn vt_screen_coalesces_bell_effects_until_the_host_drains_them() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        screen.feed(b"\x1b]2;OSC terminator\x07");
        assert!(
            !screen.take_bell(),
            "an OSC terminator is not a bell effect"
        );

        screen.feed(b"\x07\x07");
        assert!(screen.take_bell());
        assert!(!screen.take_bell());
    }

    #[test]
    fn vt_screen_tracks_latest_progress_report() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        screen.feed(b"\x1b]9;4;1;42\x07\x1b]9;4;4;75\x1b\\");
        assert_eq!(screen.progress(), Some(TerminalProgress::Paused(Some(75))));

        screen.feed(b"\x1b]9;4;2");
        screen.feed(b";7\x1b\\");
        assert_eq!(screen.progress(), Some(TerminalProgress::Error(Some(7))));

        screen.feed(b"\x1b]9;4;3\x07");
        assert_eq!(screen.progress(), Some(TerminalProgress::Indeterminate));

        screen.feed(b"\x1b]9;4;0\x07");
        assert_eq!(screen.progress(), None);
    }

    #[test]
    fn terminal_progress_expires_without_a_fresh_report() {
        let progress = Some(TerminalProgress::Running(Some(42)));
        let reported_at = 10;
        let encoded = encode_timed_terminal_progress(progress, reported_at);
        let timeout = u64::try_from(TERMINAL_PROGRESS_TIMEOUT.as_millis()).unwrap();

        assert_eq!(
            decode_timed_terminal_progress(encoded, reported_at + timeout - 1),
            progress
        );
        assert_eq!(
            decode_timed_terminal_progress(encoded, reported_at + timeout),
            None
        );
    }

    #[test]
    fn vt_screen_tracks_title_effects_without_a_pty_writer() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        assert_eq!(screen.reported_title(), None);
        assert_eq!(screen.title(), None);

        screen.feed(b"\x1b]0;first title\x07\x1b]2;final title\x1b\\");

        assert_eq!(
            screen.reported_title(),
            Some(Some("final title".to_owned()))
        );
        assert_eq!(screen.title().as_deref(), Some("final title"));

        screen.feed(b"\x1b]2;\x07");

        assert_eq!(screen.reported_title(), Some(None));
        assert_eq!(screen.title(), None);
    }

    #[test]
    fn vt_screen_reports_osc7_current_dir() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        assert_eq!(screen.current_dir(), None);
        screen.feed(b"\x1b]7;file:///tmp/con-vt-cwd\x07");

        assert_eq!(screen.current_dir().as_deref(), Some("/tmp/con-vt-cwd"));
    }

    #[test]
    fn vt_screen_coalesces_and_clears_current_dir_effects_without_a_pty_writer() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        screen.feed(b"\x1b]7;file:///tmp/first\x07\x1b]9;9;/tmp/final\x1b\\");

        assert_eq!(screen.current_dir().as_deref(), Some("/tmp/final"));

        screen.feed(b"\x1b]7;\x07");

        assert_eq!(screen.current_dir(), None);
    }

    #[test]
    fn vt_screen_reports_windows_osc7_current_dir() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        screen.feed(b"\x1b]7;file:///C:/Users/WeyGu/dev/con-terminal\x07");

        assert_eq!(
            screen.current_dir().as_deref(),
            Some("C:\\Users\\WeyGu\\dev\\con-terminal")
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn vt_screen_reports_localhost_windows_osc7_current_dir() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        screen.feed(b"\x1b]7;file://localhost/C:/Users/WeyGu/dev/con-terminal\x07");

        assert_eq!(
            screen.current_dir().as_deref(),
            Some("C:\\Users\\WeyGu\\dev\\con-terminal")
        );
    }

    #[test]
    fn vt_screen_reports_split_osc7_current_dir() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        screen.feed(b"\x1b]7;file:///home/me/con");
        screen.feed(b"%20terminal\x1b\\");

        assert_eq!(
            screen.current_dir().as_deref(),
            Some("/home/me/con terminal")
        );
    }

    #[test]
    fn vt_screen_preserves_whitespace_in_bare_current_dir() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        screen.feed(b"\x1b]9;9; /tmp/con cwd \x07");

        assert_eq!(screen.current_dir().as_deref(), Some(" /tmp/con cwd "));
    }

    #[test]
    fn normal_screen_preserves_explicit_palette_black_background() {
        let theme = catppuccin_like_theme();
        let screen = VtScreen::new(4, 2, Some(&theme)).expect("create vt screen");

        screen.feed(b"\x1b[40mX");
        let snapshot = screen.snapshot();

        assert!(!snapshot.alternate_screen);
        let cell = snapshot.cells.first().expect("first cell");
        assert_eq!(cell.codepoint, 'X' as u32);
        assert_eq!(cell.bg, rgba([0x45, 0x47, 0x5A], 0xFF));
    }

    #[test]
    fn alternate_screen_uses_default_canvas_for_palette_black_background() {
        let theme = catppuccin_like_theme();
        let screen = VtScreen::new(4, 2, Some(&theme)).expect("create vt screen");

        screen.feed(b"\x1b[?1049h\x1b[H\x1b[40mX");
        let snapshot = screen.snapshot();

        assert!(snapshot.alternate_screen);
        let cell = snapshot.cells.first().expect("first cell");
        assert_eq!(cell.codepoint, 'X' as u32);
        assert_eq!(cell.bg, rgba([0x1E, 0x1E, 0x2E], 0x00));
    }

    fn catppuccin_like_theme() -> ThemeColors {
        let mut ansi = [[0u8; 3]; 16];
        ansi[0] = [0x45, 0x47, 0x5A];
        ansi[7] = [0xBA, 0xC2, 0xDE];
        ansi[15] = [0xCD, 0xD6, 0xF4];
        ThemeColors::from_ansi16([0xCD, 0xD6, 0xF4], [0x1E, 0x1E, 0x2E], ansi)
    }

    fn rgba(rgb: [u8; 3], alpha: u8) -> u32 {
        ((rgb[0] as u32) << 24) | ((rgb[1] as u32) << 16) | ((rgb[2] as u32) << 8) | alpha as u32
    }
}
