//! Linux terminal view backed by con's local Unix PTY + libghostty-vt
//! parser. Phase 4 styled-cell renderer: this view consumes the parsed
//! `ScreenSnapshot` from `con-ghostty` and paints each row as a GPUI
//! `StyledText` element with one `TextRun` per styled span. That keeps
//! prompt colors, ANSI palette, bold/italic/underline, and selection
//! inverse working without bringing the full D3D11 / DirectWrite stack
//! the Windows backend needs.
//!
//! Rendering trade-off: this is a CPU-side per-cell paint path, not a
//! real glyph atlas. It's good enough for shell prompts, vim/less, and
//! basic TUIs while the long-term GPUI-owned grid renderer matures, and
//! it avoids the previous "trim to plain text" downgrade that hid color
//! and layout state. The Windows D3D11 path remains the model for the
//! eventual native renderer.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use con_ghostty::vt::{VtKeyAction, VtKeyEvent, VtKeyModifiers, VtPasteResult, VtPasteSource};
use con_ghostty::{
    ATTR_BOLD, ATTR_INVERSE, ATTR_ITALIC, ATTR_STRIKE, ATTR_UNDERLINE, GhosttyApp,
    GhosttySplitDirection, GhosttyTerminal, KittyImage, KittyPlacement, ScreenSnapshot,
    SurfaceSize, VtCell, VtCursor,
};
use futures::StreamExt;
use futures::channel::mpsc::unbounded;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::ContextMenuExt;
use gpui_component::{ActiveTheme, Sizable as _};
use image::{Frame, RgbaImage};
use smallvec::SmallVec;

use crate::mouse_sequence::MouseButtonSequence;
use crate::terminal_ime::{TerminalImeInputHandler, TerminalImeView};
use crate::terminal_links::{self, TerminalLink};
use crate::terminal_paste::{
    TerminalPastePayload, payload_from_clipboard, payload_from_external_paths, unsafe_paste_preview,
};
use crate::terminal_restore::restored_terminal_output;

const DEFAULT_FONT_SIZE: f32 = 14.0;
const MIN_FONT_SIZE_PX: f32 = 12.0;
const DEFAULT_CELL_WIDTH_RATIO: f32 = 0.62;
const DEFAULT_CELL_HEIGHT_RATIO: f32 = 1.45;
const TERMINAL_PADDING_X_PX: f32 = 12.0;
const TERMINAL_PADDING_Y_PX: f32 = 10.0;
const KITTY_BELOW_BACKGROUND_LIMIT: i32 = i32::MIN / 2;

/// Resolved logical font size used for both the cell-grid estimate
/// (`estimate_surface_size`) and the actual paint (`render`). Both
/// callers used to clamp differently — paint floored at 12 px,
/// estimate didn't — which let a sub-12 px config size the PTY grid
/// to cells smaller than the cells we actually drew, so text
/// overran the estimated column count and lines wrapped unexpectedly
/// on the alternate screen. Centralising here keeps them honest.
fn effective_font_size(configured: f32) -> f32 {
    let base = if configured > 0.0 {
        configured
    } else {
        DEFAULT_FONT_SIZE
    };
    base.max(MIN_FONT_SIZE_PX)
}

fn cell_width_px(font_size_px: f32) -> f32 {
    (font_size_px * DEFAULT_CELL_WIDTH_RATIO).round().max(7.0)
}

fn cell_height_px(font_size_px: f32) -> f32 {
    (font_size_px * DEFAULT_CELL_HEIGHT_RATIO).round().max(14.0)
}

fn physical_cell_size(font_size_px: f32, scale_factor: f32) -> (u32, u32) {
    let scale_factor = scale_factor.max(f32::EPSILON);
    (
        (cell_width_px(font_size_px) * scale_factor)
            .round()
            .max(1.0) as u32,
        (cell_height_px(font_size_px) * scale_factor)
            .round()
            .max(1.0) as u32,
    )
}

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

pub struct GhosttyView {
    app: Arc<GhosttyApp>,
    terminal: Option<Arc<GhosttyTerminal>>,
    focus_handle: FocusHandle,
    initial_cwd: Option<String>,
    restored_screen_text: Option<Vec<String>>,
    initial_command: Option<crate::startup_args::TerminalCommand>,
    initial_font_size: f32,
    initialized: bool,
    process_exit_emitted: bool,
    last_title: Option<String>,
    last_cwd: Option<String>,
    pending_write: Option<Vec<u8>>,
    snapshot: Option<ScreenSnapshot>,
    row_cache: Vec<CachedTerminalRow>,
    row_cache_generation: Option<u64>,
    row_cache_cursor: Option<VtCursor>,
    row_cache_style: Option<RowCacheStyleKey>,
    row_cache_shape: Option<(u16, u16)>,
    kitty_images: HashMap<KittyImageKey, Arc<RenderImage>>,
    failed_kitty_images: HashSet<KittyImageKey>,
    /// Latched after the first PTY snapshot that contained any
    /// printable content. Used to gate the "Waiting for shell
    /// prompt…" placeholder so it disappears the moment bash echoes
    /// its first prompt and never comes back — even when a TUI like
    /// htop / vim / less switches to the alternate screen and
    /// briefly leaves the grid empty before drawing its own UI.
    seen_any_output: bool,
    pane_bounds: Option<Bounds<Pixels>>,
    scale_factor: f32,
    ime_marked_text: Option<String>,
    ime_selected_range: Option<Range<usize>>,
    last_surface_size: Option<SurfaceSize>,
    mouse_down_link: Option<TerminalLink>,
    suppress_link_mouse_up: bool,
    hovered_link: Option<TerminalLink>,
    last_mouse_position: Option<Point<Pixels>>,
    selection: Option<TerminalSelection>,
    drag_anchor: Option<(u16, u64)>,
    /// Whether the most recent right-button press was consumed by the
    /// terminal app (an SGR report emitted). The context-menu builder
    /// suppresses con's menu only when this is true.
    terminal_mouse_right_consumed: Option<bool>,
    terminal_right_mouse_sequence: MouseButtonSequence<bool>,
    keys_awaiting_release: HashMap<String, crate::terminal_keys::TrackedVtKey>,
    pending_unsafe_paste: Option<(String, VtPasteSource)>,
}

pub fn init(cx: &mut App) {
    // Tab is a focus-navigation key in GPUI Root. Bind it inside the
    // terminal context so shells receive completion requests instead of
    // the window moving focus away from the terminal.
    cx.bind_keys([
        KeyBinding::new("tab", ConsumeTab, Some("GhosttyTerminal")),
        KeyBinding::new("shift-tab", ConsumeTabPrev, Some("GhosttyTerminal")),
    ]);
}

impl GhosttyView {
    pub fn new(
        app: Arc<GhosttyApp>,
        cwd: Option<String>,
        restored_screen_text: Option<Vec<String>>,
        command: Option<crate::startup_args::TerminalCommand>,
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminal = Arc::new(GhosttyTerminal::new());
        let (wake_tx, mut wake_rx) = unbounded::<()>();
        let wake_for_pty: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = wake_tx.unbounded_send(());
        });
        terminal.set_wake_callback(Some(wake_for_pty));

        cx.spawn(async move |this, cx| {
            while wake_rx.next().await.is_some() {
                while wake_rx.try_recv().is_ok() {}
                if this
                    .update(cx, |view, cx| {
                        let mut changed = false;
                        if let Some(terminal) = view.terminal.as_ref() {
                            if terminal.take_needs_render() {
                                changed |= view.refresh_snapshot();
                            }
                        }
                        if changed {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        Self {
            app,
            terminal: Some(terminal),
            focus_handle: cx.focus_handle(),
            initial_cwd: cwd,
            restored_screen_text,
            initial_command: command,
            initial_font_size: font_size,
            initialized: false,
            process_exit_emitted: false,
            last_title: None,
            last_cwd: None,
            pending_write: None,
            snapshot: None,
            row_cache: Vec::new(),
            row_cache_generation: None,
            row_cache_cursor: None,
            row_cache_style: None,
            row_cache_shape: None,
            kitty_images: HashMap::new(),
            failed_kitty_images: HashSet::new(),
            seen_any_output: false,
            pane_bounds: None,
            scale_factor: 1.0,
            ime_marked_text: None,
            ime_selected_range: None,
            last_surface_size: None,
            mouse_down_link: None,
            suppress_link_mouse_up: false,
            hovered_link: None,
            last_mouse_position: None,
            selection: None,
            drag_anchor: None,
            terminal_mouse_right_consumed: None,
            terminal_right_mouse_sequence: MouseButtonSequence::default(),
            keys_awaiting_release: HashMap::new(),
            pending_unsafe_paste: None,
        }
    }

    pub fn terminal(&self) -> Option<&Arc<GhosttyTerminal>> {
        self.terminal.as_ref()
    }

    pub fn write_or_queue(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.clear_restored_screen_text();
            self.clear_selection();
        }

        if let Some(terminal) = &self.terminal {
            if self.initialized && terminal.is_attached() {
                terminal.write_to_pty(data);
                return;
            }
        }

        self.pending_write
            .get_or_insert_with(Vec::new)
            .extend_from_slice(data);
    }

    pub fn title(&self) -> Option<String> {
        self.terminal.as_ref().and_then(|terminal| terminal.title())
    }

    pub fn current_dir(&self) -> Option<String> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.current_dir())
            .or_else(|| self.initial_cwd.clone())
    }

    pub fn is_alive(&self) -> bool {
        self.terminal
            .as_ref()
            .is_some_and(|terminal| terminal.is_alive())
    }

    pub fn surface_ready(&self) -> bool {
        self.initialized
    }

    pub fn selection_text(&self) -> Option<String> {
        extract_selection_text(self.snapshot.as_ref()?, self.selection?)
    }

    pub fn release_mouse_selection(&mut self, cx: &mut Context<Self>) {
        let Some(position) = self.last_mouse_position else {
            return;
        };
        if self.finish_selection(position) {
            cx.notify();
        }
    }

    fn clear_selection(&mut self) -> bool {
        let changed = self.selection.take().is_some() || self.drag_anchor.take().is_some();
        changed
    }

    fn copy_current_selection_to_clipboard(&mut self, cx: &mut App) -> bool {
        if self.selection.is_none() {
            return false;
        }

        let Some(selection) = self
            .selection_text()
            .filter(|selection| !selection.is_empty())
        else {
            self.clear_selection();
            return true;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(selection));
        self.clear_selection();
        true
    }

    pub fn shutdown_surface(&mut self, mut window: Option<&mut Window>, cx: &mut App) {
        self.release_tracked_keys();
        self.cancel_pointer_interactions();
        if let Some(terminal) = &self.terminal {
            terminal.request_close();
        }
        self.initialized = false;
        self.process_exit_emitted = false;
        self.last_title = None;
        self.pending_write = None;
        self.snapshot = None;
        self.row_cache.clear();
        self.row_cache_generation = None;
        self.row_cache_cursor = None;
        self.row_cache_style = None;
        self.row_cache_shape = None;
        for image in self.kitty_images.drain().map(|(_, image)| image) {
            cx.drop_image(image, window.as_deref_mut());
        }
        self.failed_kitty_images.clear();
        self.seen_any_output = false;
        self.ime_marked_text = None;
        self.ime_selected_range = None;
        self.last_surface_size = None;
        self.mouse_down_link = None;
        self.suppress_link_mouse_up = false;
        self.hovered_link = None;
        self.last_mouse_position = None;
        self.selection = None;
        self.drag_anchor = None;
        self.terminal_mouse_right_consumed = None;
        self.terminal_right_mouse_sequence = MouseButtonSequence::default();
        self.keys_awaiting_release.clear();
        self.pending_unsafe_paste = None;
    }

    pub fn set_surface_focus_state(&mut self, focused: bool) {
        if !focused {
            self.release_tracked_keys();
            self.cancel_pointer_interactions();
        }
        if let Some(terminal) = &self.terminal {
            terminal.set_focus(focused);
        }
    }

    pub fn ensure_initialized_for_control(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = self.ensure_session(cx);
        if let Some(bounds) = self.pane_bounds {
            let _ = self.sync_surface_size(bounds, window.scale_factor());
        }
    }

    pub fn sync_surface_layout_for_host(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changed = self.ensure_session(cx);
        changed |= self.sync_surface_size(bounds, window.scale_factor());
        if changed {
            cx.notify();
        }
    }

    pub fn set_visible(&self, _visible: bool) {}

    pub fn sync_window_background_blur(&self) {}

    pub fn drain_surface_state(
        &mut self,
        _sync_native_scroll: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = self.ensure_session(cx);

        let Some(terminal) = self.terminal.as_ref().cloned() else {
            return changed;
        };

        if terminal.take_needs_render() {
            changed |= self.refresh_snapshot();
        }

        let title = terminal.title();
        if title != self.last_title {
            self.last_title = title.clone();
            changed = true;
            cx.emit(GhosttyTitleChanged(title));
        }

        let cwd = terminal.current_dir();
        if cwd != self.last_cwd {
            self.last_cwd = cwd.clone();
            changed = true;
            cx.emit(GhosttyCwdChanged(cwd));
        }

        if !terminal.is_alive() && !self.process_exit_emitted {
            self.process_exit_emitted = true;
            changed = true;
            cx.emit(GhosttyProcessExited);
        }

        if changed {
            cx.notify();
        }

        changed
    }

    pub fn pump_deferred_work(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = self.ensure_session(cx);

        if let Some(terminal) = self.terminal.as_ref().cloned() {
            // Only re-snapshot when libghostty-vt actually has new
            // output. The previous code also fell through to
            // `refresh_snapshot()` on every poll tick whenever the
            // shell was alive — that re-ran the full FFI walk
            // 60×/s for nothing, ate measurable CPU on busy panes
            // (htop / vim), and also drowned out the per-PTY-write
            // wake signal we explicitly want to react to.
            if terminal.take_needs_render() {
                changed |= self.refresh_snapshot();
            }

            let title = terminal.title();
            if title != self.last_title {
                self.last_title = title.clone();
                changed = true;
                cx.emit(GhosttyTitleChanged(title));
            }

            let cwd = terminal.current_dir();
            if cwd != self.last_cwd {
                self.last_cwd = cwd.clone();
                changed = true;
                cx.emit(GhosttyCwdChanged(cwd));
            }

            if !terminal.is_alive() && !self.process_exit_emitted {
                self.process_exit_emitted = true;
                changed = true;
                cx.emit(GhosttyProcessExited);
            }
        }

        if changed {
            cx.notify();
        }

        changed
    }

    fn clear_restored_screen_text(&mut self) {
        self.restored_screen_text = None;
    }

    fn ensure_session(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(terminal) = self.terminal.as_ref().cloned() else {
            return false;
        };

        if terminal.is_attached() {
            self.initialized = true;
            return false;
        }

        let mut options = self.app.default_pty_options(self.initial_cwd.as_deref());
        if let Some(command) = self.initial_command.as_ref() {
            options.command_program = Some(command.program.clone());
            options.command_args = Some(command.args.clone());
        }
        options.initial_output = restored_terminal_output(self.restored_screen_text.as_deref());
        match terminal.spawn_with_options(options) {
            Ok(()) => {
                self.initial_command = None;
                self.restored_screen_text = None;
                self.initialized = true;
                self.process_exit_emitted = false;
                self.last_cwd = terminal.current_dir();
                if let Some(pending) = self.pending_write.take() {
                    terminal.write_to_pty(&pending);
                }
                self.last_title = terminal.title();
                let _ = self.refresh_snapshot();
                cx.notify();
                true
            }
            Err(err) => {
                log::error!("failed to start linux shell: {err}");
                false
            }
        }
    }

    fn refresh_snapshot(&mut self) -> bool {
        let Some(terminal) = self.terminal.as_ref().cloned() else {
            return false;
        };
        let Some(snapshot) = terminal.snapshot() else {
            return false;
        };
        // Generation alone is enough: libghostty-vt bumps the screen
        // generation on every parser feed that changed grid state.
        // The previous code also did a `prev.cells == snapshot.cells`
        // deep-compare on every refresh — that was a 50–200 KB Vec
        // compare per frame on busy panes and never short-circuited
        // (callers only invoke this when `take_needs_render()`
        // already returned true), so it was pure cost.
        if self
            .snapshot
            .as_ref()
            .is_some_and(|prev| prev.generation == snapshot.generation)
        {
            return false;
        }
        // Latch once the parser has handed us any visible terminal output.
        // Used to suppress the "Waiting for shell prompt…" placeholder
        // for the lifetime of the PTY session — important for TUIs
        // (htop, vim, less, fzf, …) that switch to the alternate
        // screen and leave the grid empty for ~hundreds of ms before
        // drawing their UI. Without this latch the placeholder would
        // briefly flash over a black backdrop on every alt-screen
        // entry and look like a regression in shell readiness. Image-only
        // applications count too, otherwise the placeholder would shift
        // their Kitty placements down by one synthetic row indefinitely.
        if !self.seen_any_output
            && (snapshot.cells.iter().any(|c| c.codepoint != 0)
                || !snapshot.kitty_placements.is_empty())
        {
            self.seen_any_output = true;
        }
        self.clear_selection();
        self.snapshot = Some(snapshot);
        true
    }

    fn sync_surface_size(&mut self, bounds: Bounds<Pixels>, scale_factor: f32) -> bool {
        self.pane_bounds = Some(bounds);
        self.scale_factor = scale_factor;

        let Some(terminal) = self.terminal.as_ref().cloned() else {
            return false;
        };

        let size = self.estimate_surface_size(bounds, scale_factor);
        if self.last_surface_size == Some(size) {
            return false;
        }

        self.clear_selection();
        if let Err(err) = terminal.resize_surface(size) {
            // Do not cache a resize that never reached the PTY. A later layout
            // or render pass can retry the same dimensions after backpressure
            // on the Flatpak host bridge clears.
            log::debug!("linux pty resize failed: {err}");
            return false;
        }
        self.last_surface_size = Some(size);
        false
    }

    fn estimate_surface_size(&self, bounds: Bounds<Pixels>, scale_factor: f32) -> SurfaceSize {
        let width_px = ((f32::from(bounds.size.width) * scale_factor).ceil() as u32).max(1);
        let height_px = ((f32::from(bounds.size.height) * scale_factor).ceil() as u32).max(1);

        // Until the real grid renderer lands we estimate the PTY grid
        // from the configured mono font size so shells and TUIs do
        // not stay stuck at the initial 80x24 forever. Run through
        // the same `effective_font_size` clamp `render()` uses so
        // the grid we ask the PTY for matches the cell size we
        // actually paint at — picking different floors here would
        // make text overrun the estimated column count and lines
        // wrap unexpectedly on the alternate screen.
        let font_size_px = effective_font_size(self.initial_font_size);
        // GPUI paints in logical pixels and applies the display scale after
        // layout. Scale those exact logical cell metrics for libghostty too;
        // recalculating the ratios from an already-scaled font can round to a
        // different device size (for example 9 px logical became 17 px at 2x),
        // which makes Kitty placements drift away from their text columns.
        let (cell_width, cell_height) = physical_cell_size(font_size_px, scale_factor);
        let columns = (width_px / cell_width.max(1))
            .max(1)
            .min(u32::from(u16::MAX)) as u16;
        let rows = (height_px / cell_height.max(1))
            .max(1)
            .min(u32::from(u16::MAX)) as u16;

        SurfaceSize {
            columns,
            rows,
            width_px,
            height_px,
            cell_width_px: cell_width,
            cell_height_px: cell_height,
        }
    }

    fn cell_from_event_position(&self, pos: Point<Pixels>) -> Option<(u16, u16)> {
        self.cell_from_event_position_impl(pos, false)
    }

    fn clamped_cell_from_event_position(&self, pos: Point<Pixels>) -> Option<(u16, u16)> {
        self.cell_from_event_position_impl(pos, true)
    }

    fn cell_from_event_position_impl(
        &self,
        pos: Point<Pixels>,
        clamp_to_grid: bool,
    ) -> Option<(u16, u16)> {
        let bounds = self.pane_bounds?;
        let snapshot = self.snapshot.as_ref()?;
        let font_size_px = effective_font_size(self.initial_font_size);
        let cell_width_px = cell_width_px(font_size_px);
        let cell_height_px = cell_height_px(font_size_px);

        let mut local_x = f32::from(pos.x) - f32::from(bounds.origin.x) - TERMINAL_PADDING_X_PX;
        let mut local_y = f32::from(pos.y) - f32::from(bounds.origin.y) - TERMINAL_PADDING_Y_PX;
        let grid_width = snapshot.cols as f32 * cell_width_px;
        let grid_height = snapshot.rows as f32 * cell_height_px;
        if clamp_to_grid {
            local_x = local_x.clamp(0.0, (grid_width - f32::EPSILON).max(0.0));
            local_y = local_y.clamp(0.0, (grid_height - f32::EPSILON).max(0.0));
        } else if local_x < 0.0 || local_y < 0.0 || local_x >= grid_width || local_y >= grid_height
        {
            return None;
        }

        let col = (local_x / cell_width_px).floor() as u16;
        let row = (local_y / cell_height_px).floor() as u16;
        if col >= snapshot.cols || row >= snapshot.rows {
            return None;
        }
        Some((col, row))
    }

    fn link_at_position(&self, pos: Point<Pixels>) -> Option<TerminalLink> {
        let (col, row) = self.cell_from_event_position(pos)?;
        let snapshot = self.snapshot.as_ref()?;
        terminal_links::link_at_snapshot(snapshot, col, row)
    }

    fn update_hovered_link(&mut self, modifiers: &Modifiers) -> bool {
        let next = if terminal_links::should_open_link(modifiers) {
            self.last_mouse_position
                .and_then(|position| self.link_at_position(position))
        } else {
            None
        };
        if self.hovered_link == next {
            return false;
        }
        self.hovered_link = next;
        true
    }

    fn clear_hovered_link(&mut self) -> bool {
        let changed = self.hovered_link.take().is_some();
        if self.drag_anchor.is_none() && !self.terminal_right_mouse_sequence.is_active() {
            self.last_mouse_position = None;
        }
        changed
    }

    fn begin_selection(&mut self, pos: Point<Pixels>) -> bool {
        let Some(point) = self.selection_point_from_event_position(pos) else {
            return self.clear_selection();
        };
        self.drag_anchor = Some(point);
        self.selection = Some(TerminalSelection {
            anchor: point,
            extent: point,
        });
        true
    }

    fn update_selection_drag(&mut self, pos: Point<Pixels>) -> bool {
        let Some(anchor) = self.drag_anchor else {
            return false;
        };
        let Some(extent) = self.clamped_selection_point_from_event_position(pos) else {
            return false;
        };
        let next = TerminalSelection { anchor, extent };
        if self.selection == Some(next) {
            return false;
        }
        self.selection = Some(next);
        true
    }

    fn finish_selection(&mut self, pos: Point<Pixels>) -> bool {
        let anchor = self.drag_anchor.take();
        let Some(anchor) = anchor else {
            return false;
        };
        let extent = self
            .clamped_selection_point_from_event_position(pos)
            .unwrap_or(anchor);
        if anchor == extent {
            return self.clear_selection();
        }
        let next = TerminalSelection { anchor, extent };
        if self.selection == Some(next) {
            return false;
        }
        self.selection = Some(next);
        true
    }

    fn finish_right_mouse_sequence(&mut self, pos: Point<Pixels>) -> bool {
        let Some(shift) = self.terminal_right_mouse_sequence.finish() else {
            return false;
        };

        if let Some(terminal) = self.terminal()
            && let Some((col, row)) = self.clamped_cell_from_event_position(pos)
        {
            terminal.mouse_release(2, col, row, shift);
        }
        true
    }

    fn cancel_left_pointer_interactions(&mut self, position: Point<Pixels>) {
        self.mouse_down_link = None;
        self.suppress_link_mouse_up = false;
        self.finish_selection(position);
    }

    fn cancel_pointer_interactions(&mut self) {
        let Some(position) = self.last_mouse_position else {
            self.mouse_down_link = None;
            self.suppress_link_mouse_up = false;
            self.drag_anchor = None;
            self.terminal_right_mouse_sequence.finish();
            return;
        };
        self.cancel_left_pointer_interactions(position);
        self.finish_right_mouse_sequence(position);
    }

    fn selection_point_from_event_position(&self, pos: Point<Pixels>) -> Option<(u16, u64)> {
        self.cell_from_event_position(pos)
            .map(|cell| self.selection_point_from_cell(cell))
    }

    fn clamped_selection_point_from_event_position(
        &self,
        pos: Point<Pixels>,
    ) -> Option<(u16, u64)> {
        self.clamped_cell_from_event_position(pos)
            .map(|cell| self.selection_point_from_cell(cell))
    }

    fn selection_point_from_cell(&self, cell: (u16, u16)) -> (u16, u64) {
        let viewport_offset = self
            .snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.scrollbar)
            .map_or(0, |scrollbar| scrollbar.offset);
        (cell.0, viewport_offset.saturating_add(cell.1 as u64))
    }

    fn render_link_cursor_overlay(
        &self,
        cell_width_px: f32,
        line_height_px: f32,
    ) -> Option<AnyElement> {
        let link = self.hovered_link.as_ref()?;
        let width_cols = link.end_col.saturating_sub(link.start_col).max(1);

        Some(
            div()
                .absolute()
                .left(px(
                    TERMINAL_PADDING_X_PX + link.start_col as f32 * cell_width_px
                ))
                .top(px(TERMINAL_PADDING_Y_PX + link.row as f32 * line_height_px))
                .w(px(width_cols as f32 * cell_width_px))
                .h(px(line_height_px))
                .bg(gpui::transparent_black())
                .cursor_pointer()
                .into_any_element(),
        )
    }

    fn send_vt_key(
        &mut self,
        tracking_key: &str,
        event: &VtKeyEvent<'_>,
    ) -> Result<con_ghostty::vt::VtKeyOutcome, String> {
        let Some(terminal) = self.terminal.as_ref().cloned() else {
            return Ok(con_ghostty::vt::VtKeyOutcome::default());
        };
        let outcome = terminal.send_key(event)?;
        if outcome.output_accepted {
            self.clear_restored_screen_text();
            self.clear_selection();
            if outcome.report_releases
                && event.action != VtKeyAction::Release
                && !self.keys_awaiting_release.contains_key(tracking_key)
            {
                self.keys_awaiting_release.insert(
                    tracking_key.to_owned(),
                    crate::terminal_keys::TrackedVtKey::from_non_release_event(event),
                );
            }
        }
        Ok(outcome)
    }

    fn handle_key_up(&mut self, event: &KeyUpEvent) -> bool {
        let Some(tracked) = self.keys_awaiting_release.remove(&event.keystroke.key) else {
            return false;
        };
        let release =
            tracked.release_with_modifiers(&event.keystroke.key, &event.keystroke.modifiers);
        match self.send_vt_key(&event.keystroke.key, &release) {
            Ok(outcome) => outcome.output_accepted,
            Err(err) => {
                // Preserve the press so focus loss can retry the release if
                // this was a transient PTY write failure.
                self.keys_awaiting_release
                    .insert(event.keystroke.key.clone(), tracked);
                log::debug!("linux terminal key release failed: {err}");
                false
            }
        }
    }

    fn release_tracked_keys(&mut self) {
        let tracked_keys = std::mem::take(&mut self.keys_awaiting_release);
        let Some(terminal) = self.terminal.as_ref().cloned() else {
            return;
        };
        for (key, tracked) in tracked_keys {
            let release = tracked.release(&key);
            if let Err(err) = terminal.send_key(&release) {
                self.keys_awaiting_release.insert(key, tracked);
                log::debug!("linux terminal key release failed: {err}");
            }
        }
    }

    fn send_tab_key(&mut self, shift: bool) -> bool {
        let event = VtKeyEvent {
            key: "tab",
            text: "",
            unshifted_codepoint: None,
            action: if self.keys_awaiting_release.contains_key("tab") {
                VtKeyAction::Repeat
            } else {
                VtKeyAction::Press
            },
            modifiers: VtKeyModifiers {
                shift,
                ..VtKeyModifiers::default()
            },
            consumed_modifiers: VtKeyModifiers::default(),
        };
        match self.send_vt_key("tab", &event) {
            Ok(outcome) => outcome.output_accepted,
            Err(err) => {
                log::debug!("linux terminal key encoding failed: {err}");
                false
            }
        }
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.terminal.is_none() {
            return false;
        }

        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform {
            return false;
        }
        if crate::terminal_shortcuts::key_down_starts_action_binding(
            event,
            window,
            &crate::TogglePaneZoom,
        ) || crate::terminal_shortcuts::key_down_starts_action_binding(
            event,
            window,
            &crate::FocusFiles,
        ) || crate::terminal_shortcuts::key_down_starts_action_binding(
            event,
            window,
            &crate::SearchFiles,
        ) {
            return false;
        }
        // App-level tab selection. Let GPUI dispatch SelectTab1..9
        // instead of forwarding Ctrl+digit to the shell.
        if keystroke.modifiers.control
            && !keystroke.modifiers.shift
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
            && matches!(
                keystroke.key.as_str(),
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
            )
        {
            return false;
        }

        // Plain Ctrl+C copies a terminal selection; without a selection it
        // keeps the shell's interrupt semantics.
        if keystroke.modifiers.control
            && !keystroke.modifiers.shift
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
            && keystroke.key == "c"
            && self.copy_current_selection_to_clipboard(cx)
        {
            return true;
        }

        if keystroke.modifiers.control
            && keystroke.modifiers.shift
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
        {
            match keystroke.key.as_str() {
                "c" => {
                    self.copy_current_selection_to_clipboard(cx);
                    return true;
                }
                "v" => {
                    self.paste_from_clipboard(cx);
                    return true;
                }
                _ => {}
            }
        }

        // XKB compose and IME completion can arrive as a normal keydown
        // while GPUI still owns marked text. Its InputHandler must commit
        // the text and clear that state; encoding here would leave the
        // preedit overlay stale even though the character reached the PTY.
        if self.ime_marked_text.is_some()
            && keystroke
                .key_char
                .as_deref()
                .is_some_and(|text| !text.is_empty())
        {
            return false;
        }

        let Some(vt_event) = crate::terminal_keys::vt_key_down_event(event) else {
            return false;
        };
        match self.send_vt_key(&keystroke.key, &vt_event) {
            Ok(outcome) => outcome.output_accepted,
            Err(err) => {
                log::debug!("linux terminal key encoding failed: {err}");
                false
            }
        }
    }

    fn handle_terminal_paste_payload(
        &mut self,
        payload: TerminalPastePayload,
        source: VtPasteSource,
    ) -> bool {
        // A new paste intent invalidates any confirmation for older text,
        // even when this attempt later turns out to be empty or fails.
        let replaced_confirmation = self.pending_unsafe_paste.take().is_some();
        let Some(terminal) = self.terminal.as_ref().cloned() else {
            return replaced_confirmation;
        };

        match payload {
            TerminalPastePayload::Text(text) if !text.is_empty() => {
                match terminal.paste_text(&text, source, false) {
                    Ok(VtPasteResult::Accepted) => {
                        self.clear_restored_screen_text();
                        self.clear_selection();
                        true
                    }
                    Ok(VtPasteResult::RequiresConfirmation) => {
                        self.pending_unsafe_paste = Some((text, source));
                        true
                    }
                    Ok(VtPasteResult::Empty) => replaced_confirmation,
                    Err(err) => {
                        log::debug!("linux terminal paste failed: {err}");
                        replaced_confirmation
                    }
                }
            }
            TerminalPastePayload::ForwardCtrlV => {
                self.clear_restored_screen_text();
                self.clear_selection();
                terminal.send_text("\x16");
                true
            }
            TerminalPastePayload::Text(_) => replaced_confirmation,
        }
    }

    fn paste_from_clipboard(&mut self, cx: &mut App) -> bool {
        let replaced_confirmation = self.pending_unsafe_paste.take().is_some();
        let Some(payload) = cx
            .read_from_clipboard()
            .and_then(|item| payload_from_clipboard(&item))
        else {
            return replaced_confirmation;
        };
        self.handle_terminal_paste_payload(payload, VtPasteSource::Clipboard)
            || replaced_confirmation
    }

    fn confirm_unsafe_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, source)) = self.pending_unsafe_paste.take() else {
            return;
        };
        let Some(terminal) = self.terminal.as_ref().cloned() else {
            self.pending_unsafe_paste = Some((text, source));
            return;
        };

        match terminal.paste_text(&text, source, true) {
            Ok(VtPasteResult::Accepted) => {
                self.clear_restored_screen_text();
                self.clear_selection();
            }
            Ok(VtPasteResult::Empty) => {}
            Ok(VtPasteResult::RequiresConfirmation) => {
                self.pending_unsafe_paste = Some((text, source));
            }
            Err(err) => {
                log::debug!("linux confirmed terminal paste failed: {err}");
                self.pending_unsafe_paste = Some((text, source));
            }
        }
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn cancel_unsafe_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_unsafe_paste = None;
        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn render_unsafe_paste_confirmation(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let text = self.pending_unsafe_paste.as_ref()?.0.as_str();
        let preview = unsafe_paste_preview(text);
        let theme = cx.theme();

        Some(
            div()
                .absolute()
                .left(px(12.0))
                .right(px(12.0))
                .bottom(px(12.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .occlude()
                        .w_full()
                        .max_w(px(620.0))
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .p(px(10.0))
                        .rounded(px(8.0))
                        .bg(theme
                            .warning
                            .opacity(if theme.is_dark() { 0.18 } else { 0.12 }))
                        .child(
                            div()
                                .text_size(px(12.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.foreground)
                                .child("This paste can run commands. Review it before continuing."),
                        )
                        .child(
                            div()
                                .max_h(px(88.0))
                                .overflow_hidden()
                                .px(px(8.0))
                                .py(px(6.0))
                                .rounded(px(6.0))
                                .bg(theme.foreground.opacity(0.06))
                                .font_family(theme.mono_font_family.clone())
                                .text_size(px(11.0))
                                .line_height(px(15.0))
                                .text_color(theme.foreground.opacity(0.82))
                                .child(preview),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    Button::new("linux-cancel-unsafe-paste")
                                        .label("Cancel")
                                        .small()
                                        .ghost()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cancel_unsafe_paste(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("linux-confirm-unsafe-paste")
                                        .label("Paste")
                                        .small()
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.confirm_unsafe_paste(window, cx);
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn ime_cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let bounds = self.pane_bounds?;
        let snapshot = self.snapshot.as_ref()?;
        let font_size_px = effective_font_size(self.initial_font_size);
        let cell_width = cell_width_px(font_size_px);
        let cell_height = cell_height_px(font_size_px);
        let col = snapshot.cursor.col.min(snapshot.cols.saturating_sub(1)) as f32;
        let row = snapshot.cursor.row.min(snapshot.rows.saturating_sub(1)) as f32;

        Some(Bounds::new(
            point(
                bounds.origin.x + px(TERMINAL_PADDING_X_PX + col * cell_width),
                bounds.origin.y + px(TERMINAL_PADDING_Y_PX + row * cell_height),
            ),
            size(px(cell_width.max(1.0)), px(cell_height.max(1.0))),
        ))
    }

    fn sync_kitty_image_cache(
        &mut self,
        placements: &[KittyPlacement],
        window: &mut Window,
        cx: &mut App,
    ) {
        let active = placements
            .iter()
            .map(|placement| KittyImageKey::from(placement.image.as_ref()))
            .collect::<HashSet<_>>();
        let mut stale = Vec::new();
        self.kitty_images.retain(|key, image| {
            let keep = active.contains(key);
            if !keep {
                stale.push(image.clone());
            }
            keep
        });
        for image in stale {
            cx.drop_image(image, Some(window));
        }
        self.failed_kitty_images.retain(|key| active.contains(key));

        for placement in placements {
            let key = KittyImageKey::from(placement.image.as_ref());
            if self.kitty_images.contains_key(&key) || self.failed_kitty_images.contains(&key) {
                continue;
            }
            if let Some(image) = kitty_image_to_render_image(&placement.image) {
                self.kitty_images.insert(key, image);
            } else {
                log::warn!(
                    "ignoring invalid Kitty image {} generation {} ({}x{}, {} bytes)",
                    key.id,
                    key.generation,
                    placement.image.width,
                    placement.image.height,
                    placement.image.rgba.len()
                );
                self.failed_kitty_images.insert(key);
            }
        }
    }

    fn sync_row_cache(
        &mut self,
        default_fg: Hsla,
        default_bg: Hsla,
        base_font: &Font,
        font_size: Pixels,
        line_height: Pixels,
    ) {
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.row_cache.clear();
            self.row_cache_generation = None;
            self.row_cache_cursor = None;
            self.row_cache_style = None;
            self.row_cache_shape = None;
            return;
        };

        let style = RowCacheStyleKey {
            font_family: base_font.family.clone(),
            default_fg,
            default_bg,
            font_size,
            line_height,
        };
        let shape = (snapshot.cols, snapshot.rows);
        let force_full_rebuild = self.row_cache_style.as_ref() != Some(&style)
            || self.row_cache_shape != Some(shape)
            || self.row_cache.len() != usize::from(snapshot.rows);

        if force_full_rebuild {
            self.row_cache
                .resize_with(usize::from(snapshot.rows), CachedTerminalRow::default);
        }

        let mut rows_to_refresh = if force_full_rebuild {
            rows_needing_refresh(snapshot, self.row_cache_cursor, true)
        } else if self.row_cache_generation != Some(snapshot.generation) {
            // Linux currently paints terminal rows through GPUI `StyledText`
            // elements, so stale rows remain visible if the VT dirty-row set
            // misses rows that became blank during alternate-screen restore.
            // Rebuilding all row elements for a changed snapshot is still
            // bounded by the visible grid and keeps TUI exits correct.
            (0..usize::from(snapshot.rows)).collect()
        } else {
            Vec::new()
        };

        rows_to_refresh.sort_unstable();
        rows_to_refresh.dedup();

        for row_idx in rows_to_refresh {
            let row_start = row_idx * usize::from(snapshot.cols);
            let row_end = row_start + usize::from(snapshot.cols);
            let Some(cells) = snapshot.cells.get(row_start..row_end) else {
                break;
            };
            let cursor_for_row = cursor_col_for_row(snapshot.cursor, row_idx);
            self.row_cache[row_idx] = build_terminal_row(
                cells,
                default_fg,
                default_bg,
                base_font,
                cursor_for_row,
                None,
                default_bg,
            );
        }

        self.row_cache_generation = Some(snapshot.generation);
        self.row_cache_cursor = Some(snapshot.cursor);
        self.row_cache_style = Some(style);
        self.row_cache_shape = Some(shape);
    }
}

impl Focusable for GhosttyView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

type LinuxTerminalInputHandler = TerminalImeInputHandler<GhosttyView>;

impl TerminalImeView for GhosttyView {
    fn ime_marked_text(&self) -> Option<&str> {
        self.ime_marked_text.as_deref()
    }

    fn ime_selected_range(&self) -> Option<Range<usize>> {
        self.ime_selected_range.clone()
    }

    fn set_ime_state(&mut self, marked_text: Option<String>, selected_range: Option<Range<usize>>) {
        self.ime_marked_text = marked_text;
        self.ime_selected_range = selected_range;
    }

    fn clear_ime_state(&mut self) {
        self.ime_marked_text = None;
        self.ime_selected_range = None;
    }

    fn send_ime_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let _ = self.ensure_session(cx);
        if !text.is_empty() {
            self.clear_restored_screen_text();
            self.clear_selection();
        }
        if let Some(terminal) = &self.terminal {
            terminal.send_text(text);
        }
    }

    fn prepare_ime_marked_text(&mut self, marked_text: &str, cx: &mut Context<Self>) {
        let _ = self.ensure_session(cx);
        if !marked_text.is_empty() {
            self.clear_restored_screen_text();
        }
    }

    fn ime_cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        GhosttyView::ime_cursor_bounds(self)
    }
}

impl Render for GhosttyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let unsafe_paste_confirmation = self.render_unsafe_paste_confirmation(cx);
        let kitty_placements = self
            .snapshot
            .as_ref()
            .map(|snapshot| snapshot.kitty_placements.clone())
            .unwrap_or_default();
        self.sync_kitty_image_cache(&kitty_placements, window, cx);

        let theme = cx.theme();
        let focus = self.focus_handle.clone();
        let input_focus = focus.clone();
        let context_focus = focus.clone();
        let menu_focus = focus.clone();
        let entity = cx.entity().downgrade();
        let input_entity = entity.clone();
        let menu_entity = entity.clone();
        let font_size_px = effective_font_size(self.initial_font_size);
        let line_height_px = cell_height_px(font_size_px);
        let cell_width_px = cell_width_px(font_size_px);
        let physical_cell = self
            .last_surface_size
            .map(|size| (size.cell_width_px, size.cell_height_px))
            .unwrap_or_else(|| physical_cell_size(font_size_px, self.scale_factor));
        let mono_font = Font {
            family: theme.mono_font_family.clone(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };

        let status_message = if !self.initialized {
            Some("Launching Linux shell…")
        } else if !self.is_alive() {
            Some("Linux shell exited")
        } else if !self.seen_any_output {
            // Only show the "waiting for prompt" placeholder before
            // bash has echoed *anything* for the first time. Once
            // the latch flips, alt-screen TUIs like htop / vim that
            // briefly clear the grid stay silent instead of
            // flashing this placeholder over their startup gap.
            Some("Waiting for shell prompt…")
        } else {
            None
        };

        let foreground = theme.foreground;
        let status_color = foreground.opacity(0.5);
        let configured_pane_opacity = self.app.background_opacity().clamp(0.0, 1.0);
        let pane_opacity = if self
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.alternate_screen)
            && configured_pane_opacity > f32::EPSILON
        {
            1.0
        } else {
            configured_pane_opacity
        };
        let pane_background = theme.background.opacity(pane_opacity);
        let selection = self.selection;
        let selection_bg = theme.selection.opacity(0.42);
        let mut has_kitty_images = false;
        let mut split_terminal_rows = false;
        for placement in kitty_placements.iter() {
            let key = KittyImageKey::from(placement.image.as_ref());
            let valid = self.kitty_images.contains_key(&key)
                && kitty_placement_geometry(
                    placement,
                    cell_width_px,
                    line_height_px,
                    physical_cell.0,
                    physical_cell.1,
                )
                .is_some();
            has_kitty_images |= valid;
            split_terminal_rows |= valid && placement.z < 0;
        }
        self.sync_row_cache(
            foreground,
            theme.background,
            &mono_font,
            px(font_size_px),
            px(line_height_px),
        );

        let mut rows: Vec<AnyElement> = Vec::with_capacity(
            usize::from(self.snapshot.as_ref().map_or(0, |snapshot| snapshot.rows))
                + if status_message.is_some() { 1 } else { 0 },
        );
        let mut cell_backgrounds = split_terminal_rows.then(Vec::new);
        let mut overlay_backgrounds = split_terminal_rows.then(Vec::new);
        let status_row_offset = usize::from(status_message.is_some());
        if let Some(message) = status_message {
            rows.push(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(font_size_px))
                    .line_height(px(line_height_px))
                    .text_color(status_color)
                    .child(message.to_string())
                    .into_any_element(),
            );
        }

        if let Some(snapshot) = self.snapshot.as_ref() {
            for row_idx in 0..usize::from(snapshot.rows) {
                if let Some(selection_cols) =
                    selection.and_then(|selection| selection.row_range(row_idx, snapshot))
                {
                    let row_start = row_idx * usize::from(snapshot.cols);
                    let row_end = row_start + usize::from(snapshot.cols);
                    if let Some(cells) = snapshot.cells.get(row_start..row_end) {
                        let cursor_for_row = cursor_col_for_row(snapshot.cursor, row_idx);
                        let row = build_terminal_row(
                            cells,
                            foreground,
                            theme.background,
                            &mono_font,
                            cursor_for_row,
                            Some(selection_cols),
                            selection_bg,
                        );
                        if let (Some(backgrounds), Some(overlays)) =
                            (cell_backgrounds.as_mut(), overlay_backgrounds.as_mut())
                        {
                            append_terminal_backgrounds(
                                &row,
                                row_idx + status_row_offset,
                                backgrounds,
                                overlays,
                            );
                            rows.push(render_terminal_foreground_row(
                                &row,
                                px(font_size_px),
                                px(line_height_px),
                            ));
                        } else {
                            rows.push(render_cached_terminal_row(
                                &row,
                                px(font_size_px),
                                px(line_height_px),
                            ));
                        }
                    }
                } else if let Some(row) = self.row_cache.get(row_idx) {
                    if let (Some(backgrounds), Some(overlays)) =
                        (cell_backgrounds.as_mut(), overlay_backgrounds.as_mut())
                    {
                        append_terminal_backgrounds(
                            row,
                            row_idx + status_row_offset,
                            backgrounds,
                            overlays,
                        );
                        rows.push(render_terminal_foreground_row(
                            row,
                            px(font_size_px),
                            px(line_height_px),
                        ));
                    } else {
                        rows.push(render_cached_terminal_row(
                            row,
                            px(font_size_px),
                            px(line_height_px),
                        ));
                    }
                }
            }
        }

        if rows.is_empty() {
            rows.push(
                div()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(font_size_px))
                    .line_height(px(line_height_px))
                    .text_color(status_color)
                    .child("\u{00A0}".to_string())
                    .into_any_element(),
            );
        }

        let terminal_content = if has_kitty_images {
            let row_layer = div()
                .absolute()
                .flex()
                .flex_col()
                .size_full()
                .items_start()
                .justify_start()
                .text_color(foreground)
                .children(rows);
            let content_viewport = if split_terminal_rows {
                div()
                    .absolute()
                    .left(px(TERMINAL_PADDING_X_PX))
                    .right(px(TERMINAL_PADDING_X_PX))
                    .top(px(TERMINAL_PADDING_Y_PX))
                    .bottom(px(TERMINAL_PADDING_Y_PX))
                    .overflow_hidden()
                    .child(render_kitty_image_layer(
                        &kitty_placements,
                        &self.kitty_images,
                        KittyImageLayer::BelowBackground,
                        cell_width_px,
                        line_height_px,
                        physical_cell.0,
                        physical_cell.1,
                        status_row_offset as f32 * line_height_px,
                    ))
                    .child(render_terminal_background_layer(
                        cell_backgrounds.unwrap_or_default(),
                        px(cell_width_px),
                        px(line_height_px),
                    ))
                    .child(render_kitty_image_layer(
                        &kitty_placements,
                        &self.kitty_images,
                        KittyImageLayer::BelowText,
                        cell_width_px,
                        line_height_px,
                        physical_cell.0,
                        physical_cell.1,
                        status_row_offset as f32 * line_height_px,
                    ))
                    .child(render_terminal_background_layer(
                        overlay_backgrounds.unwrap_or_default(),
                        px(cell_width_px),
                        px(line_height_px),
                    ))
                    .child(row_layer)
                    .child(render_kitty_image_layer(
                        &kitty_placements,
                        &self.kitty_images,
                        KittyImageLayer::AboveText,
                        cell_width_px,
                        line_height_px,
                        physical_cell.0,
                        physical_cell.1,
                        status_row_offset as f32 * line_height_px,
                    ))
            } else {
                div()
                    .absolute()
                    .left(px(TERMINAL_PADDING_X_PX))
                    .right(px(TERMINAL_PADDING_X_PX))
                    .top(px(TERMINAL_PADDING_Y_PX))
                    .bottom(px(TERMINAL_PADDING_Y_PX))
                    .overflow_hidden()
                    .child(row_layer)
                    .child(render_kitty_image_layer(
                        &kitty_placements,
                        &self.kitty_images,
                        KittyImageLayer::AboveText,
                        cell_width_px,
                        line_height_px,
                        physical_cell.0,
                        physical_cell.1,
                        status_row_offset as f32 * line_height_px,
                    ))
            };
            div()
                .relative()
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .bg(pane_background)
                .child(content_viewport)
        } else {
            div()
                .flex()
                .flex_col()
                .size_full()
                .min_w_0()
                .min_h_0()
                .overflow_hidden()
                .bg(pane_background)
                .px(px(TERMINAL_PADDING_X_PX))
                .py(px(TERMINAL_PADDING_Y_PX))
                .text_color(foreground)
                .items_start()
                .justify_start()
                .children(rows)
        };
        let mut terminal_children = vec![terminal_content.into_any_element()];
        if let Some(overlay) = self.render_link_cursor_overlay(cell_width_px, line_height_px) {
            terminal_children.push(overlay);
        }
        let terminal_layer = div()
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .children(terminal_children);

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_w_0()
            .min_h_0()
            .font_family(theme.font_family.clone())
            .key_context("GhosttyTerminal")
            .track_focus(&self.focus_handle)
            .id(&self.focus_handle)
            .bg(theme.transparent)
            .on_action(cx.listener(|this, _: &ConsumeTab, window, cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                let _ = this.ensure_session(cx);
                this.send_tab_key(false);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ConsumeTabPrev, window, cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                let _ = this.ensure_session(cx);
                this.send_tab_key(true);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &crate::Copy, _window, cx| {
                if this.copy_current_selection_to_clipboard(cx) {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Paste, _window, cx| {
                let _ = this.ensure_session(cx);
                if this.paste_from_clipboard(cx) {
                    cx.notify();
                }
            }))
            .drag_over::<ExternalPaths>(|style, _, _, _| style)
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                let Some(payload) = payload_from_external_paths(paths) else {
                    return;
                };
                window.focus(&this.focus_handle, cx);
                cx.emit(GhosttyFocusChanged);
                let _ = this.ensure_session(cx);
                if this.handle_terminal_paste_payload(payload, VtPasteSource::Text) {
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                let _ = this.ensure_session(cx);
                if this.handle_key_down(event, window, cx) {
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                if this.handle_key_up(event) {
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_modifiers_changed(cx.listener(
                |this, event: &ModifiersChangedEvent, _window, cx| {
                    if this.update_hovered_link(&event.modifiers) {
                        cx.notify();
                    }
                },
            ))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                if !hovered && this.clear_hovered_link() {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&context_focus, cx);
                    let _ = this.ensure_session(cx);
                    this.last_mouse_position = Some(event.position);
                    this.finish_right_mouse_sequence(event.position);
                    let _ = this.update_hovered_link(&event.modifiers);
                    // SGR button 2 = right; unconsumed when tracking is off.
                    // Record the press shift state so the matching release
                    // isn't rejected if Shift changes between press/release.
                    let shift_at_press = event.modifiers.shift;
                    this.terminal_mouse_right_consumed = if let Some(terminal) = this.terminal() {
                        if let Some((col, row)) = this.cell_from_event_position(event.position) {
                            Some(terminal.mouse_report(2, col, row, shift_at_press))
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if this.terminal_mouse_right_consumed == Some(true) {
                        this.terminal_right_mouse_sequence
                            .press_sent(shift_at_press);
                    }
                    cx.emit(GhosttyFocusChanged);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&focus, cx);
                    let _ = this.ensure_session(cx);
                    this.last_mouse_position = Some(event.position);
                    this.cancel_left_pointer_interactions(event.position);
                    let _ = this.update_hovered_link(&event.modifiers);
                    if terminal_links::should_open_link(&event.modifiers)
                        && let Some(link) = this.link_at_position(event.position)
                    {
                        this.mouse_down_link = Some(link);
                        this.suppress_link_mouse_up = true;
                        window.prevent_default();
                        cx.stop_propagation();
                        cx.emit(GhosttyFocusChanged);
                        cx.notify();
                        return;
                    }
                    this.begin_selection(event.position);
                    cx.emit(GhosttyFocusChanged);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.last_mouse_position = Some(event.position);
                if this.suppress_link_mouse_up {
                    if event.pressed_button == Some(MouseButton::Left) {
                        let mut changed = this.update_hovered_link(&event.modifiers);
                        if let Some(down_link) = this.mouse_down_link.as_ref() {
                            let still_on_same_link =
                                this.link_at_position(event.position).as_ref() == Some(down_link);
                            if !still_on_same_link {
                                this.mouse_down_link = None;
                                changed = true;
                            }
                        }
                        cx.stop_propagation();
                        if changed {
                            cx.notify();
                        }
                        return;
                    }
                    if event.pressed_button.is_none() {
                        let mut changed = this.update_hovered_link(&event.modifiers);
                        changed |= this.mouse_down_link.take().is_some();
                        this.suppress_link_mouse_up = false;
                        cx.stop_propagation();
                        if changed {
                            cx.notify();
                        }
                    }
                }
                let mut changed = this.update_hovered_link(&event.modifiers);
                if event.pressed_button == Some(MouseButton::Left) {
                    changed |= this.update_selection_drag(event.position);
                } else if event.pressed_button.is_none() && this.drag_anchor.is_some() {
                    changed |= this.finish_selection(event.position);
                }
                if event.pressed_button.is_none() && this.terminal_right_mouse_sequence.is_active()
                {
                    changed |= this.finish_right_mouse_sequence(event.position);
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.last_mouse_position = Some(event.position);
                    if this.suppress_link_mouse_up {
                        let down_link = this.mouse_down_link.take();
                        this.suppress_link_mouse_up = false;
                        if let Some(down_link) = down_link
                            && this.link_at_position(event.position).as_ref() == Some(&down_link)
                        {
                            cx.open_url(&down_link.url);
                        }
                        window.prevent_default();
                        cx.stop_propagation();
                        let _ = this.update_hovered_link(&event.modifiers);
                        cx.notify();
                        return;
                    }
                    let mut changed = this.finish_selection(event.position);
                    changed |= this.update_hovered_link(&event.modifiers);
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    if this.drag_anchor.is_none() && !this.suppress_link_mouse_up {
                        return;
                    }
                    this.last_mouse_position = Some(event.position);
                    this.mouse_down_link = None;
                    this.suppress_link_mouse_up = false;
                    let mut changed = this.finish_selection(event.position);
                    changed |= this.update_hovered_link(&event.modifiers);
                    if changed {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.last_mouse_position = Some(event.position);
                    if this.finish_right_mouse_sequence(event.position) {
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    if !this.terminal_right_mouse_sequence.is_active() {
                        return;
                    }
                    this.last_mouse_position = Some(event.position);
                    if this.finish_right_mouse_sequence(event.position) {
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .on_children_prepainted(move |bounds_list: Vec<Bounds<Pixels>>, window, cx| {
                        let Some(bounds) = bounds_list.first().copied() else {
                            return;
                        };
                        let scale = window.scale_factor();
                        if let Some(view) = entity.upgrade() {
                            view.update(cx, |view, cx| {
                                let mut changed = view.ensure_session(cx);
                                changed |= view.sync_surface_size(bounds, scale);
                                if changed {
                                    cx.notify();
                                }
                            });
                        }
                    })
                    .child(terminal_layer)
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |_, _, window, cx| {
                                window.handle_input(
                                    &input_focus,
                                    LinuxTerminalInputHandler::new(input_entity.clone()),
                                    cx,
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    )
                    .children(unsafe_paste_confirmation),
            )
            .context_menu(move |menu, window, cx| {
                // Empty PopupMenu renders nothing; suppress con's menu only
                // when the terminal app consumed the right-button press.
                let right_consumed = menu_entity.upgrade().is_some_and(|view| {
                    view.read(cx).terminal_mouse_right_consumed.unwrap_or(false)
                });
                if right_consumed {
                    return menu;
                }
                crate::terminal_context_menu::terminal_context_menu(
                    menu.action_context(menu_focus.clone()),
                    window,
                    cx,
                )
            })
    }
}

impl Drop for GhosttyView {
    fn drop(&mut self) {
        self.release_tracked_keys();
        self.cancel_pointer_interactions();
        if let Some(terminal) = &self.terminal {
            terminal.request_close();
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct KittyImageKey {
    id: u32,
    generation: u64,
}

impl From<&KittyImage> for KittyImageKey {
    fn from(image: &KittyImage) -> Self {
        Self {
            id: image.id,
            generation: image.generation,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KittyImageLayer {
    BelowBackground,
    BelowText,
    AboveText,
}

impl KittyImageLayer {
    fn contains(self, z: i32) -> bool {
        match self {
            Self::BelowBackground => z < KITTY_BELOW_BACKGROUND_LIMIT,
            Self::BelowText => (KITTY_BELOW_BACKGROUND_LIMIT..0).contains(&z),
            Self::AboveText => z >= 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct KittyPlacementGeometry {
    left_px: f32,
    top_px: f32,
    width_px: f32,
    height_px: f32,
    image_left_px: f32,
    image_top_px: f32,
    image_width_px: f32,
    image_height_px: f32,
}

fn kitty_placement_geometry(
    placement: &KittyPlacement,
    logical_cell_width_px: f32,
    logical_cell_height_px: f32,
    physical_cell_width_px: u32,
    physical_cell_height_px: u32,
) -> Option<KittyPlacementGeometry> {
    let image = &placement.image;
    let source_right = placement.source_x.checked_add(placement.source_width)?;
    let source_bottom = placement.source_y.checked_add(placement.source_height)?;
    if placement.pixel_width == 0
        || placement.pixel_height == 0
        || placement.source_width == 0
        || placement.source_height == 0
        || image.width == 0
        || image.height == 0
        || source_right > image.width
        || source_bottom > image.height
        || !logical_cell_width_px.is_finite()
        || !logical_cell_height_px.is_finite()
        || logical_cell_width_px <= 0.0
        || logical_cell_height_px <= 0.0
        || physical_cell_width_px == 0
        || physical_cell_height_px == 0
    {
        return None;
    }

    // libghostty positions placements in the integer physical cell geometry
    // supplied by `resize_surface`. Derive each axis from that exact quantized
    // size instead of dividing by the fractional display scale, which drifts
    // away from GPUI's logical text grid after several columns.
    let device_to_logical_x = logical_cell_width_px as f64 / physical_cell_width_px as f64;
    let device_to_logical_y = logical_cell_height_px as f64 / physical_cell_height_px as f64;
    let width_px = placement.pixel_width as f64 * device_to_logical_x;
    let height_px = placement.pixel_height as f64 * device_to_logical_y;
    let source_scale_x = width_px / placement.source_width as f64;
    let source_scale_y = height_px / placement.source_height as f64;
    let values = KittyPlacementGeometry {
        left_px: (placement.viewport_col as f64 * logical_cell_width_px as f64
            + placement.cell_x_offset as f64 * device_to_logical_x) as f32,
        top_px: (placement.viewport_row as f64 * logical_cell_height_px as f64
            + placement.cell_y_offset as f64 * device_to_logical_y) as f32,
        width_px: width_px as f32,
        height_px: height_px as f32,
        image_left_px: (-(placement.source_x as f64) * source_scale_x) as f32,
        image_top_px: (-(placement.source_y as f64) * source_scale_y) as f32,
        image_width_px: (image.width as f64 * source_scale_x) as f32,
        image_height_px: (image.height as f64 * source_scale_y) as f32,
    };
    if [
        values.left_px,
        values.top_px,
        values.width_px,
        values.height_px,
        values.image_left_px,
        values.image_top_px,
        values.image_width_px,
        values.image_height_px,
    ]
    .iter()
    .all(|value| value.is_finite())
    {
        Some(values)
    } else {
        None
    }
}

fn kitty_image_to_render_image(image: &KittyImage) -> Option<Arc<RenderImage>> {
    let expected_len = (image.width as usize)
        .checked_mul(image.height as usize)?
        .checked_mul(4)?;
    if image.width == 0 || image.height == 0 || image.rgba.len() != expected_len {
        return None;
    }

    // GPUI's `RenderImage` byte contract is BGRA even though the image crate
    // buffer type is named `RgbaImage`. Keep the shared VT snapshot in its
    // renderer-neutral RGBA form and swizzle only once per image generation.
    let mut bgra = image.rgba.to_vec();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let buffer = RgbaImage::from_raw(image.width, image.height, bgra)?;
    let frame = Frame::new(buffer);
    let frames: SmallVec<[Frame; 1]> = SmallVec::from_buf([frame]);
    Some(Arc::new(RenderImage::new(frames)))
}

fn render_kitty_image_layer(
    placements: &[KittyPlacement],
    images: &HashMap<KittyImageKey, Arc<RenderImage>>,
    layer: KittyImageLayer,
    logical_cell_width_px: f32,
    logical_cell_height_px: f32,
    physical_cell_width_px: u32,
    physical_cell_height_px: u32,
    top_offset_px: f32,
) -> AnyElement {
    let paint_records = placements
        .iter()
        .filter_map(|placement| {
            if !layer.contains(placement.z) {
                return None;
            }
            let key = KittyImageKey::from(placement.image.as_ref());
            let image = images.get(&key)?.clone();
            let geometry = kitty_placement_geometry(
                placement,
                logical_cell_width_px,
                logical_cell_height_px,
                physical_cell_width_px,
                physical_cell_height_px,
            )?;
            Some((image, geometry))
        })
        .collect::<Vec<_>>();

    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for (image, geometry) in paint_records {
                let crop_bounds = Bounds::new(
                    point(
                        bounds.origin.x + px(geometry.left_px),
                        bounds.origin.y + px(geometry.top_px + top_offset_px),
                    ),
                    size(px(geometry.width_px), px(geometry.height_px)),
                );
                if !crop_bounds.intersects(&bounds) {
                    continue;
                }

                let image_bounds = Bounds::new(
                    point(
                        crop_bounds.origin.x + px(geometry.image_left_px),
                        crop_bounds.origin.y + px(geometry.image_top_px),
                    ),
                    size(px(geometry.image_width_px), px(geometry.image_height_px)),
                );
                window.with_content_mask(
                    Some(ContentMask {
                        bounds: crop_bounds.intersect(&bounds),
                    }),
                    |window| {
                        if let Err(err) =
                            window.paint_image(image_bounds, Default::default(), image, 0, false)
                        {
                            log::debug!("failed to paint Kitty image placement: {err:#}");
                        }
                    },
                );
            }
        },
    )
    .absolute()
    .size_full()
    .into_any_element()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSelection {
    anchor: (u16, u64),
    extent: (u16, u64),
}

impl TerminalSelection {
    fn contains(self, col: u16, row: u16, snapshot: &ScreenSnapshot) -> bool {
        let cols = snapshot.cols;
        let viewport_offset = snapshot.scrollbar.map_or(0, |scrollbar| scrollbar.offset);
        let to_linear = |point: (u16, u64)| point.1 as u128 * cols as u128 + point.0 as u128;
        let anchor = to_linear(self.anchor);
        let extent = to_linear(self.extent);
        let (start, end) = if anchor <= extent {
            (anchor, extent)
        } else {
            (extent, anchor)
        };
        let current = to_linear((col, viewport_offset.saturating_add(row as u64)));
        current >= start && current <= end
    }

    fn row_range(self, row: usize, snapshot: &ScreenSnapshot) -> Option<(usize, usize)> {
        let cols = snapshot.cols;
        if cols == 0 || row > u16::MAX as usize {
            return None;
        }

        let row = row as u16;
        let mut start = None;
        let mut end = None;
        for col in 0..cols {
            if self.contains(col, row, snapshot) {
                start.get_or_insert(usize::from(col));
                end = Some(usize::from(col));
            }
        }

        start.zip(end)
    }
}

fn extract_selection_text(
    snapshot: &ScreenSnapshot,
    selection: TerminalSelection,
) -> Option<String> {
    if snapshot.cols == 0 || snapshot.rows == 0 || snapshot.cells.is_empty() {
        return None;
    }

    let cols = usize::from(snapshot.cols);
    let mut output = String::new();
    for row in 0..snapshot.rows {
        let Some((start, end)) = selection.row_range(usize::from(row), snapshot) else {
            continue;
        };

        let row_start = usize::from(row) * cols;
        let Some(cells) = snapshot.cells.get(row_start + start..=row_start + end) else {
            continue;
        };

        let mut row_text = String::with_capacity(cells.len());
        let mut last_non_blank = None;
        for cell in cells {
            let ch = match cell.codepoint {
                0 => ' ',
                cp => char::from_u32(cp).unwrap_or('\u{FFFD}'),
            };
            if ch != ' ' {
                last_non_blank = Some(row_text.len() + ch.len_utf8());
            }
            row_text.push(ch);
        }

        if let Some(end_byte) = last_non_blank {
            output.push_str(&row_text[..end_byte]);
        }
        output.push('\n');
    }

    if output.ends_with('\n') {
        output.pop();
    }
    if output.is_empty() {
        None
    } else {
        Some(output)
    }
}

#[derive(Clone, Default)]
struct CachedTerminalRow {
    text: SharedString,
    runs: Vec<TextRun>,
    backgrounds: Vec<TerminalBackgroundRun>,
}

#[derive(Clone, Copy)]
struct TerminalBackgroundRun {
    start_col: usize,
    len: usize,
    color: Hsla,
    overlay: bool,
}

#[derive(Clone, Copy)]
struct TerminalBackgroundQuad {
    row: usize,
    start_col: usize,
    len: usize,
    color: Hsla,
}

#[derive(Clone, PartialEq)]
struct RowCacheStyleKey {
    font_family: SharedString,
    default_fg: Hsla,
    default_bg: Hsla,
    font_size: Pixels,
    line_height: Pixels,
}

fn cursor_col_for_row(cursor: VtCursor, row_idx: usize) -> Option<usize> {
    if cursor.visible && usize::from(cursor.row) == row_idx {
        Some(usize::from(cursor.col))
    } else {
        None
    }
}

fn rows_needing_refresh(
    snapshot: &ScreenSnapshot,
    previous_cursor: Option<VtCursor>,
    force_full_rebuild: bool,
) -> Vec<usize> {
    if force_full_rebuild {
        return (0..usize::from(snapshot.rows)).collect();
    }

    let mut rows = snapshot
        .dirty_rows
        .iter()
        .copied()
        .filter(|row| *row < snapshot.rows)
        .map(usize::from)
        .collect::<Vec<_>>();

    if let Some(previous) = previous_cursor {
        if previous.visible && previous.row < snapshot.rows {
            rows.push(usize::from(previous.row));
        }
    }
    if snapshot.cursor.visible && snapshot.cursor.row < snapshot.rows {
        rows.push(usize::from(snapshot.cursor.row));
    }

    rows
}

fn render_cached_terminal_row(
    row: &CachedTerminalRow,
    font_size: Pixels,
    line_height: Pixels,
) -> AnyElement {
    div()
        .w_full()
        .h(line_height)
        .min_h(line_height)
        .overflow_hidden()
        // Terminal rows must never wrap — a sequence like "12345" that
        // doesn't fit in the remaining width should be clipped, not
        // reflowed onto the next line. Without this, GPUI's default
        // word-wrap breaks continuous runs at word boundaries and pushes
        // them down, making TUI layouts look garbled in narrow panes.
        .whitespace_nowrap()
        .text_size(font_size)
        .line_height(line_height)
        .child(StyledText::new(row.text.clone()).with_runs(row.runs.clone()))
        .into_any_element()
}

fn append_terminal_backgrounds(
    row: &CachedTerminalRow,
    display_row: usize,
    backgrounds: &mut Vec<TerminalBackgroundQuad>,
    overlays: &mut Vec<TerminalBackgroundQuad>,
) {
    for run in &row.backgrounds {
        let quad = TerminalBackgroundQuad {
            row: display_row,
            start_col: run.start_col,
            len: run.len,
            color: run.color,
        };
        if run.overlay {
            overlays.push(quad);
        } else {
            backgrounds.push(quad);
        }
    }
}

fn render_terminal_background_layer(
    backgrounds: Vec<TerminalBackgroundQuad>,
    cell_width: Pixels,
    line_height: Pixels,
) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            for background in backgrounds {
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.origin.x + cell_width * background.start_col as f32,
                            bounds.origin.y + line_height * background.row as f32,
                        ),
                        size(cell_width * background.len as f32, line_height),
                    ),
                    background.color,
                ));
            }
        },
    )
    .absolute()
    .size_full()
    .into_any_element()
}

fn render_terminal_foreground_row(
    row: &CachedTerminalRow,
    font_size: Pixels,
    line_height: Pixels,
) -> AnyElement {
    let mut runs = row.runs.clone();
    for run in &mut runs {
        run.background_color = None;
    }

    div()
        .w_full()
        .h(line_height)
        .min_h(line_height)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(font_size)
        .line_height(line_height)
        .child(StyledText::new(row.text.clone()).with_runs(runs))
        .into_any_element()
}

/// Build a single GPUI row element from a slice of `VtCell`s. We
/// collapse runs of cells that share `(fg, bg, attrs)` into one
/// `TextRun` so each row is a single `StyledText` element. That keeps
/// allocations bounded by the number of *style changes*, not the cell
/// count, while still preserving every SGR transition.
fn build_terminal_row(
    cells: &[VtCell],
    default_fg: Hsla,
    default_bg: Hsla,
    base_font: &Font,
    cursor_col: Option<usize>,
    selection_cols: Option<(usize, usize)>,
    selection_bg: Hsla,
) -> CachedTerminalRow {
    // First pass: find the last column we have to keep. A column
    // matters if it has a real glyph, OR if it carries a non-default
    // background / underline / strikethrough / inverse style, OR if
    // it sits under the cursor. Trailing default-styled blanks past
    // that column are dropped so we don't emit hundreds of empty
    // cells per row, but trailing *styled* blanks (status bars,
    // selection highlights, full-width fills) survive — those carry
    // visual information in their background color and dropping them
    // would collapse the line paint width.
    let last_meaningful_col = cells
        .iter()
        .enumerate()
        .rposition(|(col_idx, cell)| {
            let glyph_present =
                cell.codepoint != 0 && char::from_u32(cell.codepoint).is_some_and(|ch| ch != ' ');
            let styled_blank = (cell.bg & 0xFF) != 0
                || (cell.attrs & (ATTR_INVERSE | ATTR_UNDERLINE | ATTR_STRIKE)) != 0;
            let cursor_here = cursor_col == Some(col_idx);
            let selected_here =
                selection_cols.is_some_and(|(start, end)| col_idx >= start && col_idx <= end);
            glyph_present || styled_blank || cursor_here || selected_here
        })
        // `rposition` already returns the index relative to `cells`,
        // not `iter().enumerate()`'s output. Map it back to a slice
        // length via +1.
        .map(|idx| idx + 1)
        .unwrap_or(0);

    let kept = &cells[..last_meaningful_col];

    let mut text = String::with_capacity(kept.len());
    let mut runs: Vec<TextRun> = Vec::new();
    let mut backgrounds = Vec::new();
    let mut last_signature: Option<(u32, u32, u8, bool, bool)> = None;
    let mut active_run_len: usize = 0;
    let mut active_style: Option<RowStyle> = None;
    let mut active_background: Option<(usize, Hsla, bool)> = None;

    fn flush_run(
        runs: &mut Vec<TextRun>,
        active_style: &mut Option<RowStyle>,
        active_run_len: &mut usize,
    ) {
        if *active_run_len == 0 || active_style.is_none() {
            return;
        }
        let style = active_style.take().expect("active style");
        runs.push(TextRun {
            len: *active_run_len,
            font: style.font,
            color: style.fg,
            background_color: style.bg,
            underline: style.underline,
            strikethrough: style.strikethrough,
        });
        *active_run_len = 0;
    }

    for (col_idx, cell) in kept.iter().enumerate() {
        let is_cursor = cursor_col == Some(col_idx);
        let is_selected =
            selection_cols.is_some_and(|(start, end)| col_idx >= start && col_idx <= end);
        let signature = (cell.fg, cell.bg, cell.attrs, is_cursor, is_selected);
        let style = RowStyle::from_cell(
            cell,
            default_fg,
            default_bg,
            base_font,
            is_cursor,
            is_selected,
            selection_bg,
        );

        let background = style.bg.map(|color| (color, is_cursor || is_selected));
        if active_background.map(|(_, color, overlay)| (color, overlay)) != background {
            if let Some((start_col, color, overlay)) = active_background.take() {
                backgrounds.push(TerminalBackgroundRun {
                    start_col,
                    len: col_idx - start_col,
                    color,
                    overlay,
                });
            }
            active_background = background.map(|(color, overlay)| (col_idx, color, overlay));
        }

        let glyph: char = match cell.codepoint {
            0 => ' ',
            cp => char::from_u32(cp).unwrap_or('\u{FFFD}'),
        };

        if Some(signature) != last_signature {
            flush_run(&mut runs, &mut active_style, &mut active_run_len);
            active_style = Some(style);
            last_signature = Some(signature);
        }

        text.push(glyph);
        active_run_len += glyph.len_utf8();
    }

    flush_run(&mut runs, &mut active_style, &mut active_run_len);
    if let Some((start_col, color, overlay)) = active_background {
        backgrounds.push(TerminalBackgroundRun {
            start_col,
            len: kept.len() - start_col,
            color,
            overlay,
        });
    }

    let text = if text.is_empty() {
        let mut fallback = String::with_capacity(1);
        fallback.push('\u{00A0}');
        runs.push(TextRun {
            len: '\u{00A0}'.len_utf8(),
            font: base_font.clone(),
            color: default_fg,
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        fallback
    } else {
        text
    };

    CachedTerminalRow {
        text: text.into(),
        runs,
        backgrounds,
    }
}

/// Resolved per-cell style ready to emit as a `TextRun`.
struct RowStyle {
    font: Font,
    fg: Hsla,
    bg: Option<Hsla>,
    underline: Option<UnderlineStyle>,
    strikethrough: Option<StrikethroughStyle>,
}

impl RowStyle {
    fn from_cell(
        cell: &VtCell,
        default_fg: Hsla,
        default_bg: Hsla,
        base_font: &Font,
        is_cursor: bool,
        is_selected: bool,
        selection_bg: Hsla,
    ) -> Self {
        let mut font = base_font.clone();
        if cell.attrs & ATTR_BOLD != 0 {
            font.weight = FontWeight::BOLD;
        }
        if cell.attrs & ATTR_ITALIC != 0 {
            font.style = FontStyle::Italic;
        }

        let mut fg = vt_color_to_hsla(cell.fg).unwrap_or(default_fg);
        let mut bg = vt_color_to_hsla(cell.bg);

        if cell.attrs & ATTR_INVERSE != 0 {
            let resolved_bg = bg.unwrap_or(default_bg);
            bg = Some(fg);
            fg = resolved_bg;
        }

        if is_selected {
            bg = Some(selection_bg);
        }

        if is_cursor {
            // Block cursor (focused): swap the *currently resolved*
            // fg / bg so the glyph under the cursor stays legible,
            // like xterm and Ghostty's own default. Crucially we
            // operate on `fg` / `bg` (post `ATTR_INVERSE`) and *not*
            // on the raw `cell.fg` / `cell.bg`, so an inverse cell
            // under the cursor de-inverts to draw the block over
            // the already-inverted content rather than collapsing
            // into a single color and turning the glyph invisible
            // (selected rows in htop, vim status lines, less search
            // highlights all hit this exact case).
            let cursor_bg = fg;
            let cursor_fg = bg.unwrap_or(default_bg);
            fg = cursor_fg;
            bg = Some(cursor_bg);
        }

        let underline = if cell.attrs & ATTR_UNDERLINE != 0 {
            Some(UnderlineStyle {
                color: Some(fg),
                thickness: px(1.0),
                wavy: false,
            })
        } else {
            None
        };

        let strikethrough = if cell.attrs & ATTR_STRIKE != 0 {
            Some(StrikethroughStyle {
                color: Some(fg),
                thickness: px(1.0),
            })
        } else {
            None
        };

        Self {
            font,
            fg,
            bg,
            underline,
            strikethrough,
        }
    }
}

/// Decode the VT cell color (0xRRGGBBAA — alpha=0 means "default"
/// per `con-ghostty/src/vt.rs::read_cell`) into a GPUI `Hsla`.
fn vt_color_to_hsla(packed: u32) -> Option<Hsla> {
    let a = (packed & 0xFF) as u8;
    if a == 0 {
        return None;
    }
    let r = ((packed >> 24) & 0xFF) as f32 / 255.0;
    let g = ((packed >> 16) & 0xFF) as f32 / 255.0;
    let b = ((packed >> 8) & 0xFF) as f32 / 255.0;
    let a = a as f32 / 255.0;
    Some(Rgba { r, g, b, a }.into())
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_FONT_SIZE, KITTY_BELOW_BACKGROUND_LIMIT, KittyImageLayer, MIN_FONT_SIZE_PX,
        TerminalSelection, build_terminal_row, cell_height_px, cell_width_px, effective_font_size,
        extract_selection_text, kitty_image_to_render_image, kitty_placement_geometry,
        physical_cell_size, rows_needing_refresh, vt_color_to_hsla,
    };
    use con_ghostty::{
        ATTR_BOLD, ATTR_INVERSE, ATTR_UNDERLINE, KittyImage, KittyPlacement, ScreenSnapshot,
        VtCell, VtCursor,
    };
    use gpui::{Font, FontFeatures, FontStyle, FontWeight, Hsla, Rgba};
    use std::sync::Arc;

    fn base_font() -> Font {
        Font {
            family: "monospace".into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    fn fg() -> Hsla {
        Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        }
        .into()
    }

    fn bg() -> Hsla {
        Rgba {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        }
        .into()
    }

    fn make_cell(ch: char, attrs: u8, fg: u32, bg: u32) -> VtCell {
        VtCell {
            codepoint: ch as u32,
            fg,
            bg,
            attrs,
            _pad: [0; 3],
        }
    }

    #[test]
    fn vt_color_zero_alpha_means_default() {
        assert_eq!(vt_color_to_hsla(0x000000_00), None);
        assert!(vt_color_to_hsla(0x112233_FF).is_some());
    }

    #[test]
    fn cell_metrics_match_font_size_clamping() {
        assert_eq!(effective_font_size(0.0), DEFAULT_FONT_SIZE);
        assert_eq!(effective_font_size(8.0), MIN_FONT_SIZE_PX);
        assert_eq!(cell_width_px(14.0), 9.0);
        assert_eq!(cell_height_px(14.0), 20.0);
        assert_eq!(physical_cell_size(14.0, 2.0), (18, 40));
    }

    #[test]
    fn kitty_placement_maps_device_pixels_and_source_crop() {
        let placement = KittyPlacement {
            image: Arc::new(KittyImage {
                id: 1,
                generation: 2,
                width: 4,
                height: 2,
                rgba: vec![0; 4 * 2 * 4].into(),
            }),
            placement_id: 3,
            z: 0,
            viewport_col: -1,
            viewport_row: 2,
            cell_x_offset: 4,
            cell_y_offset: 6,
            pixel_width: 20,
            pixel_height: 10,
            source_x: 1,
            source_y: 0,
            source_width: 2,
            source_height: 1,
        };

        let geometry = kitty_placement_geometry(&placement, 10.0, 20.0, 20, 40).unwrap();
        assert_eq!(geometry.left_px, -8.0);
        assert_eq!(geometry.top_px, 43.0);
        assert_eq!(geometry.width_px, 10.0);
        assert_eq!(geometry.height_px, 5.0);
        assert_eq!(geometry.image_left_px, -5.0);
        assert_eq!(geometry.image_top_px, 0.0);
        assert_eq!(geometry.image_width_px, 20.0);
        assert_eq!(geometry.image_height_px, 10.0);
    }

    #[test]
    fn kitty_placement_stays_aligned_after_fractional_cell_rounding() {
        let placement = KittyPlacement {
            image: Arc::new(KittyImage {
                id: 1,
                generation: 2,
                width: 140,
                height: 30,
                rgba: vec![0; 140 * 30 * 4].into(),
            }),
            placement_id: 3,
            z: 0,
            viewport_col: 40,
            viewport_row: 2,
            cell_x_offset: 14,
            cell_y_offset: 0,
            pixel_width: 140,
            pixel_height: 30,
            source_x: 0,
            source_y: 0,
            source_width: 140,
            source_height: 30,
        };

        // A 9px logical cell rounds to 14 physical pixels at 1.5x. The
        // placement must advance exactly one logical cell for every 14 device
        // pixels rather than using 1 / 1.5 and accumulating column drift.
        let geometry = kitty_placement_geometry(&placement, 9.0, 20.0, 14, 30).unwrap();
        assert_eq!(geometry.left_px, 369.0);
        assert_eq!(geometry.top_px, 40.0);
        assert_eq!(geometry.width_px, 90.0);
        assert_eq!(geometry.height_px, 20.0);
    }

    #[test]
    fn kitty_layers_match_protocol_z_boundaries() {
        assert!(KittyImageLayer::BelowBackground.contains(i32::MIN));
        assert!(!KittyImageLayer::BelowBackground.contains(KITTY_BELOW_BACKGROUND_LIMIT));
        assert!(KittyImageLayer::BelowText.contains(KITTY_BELOW_BACKGROUND_LIMIT));
        assert!(KittyImageLayer::BelowText.contains(-1));
        assert!(!KittyImageLayer::BelowText.contains(0));
        assert!(KittyImageLayer::AboveText.contains(0));
    }

    #[test]
    fn kitty_rgba_is_swizzled_for_gpui_render_images() {
        let image = KittyImage {
            id: 1,
            generation: 1,
            width: 1,
            height: 1,
            rgba: vec![0x11, 0x22, 0x33, 0x44].into(),
        };
        let render_image = kitty_image_to_render_image(&image).unwrap();
        assert_eq!(
            render_image.as_bytes(0),
            Some(&[0x33, 0x22, 0x11, 0x44][..])
        );
    }

    #[test]
    fn renders_row_without_panicking() {
        let cells = [
            make_cell('h', 0, 0, 0),
            make_cell('i', ATTR_BOLD | ATTR_UNDERLINE, 0xFF0000FF, 0),
            make_cell(' ', 0, 0, 0),
            make_cell('!', ATTR_INVERSE, 0, 0),
        ];
        let _no_cursor = build_terminal_row(&cells, fg(), bg(), &base_font(), None, None, bg());
        let _with_cursor =
            build_terminal_row(&cells, fg(), bg(), &base_font(), Some(2), None, bg());
    }

    #[test]
    fn cursor_and_selection_backgrounds_render_above_negative_z_images() {
        let cells = [
            make_cell('a', 0, 0, 0x112233FF),
            make_cell('b', 0, 0, 0x112233FF),
            make_cell('c', 0, 0, 0x112233FF),
        ];
        let row = build_terminal_row(
            &cells,
            fg(),
            bg(),
            &base_font(),
            Some(2),
            Some((1, 1)),
            bg(),
        );

        assert!(
            row.backgrounds
                .iter()
                .any(|run| run.start_col == 0 && run.len == 1 && !run.overlay)
        );
        assert!(
            row.backgrounds
                .iter()
                .any(|run| run.start_col == 1 && run.len == 1 && run.overlay)
        );
        assert!(
            row.backgrounds
                .iter()
                .any(|run| run.start_col == 2 && run.len == 1 && run.overlay)
        );
    }

    #[test]
    fn terminal_selection_row_range_handles_reverse_drag() {
        let snapshot = ScreenSnapshot {
            cols: 5,
            rows: 3,
            cells: Vec::new(),
            kitty_placements: Default::default(),
            dirty_rows: Vec::new(),
            cursor: VtCursor {
                col: 0,
                row: 0,
                visible: false,
            },
            alternate_screen: false,
            scrollbar: None,
            title: None,
            generation: 1,
        };
        let selection = TerminalSelection {
            anchor: (3, 1),
            extent: (1, 0),
        };

        assert_eq!(selection.row_range(0, &snapshot), Some((1, 4)));
        assert_eq!(selection.row_range(1, &snapshot), Some((0, 3)));
        assert_eq!(selection.row_range(2, &snapshot), None);
    }

    #[test]
    fn extracts_selected_text_from_snapshot() {
        let mut cells = vec![VtCell::default(); 12];
        for (idx, ch) in "one two     ".chars().enumerate() {
            cells[idx].codepoint = ch as u32;
        }
        let snapshot = ScreenSnapshot {
            cols: 4,
            rows: 3,
            cells,
            kitty_placements: Default::default(),
            dirty_rows: Vec::new(),
            cursor: VtCursor {
                col: 0,
                row: 0,
                visible: false,
            },
            alternate_screen: false,
            scrollbar: None,
            title: None,
            generation: 1,
        };

        let selection = TerminalSelection {
            anchor: (1, 0),
            extent: (2, 1),
        };

        assert_eq!(
            extract_selection_text(&snapshot, selection),
            Some("ne\ntwo".to_string())
        );
    }

    #[test]
    fn refresh_rows_include_old_and_new_cursor_rows() {
        let snapshot = ScreenSnapshot {
            cols: 4,
            rows: 3,
            cells: vec![Default::default(); 12],
            kitty_placements: Default::default(),
            dirty_rows: vec![1],
            cursor: VtCursor {
                col: 2,
                row: 2,
                visible: true,
            },
            alternate_screen: false,
            scrollbar: None,
            title: None,
            generation: 7,
        };

        let mut rows = rows_needing_refresh(
            &snapshot,
            Some(VtCursor {
                col: 1,
                row: 0,
                visible: true,
            }),
            false,
        );
        rows.sort_unstable();
        rows.dedup();

        assert_eq!(rows, vec![0, 1, 2]);
    }
}
