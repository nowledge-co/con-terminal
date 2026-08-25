//! Shared `libghostty-vt` FFI bindings + render-state snapshot.
//!
//! Rewritten to match the **actual** upstream API at GHOSTTY_REV
//! `8867c37c55b578b9eb4cfaba41cb9023e557176d` — `include/ghostty/vt/*.h`.
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

use std::os::raw::{c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use crate::stub::GhosttyScrollbar;

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
pub type GhosttyAllocator = c_void;
pub type GhosttyResult = c_int;

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
    Scrollbar = 9,
    Title = 12,
    Pwd = 13,
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
    ScrollbackMaxLines = 28,
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

#[derive(Debug, Clone, Default)]
pub struct ScreenSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
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

pub type PtyWriteCallback = Arc<dyn Fn(&[u8]) + Send + Sync + 'static>;

struct VtCallbackState {
    write_pty: PtyWriteCallback,
    enquiry_response: Box<[u8]>,
    rows: AtomicU16,
    cols: AtomicU16,
    cell_width: AtomicU32,
    cell_height: AtomicU32,
    dark_mode: AtomicBool,
    device_attributes: GhosttyDeviceAttributes,
}

struct VtInner {
    terminal: GhosttyTerminal,
    render_state: GhosttyRenderState,
    row_iter: GhosttyRowIterator,
    row_cells: GhosttyRowCells,
    callback_state: Option<Box<VtCallbackState>>,
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

        let mut callback_state = write_pty.map(|write_pty| {
            Box::new(VtCallbackState {
                write_pty,
                enquiry_response: b"con".to_vec().into_boxed_slice(),
                rows: AtomicU16::new(rows),
                cols: AtomicU16::new(cols),
                cell_width: AtomicU32::new(1),
                cell_height: AtomicU32::new(1),
                dark_mode: AtomicBool::new(false),
                device_attributes: default_device_attributes(),
            })
        });
        if let Some(state) = callback_state.as_mut() {
            let userdata = state.as_mut() as *mut VtCallbackState as *mut c_void;
            let rc = unsafe {
                ghostty_terminal_set(terminal, GhosttyTerminalOption::Userdata, userdata)
            };
            if rc != 0 {
                unsafe { ghostty_terminal_free(terminal) };
                anyhow::bail!("ghostty_terminal_set(USERDATA) failed: rc={rc}");
            }

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
            ];

            for (option, callback, label) in callback_options {
                let rc = unsafe { ghostty_terminal_set(terminal, option, callback) };
                if rc != 0 {
                    unsafe { ghostty_terminal_free(terminal) };
                    anyhow::bail!("ghostty_terminal_set({label}) failed: rc={rc}");
                }
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

        if let Some(theme) = theme {
            unsafe { apply_theme_to_terminal(terminal, theme) };
        }

        Ok(Self {
            inner: Arc::new(Mutex::new(VtInner {
                terminal,
                render_state,
                row_iter,
                row_cells,
                callback_state,
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

    /// Feed bytes from the PTY into the parser. Non-reentrant per
    /// upstream: do not call from inside a registered callback.
    pub fn feed(&self, bytes: &[u8]) {
        let mut inner = self.inner.lock();
        // SAFETY: terminal valid; bytes live for the call.
        unsafe { ghostty_terminal_vt_write(inner.terminal, bytes.as_ptr(), bytes.len()) };
        inner.generation = inner.generation.wrapping_add(1);
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
        if let Some(state) = inner.callback_state.as_ref() {
            state.cols.store(cols, Ordering::Release);
            state.rows.store(rows, Ordering::Release);
            state
                .cell_width
                .store(cell_width_px.max(1), Ordering::Release);
            state
                .cell_height
                .store(cell_height_px.max(1), Ordering::Release);
        }
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

        let clone_started = perf_trace_enabled().then(Instant::now);
        let cells = inner.scratch.clone();
        let clone_elapsed_ms =
            clone_started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        let snapshot = ScreenSnapshot {
            cols,
            rows,
            cells,
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
        if let Some(state) = inner.callback_state.as_ref() {
            state.dark_mode.store(dark, Ordering::Release);
        }
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

    /// Current working directory reported by shell integration (OSC 7).
    ///
    /// Returns `None` when the shell has not reported a cwd yet. Callers
    /// should keep their launch cwd as a fallback for plain shells.
    pub fn current_dir(&self) -> Option<String> {
        let inner = self.inner.lock();
        if inner.terminal.is_null() {
            return None;
        }
        let mut pwd = GhosttyString::default();
        let rc = unsafe {
            ghostty_terminal_get(
                inner.terminal,
                GhosttyTerminalData::Pwd,
                &mut pwd as *mut _ as *mut c_void,
            )
        };
        if rc != 0 || pwd.ptr.is_null() || pwd.len == 0 {
            return None;
        }

        let bytes = unsafe { std::slice::from_raw_parts(pwd.ptr, pwd.len) };
        let pwd = std::str::from_utf8(bytes).ok()?;
        if pwd.is_empty() {
            return None;
        }
        if pwd.starts_with("file://") {
            parse_osc7_cwd(pwd)
        } else {
            Some(pwd.to_string())
        }
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
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    (state.write_pty)(bytes);
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
        dirty_rows: Vec::new(),
        cursor: Cursor::default(),
        alternate_screen: false,
        scrollbar: None,
        title: None,
        generation,
    }
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
            // Free in reverse-creation order: cells, iter, state, terminal.
            // SAFETY: unique owner via Arc::get_mut.
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
            types["GhosttyTerminalData"]["values"]["MODE"].as_i64(),
            Some(GhosttyTerminalData::Mode as i64)
        );
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
    fn vt_screen_reports_osc7_current_dir() {
        let screen = VtScreen::new(80, 24, None).expect("create vt screen");

        assert_eq!(screen.current_dir(), None);
        screen.feed(b"\x1b]7;file:///tmp/con-vt-cwd\x07");

        assert_eq!(screen.current_dir().as_deref(), Some("/tmp/con-vt-cwd"));
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
