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
//!   - `ghostty_render_state_update(state, terminal)` to refresh
//!   - `ghostty_render_state_get(state, DATA_ROW_ITERATOR, &iter)` to bind iterator
//!   - while `row_iterator_next(iter)` is true:
//!       - `row_get(iter, DIRTY, &dirty)`, skip if false
//!       - `row_get(iter, CELLS, &cells)` to bind cells iterator to the current row
//!       - while `row_cells_next(cells)` is true:
//!           - `row_cells_get(cells, RAW|STYLE|BG|FG, &out)`
//!
//! All `_next` functions return `bool`. The `_get` family uses an enum
//! key and writes to a typed `void*` out; key→type contract is per
//! upstream header comments.
//!
//! libghostty-vt is NOT thread-safe; we serialize via a Mutex. The
//! renderer reads a cloned `ScreenSnapshot` so the parser lock is
//! released between feeds and frames.

#![allow(non_camel_case_types, dead_code)]

use std::collections::HashMap;
use std::io::Cursor as IoCursor;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use crate::stub::GhosttyScrollbar;

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
pub type GhosttyKittyGraphics = *mut c_void;
pub type GhosttyKittyGraphicsImage = *mut c_void;
pub type GhosttyKittyGraphicsPlacementIterator = *mut c_void;
pub type GhosttyAllocator = c_void;
pub type GhosttyResult = c_int;

const GHOSTTY_SUCCESS: GhosttyResult = 0;
const GHOSTTY_OUT_OF_SPACE: GhosttyResult = -3;
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
    ScrollbackMaxLines = 35,
    Mode = 37,
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
    PwdChanged = 25,
    ScrollbackMaxLines = 28,
    ClipboardRead = 38,
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
#[derive(Debug, Clone, Copy, Default)]
pub struct GhosttyString {
    pub ptr: *const u8,
    pub len: usize,
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
const _: [(); 56] = [(); std::mem::size_of::<GhosttyClipboardReadReply>()];
const _: [(); 80] = [(); std::mem::size_of::<GhosttyClipboardRead>()];
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
    pub fn ghostty_terminal_paste(
        terminal: GhosttyTerminal,
        paste: *const GhosttyPaste,
        out_written: *mut bool,
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
    pub fn ghostty_render_state_get(
        state: GhosttyRenderState,
        key: GhosttyRenderStateData,
        out: *mut c_void,
    ) -> GhosttyResult;

    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const GhosttyAllocator,
        out_iter: *mut GhosttyRowIterator,
    ) -> GhosttyResult;
    pub fn ghostty_render_state_row_iterator_free(iter: GhosttyRowIterator);
    /// Returns `bool` per upstream signature. Rust `bool` is 1 byte —
    /// matches MSVC/gcc/clang C99 `_Bool` layout.
    pub fn ghostty_render_state_row_iterator_next(iter: GhosttyRowIterator) -> bool;
    pub fn ghostty_render_state_row_get(
        iter: GhosttyRowIterator,
        key: GhosttyRenderStateRowData,
        out: *mut c_void,
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

    // Cell accessor (`screen.h`). Decodes fields out of the opaque
    // `GhosttyCell` u64 we get from row_cells RAW.
    pub fn ghostty_cell_get(
        cell: GhosttyCell,
        key: GhosttyCellData,
        out: *mut c_void,
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
    pub kitty_placements: Arc<[KittyPlacement]>,
    pub dirty_rows: Vec<u16>,
    pub cursor: Cursor,
    pub alternate_screen: bool,
    pub scrollbar: Option<GhosttyScrollbar>,
    pub title: Option<String>,
    pub generation: u64,
}

// ── Safe wrapper ───────────────────────────────────────────────────────

pub struct VtScreen {
    inner: Arc<Mutex<VtInner>>,
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
    /// Serializes host callback entry. `send_key` acquires this while it still
    /// owns the VT mutex, then releases the VT mutex before invoking the host;
    /// a parser reply therefore cannot overtake the already-encoded key.
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
}

struct VtInner {
    terminal: GhosttyTerminal,
    render_state: GhosttyRenderState,
    row_iter: GhosttyRowIterator,
    row_cells: GhosttyRowCells,
    key_encoder: GhosttyKeyEncoder,
    key_event: GhosttyKeyEvent,
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
    force_full_snapshot: bool,
    scratch_cols: u16,
    scratch_rows: u16,
    scratch: Vec<Cell>,
    last_cursor: Cursor,
}

unsafe impl Send for VtInner {}

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

        let mut callback_state = Box::new(VtCallbackState {
            write_pty,
            write_order: Arc::new(Mutex::new(())),
            enquiry_response: b"con".to_vec().into_boxed_slice(),
            clipboard_text: Mutex::new(None),
            write_failed: Arc::new(AtomicBool::new(false)),
            pending_paste_write: Mutex::new(None),
            rows: AtomicU16::new(rows),
            cols: AtomicU16::new(cols),
            cell_width: AtomicU32::new(1),
            cell_height: AtomicU32::new(1),
            dark_mode: AtomicBool::new(false),
            device_attributes: default_device_attributes(),
            metadata_dirty: AtomicU8::new(0),
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
                GhosttyTerminalOption::TitleChanged,
                vt_title_changed_callback as *const c_void,
                "TITLE_CHANGED",
            ),
            (
                GhosttyTerminalOption::PwdChanged,
                vt_pwd_changed_callback as *const c_void,
                "PWD_CHANGED",
            ),
        ];
        for (option, callback, label) in effect_callbacks {
            let rc = unsafe { ghostty_terminal_set(terminal, option, callback) };
            if rc != 0 {
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_terminal_set({label}) failed: rc={rc}");
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

        if let Some(theme) = theme {
            unsafe { apply_theme_to_terminal(terminal, theme) };
        }

        Ok(Self {
            inner: Arc::new(Mutex::new(VtInner {
                terminal,
                render_state,
                row_iter,
                row_cells,
                key_encoder,
                key_event,
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
                force_full_snapshot: true,
                scratch_cols: cols,
                scratch_rows: rows,
                scratch: Vec::with_capacity(cols as usize * rows as usize),
                last_cursor: Cursor::default(),
            })),
        })
    }

    /// Replace the default fg/bg/palette. Bumps the snapshot
    /// generation so the next prepaint repaints with the new colors.
    pub fn set_theme(&self, theme: &ThemeColors) {
        let mut inner = self.inner.lock();
        unsafe { apply_theme_to_terminal(inner.terminal, theme) };
        inner.force_full_snapshot = true;
        inner.generation = inner.generation.wrapping_add(1);
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
        refresh_terminal_metadata(&mut inner);
        inner.generation = inner.generation.wrapping_add(1);
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
        let total = cols as usize * rows as usize;
        inner.scratch.clear();
        inner.scratch.resize(total, Cell::default());
        inner.scratch_cols = cols;
        inner.scratch_rows = rows;
        inner.force_full_snapshot = true;
        inner.generation = inner.generation.wrapping_add(1);
        Ok(())
    }

    pub fn snapshot(&self) -> ScreenSnapshot {
        let snapshot_started = perf_trace_enabled().then(Instant::now);
        let mut inner = self.inner.lock();

        let fallback_cols = inner.cols;
        let fallback_rows = inner.rows;

        if inner.render_state.is_null() {
            // Render-state path disabled — return empty snapshot. The
            // renderer still clears the pane to the background.
            return ScreenSnapshot {
                cols: fallback_cols,
                rows: fallback_rows,
                cells: Vec::new(),
                kitty_placements: Arc::from([]),
                dirty_rows: Vec::new(),
                cursor: Cursor::default(),
                alternate_screen: false,
                scrollbar: None,
                title: None,
                generation: inner.generation,
            };
        }

        // SAFETY: state + terminal valid for the lifetime of `inner`.
        let rc = unsafe { ghostty_render_state_update(inner.render_state, inner.terminal) };
        if rc != 0 {
            log::warn!("ghostty_render_state_update rc={rc}");
        }

        // Palette defaults. Cells with no explicit SGR color report
        // FG_COLOR / BG_COLOR as (0,0,0) — the renderer is expected to
        // substitute the terminal's default foreground/background from
        // the render state. Without this, the pwsh banner (and any
        // unstyled text) renders black-on-black.
        let mut default_fg = GhosttyColorRgb {
            r: 0xCC,
            g: 0xCC,
            b: 0xCC,
        };
        let mut default_bg = GhosttyColorRgb::default();
        // SAFETY: out params typed as GhosttyColorRgb per render.h.
        unsafe {
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::ColorForeground,
                &mut default_fg as *mut _ as *mut c_void,
            );
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::ColorBackground,
                &mut default_bg as *mut _ as *mut c_void,
            );
        }

        // Ghostty's render-state dimensions can lag the host resize by a
        // frame or two. Snapshot the actual render-state geometry so we
        // don't invent blank tail rows from our requested size while the
        // terminal catches up asynchronously.
        let mut cols = fallback_cols;
        let mut rows = fallback_rows;
        unsafe {
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::Cols,
                &mut cols as *mut _ as *mut c_void,
            );
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::Rows,
                &mut rows as *mut _ as *mut c_void,
            );
        }
        cols = cols.max(1);
        rows = rows.max(1);

        let mut active_screen = GhosttyTerminalScreen::Primary;
        let active_screen_rc = unsafe {
            ghostty_terminal_get(
                inner.terminal,
                GhosttyTerminalData::ActiveScreen,
                &mut active_screen as *mut _ as *mut c_void,
            )
        };
        let alternate_screen =
            active_screen_rc == 0 && active_screen == GhosttyTerminalScreen::Alternate;

        let mut force_all_dirty = inner.force_full_snapshot;
        let mut full_redraw = force_all_dirty;
        let mut state_dirty = GhosttyRenderStateDirty::False;
        // SAFETY: DIRTY out param is sized for the enum.
        unsafe {
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::Dirty,
                &mut state_dirty as *mut _ as *mut c_void,
            );
        }
        if state_dirty == GhosttyRenderStateDirty::Full {
            full_redraw = true;
        }

        let total = cols as usize * rows as usize;
        if inner.scratch.len() != total || inner.scratch_cols != cols || inner.scratch_rows != rows
        {
            inner.scratch.clear();
            inner.scratch.resize(total, Cell::default());
            inner.scratch_cols = cols;
            inner.scratch_rows = rows;
            force_all_dirty = true;
            full_redraw = true;
        }

        let mut dirty_rows: Vec<u16> = Vec::new();

        // Bind the row iterator to the current state.
        // SAFETY: state + iter valid.
        let rc = unsafe {
            ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::RowIterator,
                &mut inner.row_iter as *mut _ as *mut c_void,
            )
        };
        if rc != 0 {
            log::warn!("ghostty_render_state_get(ROW_ITERATOR) rc={rc}");
            return empty_snapshot(cols, rows, inner.generation);
        }

        let mut row_idx: u16 = 0;
        // SAFETY: row_iter valid; `_next` returns bool.
        while unsafe { ghostty_render_state_row_iterator_next(inner.row_iter) } {
            if row_idx >= rows {
                break;
            }

            let mut dirty = GhosttyRenderStateDirty::False;
            // SAFETY: DIRTY out param is sized for the enum.
            unsafe {
                let _ = ghostty_render_state_row_get(
                    inner.row_iter,
                    GhosttyRenderStateRowData::Dirty,
                    &mut dirty as *mut _ as *mut c_void,
                );
            }

            if !full_redraw && dirty == GhosttyRenderStateDirty::False {
                row_idx += 1;
                continue;
            }
            let mut row_changed = force_all_dirty;

            // Bind the cells iterator to the current row.
            // SAFETY: iter + cells valid.
            let rc = unsafe {
                ghostty_render_state_row_get(
                    inner.row_iter,
                    GhosttyRenderStateRowData::Cells,
                    &mut inner.row_cells as *mut _ as *mut c_void,
                )
            };
            if rc != 0 {
                log::warn!("row_get(CELLS) rc={rc} at row {row_idx}");
                row_idx += 1;
                continue;
            }

            let row_start = row_idx as usize * cols as usize;
            let mut col_idx: u16 = 0;
            // SAFETY: cells valid; `_next` returns bool.
            while unsafe { ghostty_render_state_row_cells_next(inner.row_cells) } {
                if col_idx >= cols {
                    break;
                }
                let cell = read_cell(inner.row_cells, default_fg, default_bg, alternate_screen);
                let idx = row_start + col_idx as usize;
                row_changed |= inner.scratch[idx] != cell;
                inner.scratch[idx] = cell;
                col_idx += 1;
            }
            // Clear trailing cells in the row.
            for c in col_idx..cols {
                let idx = row_start + c as usize;
                row_changed |= inner.scratch[idx] != Cell::default();
                inner.scratch[idx] = Cell::default();
            }

            if row_changed {
                dirty_rows.push(row_idx);
            }

            row_idx += 1;
        }

        if full_redraw && row_idx < rows {
            log::warn!(
                "vt snapshot full redraw ended early: iter_rows={row_idx} expected_rows={rows} cols={cols}"
            );
            for trailing_row in row_idx..rows {
                let row_start = trailing_row as usize * cols as usize;
                let row_end = row_start + cols as usize;
                let mut row_changed = force_all_dirty;
                for cell in &mut inner.scratch[row_start..row_end] {
                    row_changed |= *cell != Cell::default();
                    *cell = Cell::default();
                }
                if row_changed {
                    dirty_rows.push(trailing_row);
                }
            }
        }

        inner.force_full_snapshot = false;

        // Cursor read from the render state keys (not the terminal, to
        // stay consistent with the render snapshot).
        let mut visible: bool = false;
        let mut col_u16: u16 = 0;
        let mut row_u16: u16 = 0;
        // SAFETY: out params sized per upstream render.h.
        unsafe {
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::CursorViewportX,
                &mut col_u16 as *mut _ as *mut c_void,
            );
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::CursorViewportY,
                &mut row_u16 as *mut _ as *mut c_void,
            );
            let _ = ghostty_render_state_get(
                inner.render_state,
                GhosttyRenderStateData::CursorVisible,
                &mut visible as *mut _ as *mut c_void,
            );
        }

        let cursor = Cursor {
            col: col_u16,
            row: row_u16,
            visible,
        };
        let previous_cursor = inner.last_cursor;
        if previous_cursor != cursor {
            if previous_cursor.visible && previous_cursor.row < rows {
                push_unique_row(&mut dirty_rows, previous_cursor.row);
            }
            if cursor.visible && cursor.row < rows {
                push_unique_row(&mut dirty_rows, cursor.row);
            }
            inner.last_cursor = cursor;
        }
        dirty_rows.sort_unstable();

        let kitty_placements = snapshot_kitty_placements(&mut inner);
        let clone_started = perf_trace_enabled().then(Instant::now);
        let cells = inner.scratch.clone();
        let clone_elapsed_ms =
            clone_started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        let snapshot = ScreenSnapshot {
            cols,
            rows,
            cells,
            kitty_placements,
            dirty_rows,
            cursor,
            alternate_screen,
            scrollbar: read_scrollbar(inner.terminal),
            title: None,
            generation: inner.generation,
        };

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

        snapshot
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
        inner.force_full_snapshot = true;
        inner.generation = inner.generation.wrapping_add(1);
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
        inner.force_full_snapshot = true;
        inner.generation = inner.generation.wrapping_add(1);
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

fn empty_snapshot(cols: u16, rows: u16, generation: u64) -> ScreenSnapshot {
    ScreenSnapshot {
        cols,
        rows,
        cells: Vec::new(),
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
) -> Cell {
    // RAW here is an **opaque `GhosttyCell` u64 snapshot**, not a packed
    // codepoint. Decode fields via `ghostty_cell_get(cell, KEY, &out)`
    // per `screen.h`. Previous code bitshifted RAW directly and produced
    // nonsense codepoints (U+015C etc. for the "PowerShell" banner).
    let mut raw: GhosttyCell = 0;
    let mut style = GhosttyStyle::new();
    // BG_COLOR / FG_COLOR write a `GhosttyColorRgb` (3 bytes: R, G, B)
    // to the out pointer — NOT a packed u32.
    let mut bg = GhosttyColorRgb::default();
    let mut fg = GhosttyColorRgb::default();

    // SAFETY: out params typed per upstream contract (RAW=GhosttyCell u64,
    // STYLE=GhosttyStyle by value with `size` set to sizeof, BG/FG=GhosttyColorRgb).
    unsafe {
        let _ = ghostty_render_state_row_cells_get(
            cells,
            GhosttyRenderStateRowCellsData::Raw,
            &mut raw as *mut _ as *mut c_void,
        );
        let _ = ghostty_render_state_row_cells_get(
            cells,
            GhosttyRenderStateRowCellsData::Style,
            &mut style as *mut _ as *mut c_void,
        );
        let _ = ghostty_render_state_row_cells_get(
            cells,
            GhosttyRenderStateRowCellsData::BgColor,
            &mut bg as *mut _ as *mut c_void,
        );
        let _ = ghostty_render_state_row_cells_get(
            cells,
            GhosttyRenderStateRowCellsData::FgColor,
            &mut fg as *mut _ as *mut c_void,
        );
    }

    // Gate codepoint decode on HAS_TEXT — blank cells carry a bogus
    // grapheme-tag codepoint we'd otherwise rasterize.
    let mut has_text: bool = false;
    let mut codepoint: u32 = 0;
    // SAFETY: `has_text` is a C `_Bool` (1 byte); `codepoint` is uint32.
    unsafe {
        let _ = ghostty_cell_get(
            raw,
            GhosttyCellData::HasText,
            &mut has_text as *mut _ as *mut c_void,
        );
        if has_text {
            let _ = ghostty_cell_get(
                raw,
                GhosttyCellData::Codepoint,
                &mut codepoint as *mut _ as *mut c_void,
            );
        }
    }

    // Substitute the palette's default fg/bg when the cell's style
    // reports no SGR override (tag == GHOSTTY_STYLE_COLOR_NONE). The
    // row_cells FG_COLOR / BG_COLOR accessors return (0,0,0) for
    // unstyled cells, so without this substitution default-bg cells
    // would paint pure black. Key off the style tag, not the RGB value:
    // explicit black may still resolve to the same RGB as the default.
    const STYLE_COLOR_TAG_NONE: u32 = 0;
    const STYLE_COLOR_TAG_PALETTE: u32 = 1;
    const PALETTE_BLACK: u8 = 0;
    let fg_was_default = style.fg_color.tag == STYLE_COLOR_TAG_NONE;
    let bg_was_default = style.bg_color.tag == STYLE_COLOR_TAG_NONE;
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
    let fg = if fg_was_default { default_fg } else { fg };
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

    Cell {
        codepoint,
        fg: pack(fg, 0xFF),
        bg: pack(bg, bg_alpha),
        attrs,
        _pad: [0; 3],
    }
}

impl Drop for VtScreen {
    fn drop(&mut self) {
        if let Some(mutex) = Arc::get_mut(&mut self.inner) {
            let inner = mutex.get_mut();
            // Free in reverse-creation order: key event/encoder, render
            // helpers, then terminal.
            // SAFETY: unique owner via Arc::get_mut.
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
            if !inner.row_cells.is_null() {
                unsafe { ghostty_render_state_row_cells_free(inner.row_cells) };
                inner.row_cells = std::ptr::null_mut();
            }
            if !inner.row_iter.is_null() {
                unsafe { ghostty_render_state_row_iterator_free(inner.row_iter) };
                inner.row_iter = std::ptr::null_mut();
            }
            if !inner.render_state.is_null() {
                unsafe { ghostty_render_state_free(inner.render_state) };
                inner.render_state = std::ptr::null_mut();
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
            types["GhosttyTerminalOption"]["values"]["CLIPBOARD_READ"].as_i64(),
            Some(GhosttyTerminalOption::ClipboardRead as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["TITLE_CHANGED"].as_i64(),
            Some(GhosttyTerminalOption::TitleChanged as i64)
        );
        assert_eq!(
            types["GhosttyTerminalOption"]["values"]["PWD_CHANGED"].as_i64(),
            Some(GhosttyTerminalOption::PwdChanged as i64)
        );
        for (name, size, align) in [
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
                "GhosttySysImage",
                std::mem::size_of::<GhosttySysImage>(),
                std::mem::align_of::<GhosttySysImage>(),
            ),
            (
                "GhosttyKittyGraphicsPlacementRenderInfo",
                std::mem::size_of::<GhosttyKittyGraphicsPlacementRenderInfo>(),
                std::mem::align_of::<GhosttyKittyGraphicsPlacementRenderInfo>(),
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
            types["GhosttyResult"]["values"]["REJECTED"].as_i64(),
            Some(GHOSTTY_REJECTED as i64)
        );
        assert_eq!(
            types["GhosttyTerminalData"]["values"]["MODE"].as_i64(),
            Some(GhosttyTerminalData::Mode as i64)
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
    fn vt_screen_queries_modes_through_terminal_data() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        assert!(!screen.is_bracketed_paste());
        screen.feed(b"\x1b[?2004h");
        assert!(screen.is_bracketed_paste());
        screen.feed(b"\x1b[?2004l");
        assert!(!screen.is_bracketed_paste());
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
