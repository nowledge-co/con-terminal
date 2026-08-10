//! Windows terminal view — drives the `con-ghostty` Windows backend
//! (`WindowsGhosttyApp` + `WindowsGhosttyTerminal` + a `RenderSession`
//! that owns the renderer, VT parser, and ConPTY for one pane).
//!
//! Same public type names as the macOS `ghostty_view` so the rest of
//! `con-app` (terminal_pane.rs, workspace/) compiles unchanged. The
//! `#[path]` selector in `main.rs` picks this file on Windows.
//!
//! Paint model:
//! - No child HWND. The renderer draws into an offscreen D3D11 texture
//!   and hands back BGRA bytes each dirty frame.
//! - Full redraws replace a CPU-side BGRA backing frame; dirty-row
//!   redraws patch that backing frame before publishing one
//!   `Arc<RenderImage>`. We do not layer translucent row-strip images
//!   over an old base image because alpha blending cannot erase stale
//!   glyph pixels. The terminal pane lives inside GPUI's
//!   DirectComposition tree so modals (settings, command palette) and
//!   newly-opened panes compose correctly — no z-order flashes, no
//!   "modal is 100% transparent over the pane".
//!
//! Lifecycle:
//!
//! 1. `GhosttyView::new(app, cwd, restored_screen_text, font_size, cx)` pre-allocates a
//!    `WindowsGhosttyTerminal` so `terminal_pane` can hold an Arc to
//!    it. No renderer/ConPTY yet — those are built lazily.
//! 2. `on_children_prepainted` captures the pane's bounds the first
//!    time they're known. At that point we spin up a `RenderSession`
//!    (Renderer + VT + ConPTY) sized to those physical pixels.
//! 3. Each subsequent prepaint: resize on geometry change, update DPI
//!    on scale-factor change, pump one `render_frame()`. When the
//!    frame is fresh we rebuild `cached_image` and `cx.notify()` so
//!    the next `render()` picks it up. Local user input marks the next
//!    render latency-critical so the freshest frame wins when the
//!    staging ring is otherwise clear, while resize/backlog frames stay
//!    non-blocking and may drop stale unread readbacks instead of
//!    stalling GPUI's thread.
//! 4. Drop releases the `RenderSession` and ends the child shell.

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use con_ghostty::{GhosttyApp, GhosttyScrollbar, GhosttySplitDirection, GhosttyTerminal};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::*;
use gpui_component::ActiveTheme;
use gpui_component::menu::ContextMenuExt;
use image::{Frame, RgbaImage};
use smallvec::SmallVec;

use crate::terminal_ime::{TerminalImeInputHandler, TerminalImeView};
use crate::terminal_links::{self, TerminalLink};
use crate::terminal_paste::{
    TerminalPastePayload, copy_selection_to_clipboard, payload_from_clipboard,
    payload_from_external_paths,
};
use crate::terminal_restore::{key_down_may_write_terminal, restored_terminal_output};
use con_ghostty::windows::host_view::{MouseEventMods, RenderSession};
use con_ghostty::windows::render::{FrameBgra, RenderOutcome};

const SCROLLBAR_INSET_PX: f32 = 4.0;
const SCROLLBAR_WIDTH_PX: f32 = 6.0;
const SCROLLBAR_MIN_THUMB_PX: f32 = 28.0;
const TERMINAL_PADDING_X_PX: f32 = 10.0;
const TERMINAL_PADDING_Y_PX: f32 = 8.0;

#[derive(Debug, Clone, Copy)]
struct ScrollbarDrag {
    start_y_px: f32,
    start_offset: u64,
    total: u64,
    len: u64,
    track_height_px: f32,
    thumb_height_px: f32,
}

#[derive(Debug, Clone, Copy)]
struct ScrollbarCache {
    generation: u64,
    state: Option<GhosttyScrollbar>,
}

fn mouse_mods_from(modifiers: &Modifiers) -> MouseEventMods {
    MouseEventMods {
        shift: modifiers.shift,
        alt: modifiers.alt,
        control: modifiers.control,
    }
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
    initial_font_size: f32,
    initialized: bool,
    /// Latched after a `RenderSession::new` failure so we don't re-try
    /// on every layout pass (the same DXGI / D3D errors would fire ~60×/s
    /// otherwise). User has to recreate the pane to clear it.
    init_failed: bool,
    /// Emit `GhosttyProcessExited` exactly once on shell death.
    process_exit_emitted: bool,
    last_cwd: Option<String>,
    /// Pane bounds in logical window pixels, captured during prepaint.
    pane_bounds: Option<Bounds<Pixels>>,
    scale_factor: f32,
    ime_marked_text: Option<String>,
    ime_selected_range: Option<Range<usize>>,
    /// Last physical-pixel size we sent to `session.resize`. Avoids
    /// resize churn when the logical bounds round to the same physical
    /// size frame-to-frame.
    last_physical_size: Option<(u32, u32)>,
    /// Last scale factor handed to `session.set_dpi`.
    last_scale_factor: f32,
    /// The most recently rendered frame, wrapped as a GPUI image.
    cached_image: Option<Arc<RenderImage>>,
    /// CPU-side copy of the current BGRA frame. Dirty-row readbacks
    /// replace byte ranges in this backing store before we publish a new
    /// full `RenderImage`. Keeping the replacement semantics here is
    /// required while the terminal background is translucent: GPUI image
    /// children alpha-composite, so row-strip overlays would blend with
    /// stale text instead of erasing it.
    cached_image_size: Option<(u32, u32)>,
    cached_frame: Option<Vec<u8>>,
    cached_frame_size: Option<(u32, u32)>,
    /// `GHOSTTY_TERMINAL_DATA_SCROLLBAR` is an expensive VT query.
    /// Cache it by VT generation so render can draw the scrollbar
    /// without polling libghostty-vt every frame.
    scrollbar_cache: Option<ScrollbarCache>,
    /// Replaced images, kept live until the next prepaint so the paint
    /// that referenced them has finished. Dropped after via
    /// `Window::drop_image` to evict sprite-atlas tiles.
    images_to_drop: Vec<Arc<RenderImage>>,
    scrollbar_drag: Option<ScrollbarDrag>,
    terminal_mouse_sequence_active: bool,
    /// SGR button index of the in-flight terminal mouse sequence, so the
    /// release/move-release report uses the same button as the press.
    terminal_mouse_sequence_button: Option<u8>,
    mouse_down_link: Option<TerminalLink>,
    suppress_link_mouse_up: bool,
    hovered_link: Option<TerminalLink>,
    last_mouse_position: Option<Point<Pixels>>,
    /// Cloned and handed to `RenderSession::new`; the ConPTY reader
    /// thread sends at most one queued signal while a repaint wake is
    /// pending. The coalescer task spawned in `new()` consumes that
    /// signal on the GPUI thread and pokes `cx.notify()` so freshly
    /// arrived shell output paints on the next prepaint instead of
    /// waiting for the next user input event.
    wake_tx: UnboundedSender<()>,
    wake_pending: Arc<AtomicBool>,
}

enum SyncRenderResult {
    Unchanged,
    Rendered { needs_followup_prepaint: bool },
    Pending,
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
        font_size: f32,
        cx: &mut Context<Self>,
    ) -> Self {
        let terminal = Arc::new(GhosttyTerminal::new());
        let (wake_tx, mut wake_rx) = unbounded::<()>();
        let wake_pending = Arc::new(AtomicBool::new(false));

        // Output wake path: the ConPTY reader may produce many chunks
        // while GPUI is blocked or the window is minimized. Queue at
        // most one pending wake; the renderer's mailbox owns latest-frame
        // semantics, so more `()` entries would only grow memory.
        let pending_for_task = wake_pending.clone();
        cx.spawn(async move |this, cx| {
            while wake_rx.next().await.is_some() {
                pending_for_task.store(false, Ordering::Release);
                if this.update(cx, |_, cx| cx.notify()).is_err() {
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
            initial_font_size: font_size,
            initialized: false,
            init_failed: false,
            process_exit_emitted: false,
            last_cwd: None,
            pane_bounds: None,
            scale_factor: 1.0,
            ime_marked_text: None,
            ime_selected_range: None,
            last_physical_size: None,
            last_scale_factor: 0.0,
            cached_image: None,
            cached_image_size: None,
            cached_frame: None,
            cached_frame_size: None,
            scrollbar_cache: None,
            images_to_drop: Vec::new(),
            scrollbar_drag: None,
            terminal_mouse_sequence_active: false,
            terminal_mouse_sequence_button: None,
            mouse_down_link: None,
            suppress_link_mouse_up: false,
            hovered_link: None,
            last_mouse_position: None,
            wake_tx,
            wake_pending,
        }
    }

    pub fn terminal(&self) -> Option<&Arc<GhosttyTerminal>> {
        self.terminal.as_ref()
    }

    pub fn write_or_queue(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.clear_restored_screen_text();
        }

        if let Some(terminal) = &self.terminal {
            terminal.write_to_pty(data);
        }
    }

    pub fn title(&self) -> Option<String> {
        self.terminal.as_ref().and_then(|t| t.title())
    }

    pub fn current_dir(&self) -> Option<String> {
        self.terminal
            .as_ref()
            .and_then(|t| t.current_dir())
            .or_else(|| self.initial_cwd.clone())
    }

    pub fn is_alive(&self) -> bool {
        self.terminal.as_ref().is_some_and(|t| t.is_alive())
    }

    pub fn surface_ready(&self) -> bool {
        self.initialized
    }

    #[allow(dead_code)]
    pub fn selection_text(&self) -> Option<String> {
        self.terminal.as_ref().and_then(|t| t.selection_text())
    }

    pub fn shutdown_surface(&mut self) {
        if let Some(terminal) = &self.terminal {
            terminal.request_close();
        }
        self.initialized = false;
        // Release our own Arcs. The sprite-atlas tiles will stay until
        // the window closes; no way to reach `Window::drop_image` from
        // here. A per-pane ~2×framebytes residue is acceptable.
        self.cached_image = None;
        self.cached_image_size = None;
        self.cached_frame = None;
        self.cached_frame_size = None;
        self.scrollbar_cache = None;
        self.scrollbar_drag = None;
        self.terminal_mouse_sequence_active = false;
        self.terminal_mouse_sequence_button = None;
        self.ime_marked_text = None;
        self.ime_selected_range = None;
        self.mouse_down_link = None;
        self.suppress_link_mouse_up = false;
        self.hovered_link = None;
        self.last_mouse_position = None;
        self.images_to_drop.clear();
        self.last_physical_size = None;
    }

    pub fn set_surface_focus_state(&mut self, focused: bool) {
        if let Some(terminal) = &self.terminal {
            terminal.set_focus(focused);
        }
    }

    pub fn ensure_initialized_for_control(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Initialization is lazy inside `ensure_session` once a real
        // layout pass hands us bounds and DPI. Claiming initialized here
        // would lie about the RenderSession's existence.
    }

    pub fn sync_surface_layout_for_host(
        &mut self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scale = window.scale_factor();
        let was_initialized = self.initialized;
        let bounds_changed = self.update_pane_bounds(bounds, scale);
        if was_initialized && !bounds_changed {
            return;
        }

        match self.sync_render(window) {
            SyncRenderResult::Pending | SyncRenderResult::Rendered { .. } => cx.notify(),
            SyncRenderResult::Unchanged if bounds_changed => cx.notify(),
            SyncRenderResult::Unchanged => {}
        }
    }

    /// Cross-platform hide hook used on macOS when switching tabs (each
    /// tab's child NSView is toggled so only the active tab's terminal
    /// paints). On Windows the renderer composites through GPUI's image
    /// path, and inactive tabs simply aren't in the element tree, so
    /// there's nothing to toggle — no-op.
    pub fn set_visible(&self, _visible: bool) {}

    pub fn sync_window_background_blur(&self) {
        // Windows uses DwmSetWindowAttribute(DWMWA_SYSTEMBACKDROP_TYPE)
        // at window-creation time; there's nothing per-pane to refresh.
    }

    pub fn drain_surface_state(
        &mut self,
        _sync_native_scroll: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.poll_terminal_state(cx)
    }

    pub fn pump_deferred_work(&mut self, cx: &mut Context<Self>) -> bool {
        self.poll_terminal_state(cx)
    }

    fn poll_terminal_state(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.initialized {
            return false;
        }

        let Some(terminal) = self.terminal.as_ref() else {
            return false;
        };

        let mut changed = false;

        let cwd = terminal.current_dir();
        if cwd != self.last_cwd {
            self.last_cwd = cwd.clone();
            changed = true;
            cx.emit(GhosttyCwdChanged(cwd));
        }

        // No action-callback channel on Windows (cf. macOS's
        // `wake_generation`). Poll `is_alive` so workspace's
        // `on_terminal_process_exited` runs when the child shell exits.
        if !self.process_exit_emitted && !terminal.is_alive() {
            self.process_exit_emitted = true;
            changed = true;
            cx.emit(GhosttyProcessExited);
        }

        changed
    }

    fn ensure_session(&mut self, width_px: u32, height_px: u32, dpi: u32) {
        if self.initialized || self.init_failed {
            return;
        }
        if width_px == 0 || height_px == 0 {
            return;
        }

        let mut config = self.app.renderer_config();
        if self.initial_font_size > 0.0 {
            config.font_size_px = self.initial_font_size;
        }
        config.initial_width = width_px;
        config.initial_height = height_px;

        let wake_tx = self.wake_tx.clone();
        let wake_pending = self.wake_pending.clone();
        let wake = move || {
            // `unbounded_send` only fails after the receiver is dropped,
            // which happens when the view dies — at which point losing
            // a wake is harmless.
            if wake_pending
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
                && wake_tx.unbounded_send(()).is_err()
            {
                wake_pending.store(false, Ordering::Release);
            }
        };

        let cwd = self.initial_cwd.as_deref().map(std::path::PathBuf::from);
        let initial_output = restored_terminal_output(self.restored_screen_text.as_deref());
        match RenderSession::new(width_px, height_px, dpi, config, cwd, initial_output, wake) {
            Ok(session) => {
                if let Some(terminal) = &self.terminal {
                    terminal.attach(session);
                }
                self.restored_screen_text = None;
                self.initialized = true;
                self.last_cwd = self.terminal.as_ref().and_then(|t| t.current_dir());
                self.last_physical_size = Some((width_px, height_px));
                self.last_scale_factor = dpi as f32 / 96.0;
                self.scrollbar_cache = None;
            }
            Err(err) => {
                log::error!("RenderSession::new failed: {:#}", err);
                self.init_failed = true;
            }
        }
    }

    fn update_pane_bounds(&mut self, bounds: Bounds<Pixels>, scale_factor: f32) -> bool {
        let bounds_changed = self.pane_bounds != Some(bounds);
        let scale_changed = (self.scale_factor - scale_factor).abs() > f32::EPSILON;
        self.pane_bounds = Some(bounds);
        self.scale_factor = scale_factor;
        bounds_changed || scale_changed
    }

    /// Drives session lifecycle (init/resize/DPI) and pumps one render
    /// using the most recently observed pane bounds. Returns whether
    /// the call produced a new image, needs another frame, or made no
    /// visible progress.
    fn sync_render(&mut self, window: &mut Window) -> SyncRenderResult {
        let sync_started = perf_trace_enabled().then(Instant::now);
        let Some(bounds) = self.pane_bounds else {
            return SyncRenderResult::Unchanged;
        };
        let scale_factor = self.scale_factor.max(f32::EPSILON);

        // Drop the tile that the PRIOR frame painted. Paint has already
        // flushed for that frame (we're in prepaint for the next one),
        // so its sprite-atlas entry is no longer referenced and we can
        // evict it without corrupting what we're about to paint.
        for old in self.images_to_drop.drain(..) {
            let _ = window.drop_image(old);
        }

        // `.ceil()` matches `Window::paint_image`, which does
        // `map_size(|size| size.ceil())` on the scaled physical quad. If
        // we render at `.round()` our texture ends up 1px smaller than
        // the quad on half-pixel bounds and LINEAR sampling blurs every
        // pixel by a tiny fraction.
        let width_px = ((f32::from(bounds.size.width) * scale_factor).ceil() as u32).max(1);
        let height_px = ((f32::from(bounds.size.height) * scale_factor).ceil() as u32).max(1);
        let dpi = (scale_factor * 96.0).round().max(1.0) as u32;

        self.ensure_session(width_px, height_px, dpi);
        if !self.initialized {
            return SyncRenderResult::Unchanged;
        }

        let Some(session_arc) = self.terminal.as_ref().map(|t| t.inner()) else {
            return SyncRenderResult::Unchanged;
        };
        let guard = session_arc.lock();
        let Some(session) = guard.as_ref() else {
            return SyncRenderResult::Unchanged;
        };

        if (scale_factor - self.last_scale_factor).abs() > f32::EPSILON {
            if let Err(err) = session.set_dpi(dpi) {
                log::warn!("RenderSession::set_dpi failed: {err:#}");
            }
            self.last_scale_factor = scale_factor;
        }

        if self.last_physical_size != Some((width_px, height_px)) {
            if let Err(err) = session.resize(width_px, height_px) {
                log::warn!("RenderSession::resize failed: {err:#}");
            }
            self.last_physical_size = Some((width_px, height_px));
        }

        let render_started = perf_trace_enabled().then(Instant::now);
        let outcome = match session.render_frame() {
            Ok(RenderOutcome::Unchanged) => SyncRenderResult::Unchanged,
            Ok(RenderOutcome::Rendered {
                frame,
                needs_followup_prepaint,
            }) => {
                let render_frame_ms = render_started
                    .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                    .unwrap_or(0.0);
                let patch_count = frame.patches.len();
                let image_started = perf_trace_enabled().then(Instant::now);
                if self.cache_rendered_frame(frame) {
                    if let Some(started) = sync_started {
                        let image_ms = image_started
                            .map(|image_started| image_started.elapsed().as_secs_f64() * 1000.0)
                            .unwrap_or(0.0);
                        log::info!(
                            target: "con::perf",
                            "win_sync_render outcome=rendered width_px={} height_px={} patches={} session_ms={:.3} image_ms={:.3} total_ms={:.3}",
                            width_px,
                            height_px,
                            patch_count,
                            render_frame_ms,
                            image_ms,
                            started.elapsed().as_secs_f64() * 1000.0,
                        );
                    }
                    SyncRenderResult::Rendered {
                        needs_followup_prepaint,
                    }
                } else if needs_followup_prepaint {
                    SyncRenderResult::Pending
                } else {
                    SyncRenderResult::Unchanged
                }
            }
            Ok(RenderOutcome::Pending) => {
                if let Some(started) = sync_started {
                    let render_frame_ms = render_started
                        .map(|render_started| render_started.elapsed().as_secs_f64() * 1000.0)
                        .unwrap_or(0.0);
                    log::info!(
                        target: "con::perf",
                        "win_sync_render outcome=pending width_px={} height_px={} session_ms={:.3} total_ms={:.3}",
                        width_px,
                        height_px,
                        render_frame_ms,
                        started.elapsed().as_secs_f64() * 1000.0,
                    );
                }
                SyncRenderResult::Pending
            }
            Err(err) => {
                log::warn!("RenderSession::render_frame failed: {err:#}");
                SyncRenderResult::Unchanged
            }
        };

        self.refresh_scrollbar_cache_from_session(session);
        outcome
    }

    fn refresh_scrollbar_cache_from_session(&mut self, session: &RenderSession) {
        let generation = session.generation();
        if self
            .scrollbar_cache
            .is_some_and(|cache| cache.generation == generation)
        {
            return;
        }
        let state = session.scrollbar().filter(Self::scrollbar_visible);
        self.scrollbar_cache = Some(ScrollbarCache { generation, state });
    }

    fn cache_rendered_frame(&mut self, frame: FrameBgra) -> bool {
        let frame_width = frame.width;
        let frame_height = frame.height;
        let frame_len = (frame_width as usize)
            .saturating_mul(frame_height as usize)
            .saturating_mul(4);
        if frame_width == 0 || frame_height == 0 || frame_len == 0 {
            return false;
        }

        if self.cached_frame_size != Some((frame_width, frame_height)) {
            self.cached_frame = None;
            self.cached_frame_size = None;
        }

        let mut changed = false;

        for patch in frame.patches {
            let patch_y = patch.y;
            let patch_height = patch.height;
            let is_full_frame = patch_y == 0 && patch_height == frame.height;
            let patch_len = (frame_width as usize)
                .saturating_mul(patch_height as usize)
                .saturating_mul(4);
            if patch_height == 0 || patch.bytes.len() != patch_len {
                log::warn!(
                    "Ignoring malformed Windows terminal patch: y={} height={} bytes={} expected={}",
                    patch_y,
                    patch_height,
                    patch.bytes.len(),
                    patch_len
                );
                continue;
            }

            if is_full_frame {
                self.cached_frame = Some(patch.bytes);
                self.cached_frame_size = Some((frame_width, frame_height));
                changed = true;
                continue;
            }

            let Some(backing_len) = self.cached_frame.as_ref().map(Vec::len) else {
                log::debug!("Ignoring partial Windows terminal patch before first full frame");
                continue;
            };
            if backing_len != frame_len {
                log::warn!(
                    "Dropping Windows terminal backing frame with unexpected len {} != {}",
                    backing_len,
                    frame_len
                );
                self.cached_frame = None;
                self.cached_frame_size = None;
                continue;
            }

            let patch_bottom = patch_y.saturating_add(patch_height).min(frame_height);
            if patch_y >= patch_bottom {
                continue;
            }

            let row_bytes = frame_width as usize * 4;
            let rows_to_copy = (patch_bottom - patch_y) as usize;
            let src_len = rows_to_copy * row_bytes;
            if src_len > patch.bytes.len() {
                log::warn!(
                    "Ignoring truncated Windows terminal patch: y={} rows={} bytes={} expected_at_least={}",
                    patch_y,
                    rows_to_copy,
                    patch.bytes.len(),
                    src_len
                );
                continue;
            }

            let Some(backing) = self.cached_frame.as_mut() else {
                continue;
            };
            for row in 0..rows_to_copy {
                let dst_start = (patch_y as usize + row) * row_bytes;
                let src_start = row * row_bytes;
                backing[dst_start..dst_start + row_bytes]
                    .copy_from_slice(&patch.bytes[src_start..src_start + row_bytes]);
            }
            changed = true;
        }

        if changed
            && let Some(bytes) = self.cached_frame.as_ref().cloned()
            && let Some(image) = bgra_frame_to_image(bytes, frame_width, frame_height)
        {
            if let Some(old) = self.cached_image.replace(image) {
                self.images_to_drop.push(old);
            }
            self.cached_image_size = Some((frame_width, frame_height));
            self.cached_frame_size = Some((frame_width, frame_height));
            return true;
        }

        if changed {
            self.cached_frame = None;
            self.cached_frame_size = None;
        }

        false
    }

    fn clear_restored_screen_text(&mut self) {
        self.restored_screen_text = None;
    }

    fn image_children(&self, gap_background: Option<Hsla>) -> Vec<AnyElement> {
        let mut children = Vec::with_capacity(usize::from(self.cached_image.is_some()) + 2);
        if let Some(img_arc) = self.cached_image.clone() {
            // Keep stale D3D readback frames at their original logical
            // size while pane layout is changing. Stretching the old
            // texture to the new pane bounds makes terminal text appear
            // zoomed for a frame when closing/splitting panes; the
            // clipped parent background is a safer placeholder until the
            // resized renderer publishes the next frame.
            let image = img(ImageSource::Render(img_arc)).object_fit(ObjectFit::Fill);
            let image = if let Some((frame_width, frame_height)) = self.cached_image_size {
                let scale_factor = self.scale_factor.max(f32::EPSILON);
                let image_width = frame_width as f32 / scale_factor;
                let image_height = frame_height as f32 / scale_factor;
                if let (Some(bounds), Some(background)) = (self.pane_bounds, gap_background) {
                    let content_width = f32::from(bounds.size.width);
                    let content_height = f32::from(bounds.size.height);
                    if image_width + 0.5 < content_width {
                        children.push(
                            div()
                                .absolute()
                                .left(px(image_width))
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .bg(background)
                                .into_any_element(),
                        );
                    }
                    if image_height + 0.5 < content_height {
                        children.push(
                            div()
                                .absolute()
                                .left_0()
                                .w(px(image_width.min(content_width).max(0.0)))
                                .top(px(image_height))
                                .bottom_0()
                                .bg(background)
                                .into_any_element(),
                        );
                    }
                }
                image.w(px(image_width)).h(px(image_height))
            } else {
                // `ObjectFit::Fill` keeps each image quad exactly equal
                // to its logical bounds. The default `Contain` applies
                // aspect-ratio letterboxing using float math, which
                // produces a quad a fraction of a pixel smaller than our
                // source texture; the LINEAR sprite sampler then blends
                // neighbouring texels and terminal cells show faint
                // speckles below the glyph baseline.
                image.size_full()
            };
            children.push(image.into_any_element());
        }

        children
    }

    /// Convert a window-coordinate mouse position into a 0-based
    /// (col, row) cell address. Returns `None` when we don't yet have a
    /// session / bounds to project into.
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
        let terminal = self.terminal.as_ref()?;
        let inner = terminal.inner();
        let guard = inner.lock();
        let session = guard.as_ref()?;
        let metrics = session.metrics();
        if metrics.cell_width_px == 0 || metrics.cell_height_px == 0 {
            return None;
        }
        let scale = self.scale_factor.max(f32::EPSILON);
        let width_px = ((f32::from(bounds.size.width) * scale).ceil() as u32).max(1);
        let height_px = ((f32::from(bounds.size.height) * scale).ceil() as u32).max(1);
        let cols = (width_px / metrics.cell_width_px.max(1)).max(1);
        let rows = (height_px / metrics.cell_height_px.max(1)).max(1);
        let grid_width_px = (cols * metrics.cell_width_px.max(1)) as f32;
        let grid_height_px = (rows * metrics.cell_height_px.max(1)) as f32;
        let local_x = f32::from(pos.x) - f32::from(bounds.origin.x);
        let local_y = f32::from(pos.y) - f32::from(bounds.origin.y);
        let mut phys_x = local_x * scale;
        let mut phys_y = local_y * scale;
        if clamp_to_grid {
            phys_x = phys_x.clamp(0.0, (grid_width_px - f32::EPSILON).max(0.0));
            phys_y = phys_y.clamp(0.0, (grid_height_px - f32::EPSILON).max(0.0));
        } else if phys_x < 0.0
            || phys_y < 0.0
            || phys_x >= width_px as f32
            || phys_y >= height_px as f32
            || phys_x >= grid_width_px
            || phys_y >= grid_height_px
        {
            return None;
        }
        let col = ((phys_x as u32) / metrics.cell_width_px.max(1)).min(cols - 1) as u16;
        let row = ((phys_y as u32) / metrics.cell_height_px.max(1)).min(rows - 1) as u16;
        Some((col, row))
    }

    fn link_at_position(&self, pos: Point<Pixels>) -> Option<TerminalLink> {
        let (col, row) = self.cell_from_event_position(pos)?;
        let terminal = self.terminal.as_ref()?;
        let inner = terminal.inner();
        let guard = inner.lock();
        let session = guard.as_ref()?;
        let snapshot = session.vt().snapshot();
        terminal_links::link_at_snapshot(&snapshot, col, row)
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
        self.last_mouse_position = None;
        changed
    }

    fn render_link_cursor_overlay(&self) -> Option<AnyElement> {
        let link = self.hovered_link.as_ref()?;
        let terminal = self.terminal.as_ref()?;
        let inner = terminal.inner();
        let guard = inner.lock();
        let session = guard.as_ref()?;
        let metrics = session.metrics();
        let scale = self.scale_factor.max(0.5);
        let cell_w = metrics.cell_width_px.max(1) as f32 / scale;
        let cell_h = metrics.cell_height_px.max(1) as f32 / scale;
        let width_cols = link.end_col.saturating_sub(link.start_col).max(1);

        Some(
            div()
                .absolute()
                .left(px(link.start_col as f32 * cell_w))
                .top(px(link.row as f32 * cell_h))
                .w(px(width_cols as f32 * cell_w))
                .h(px(cell_h))
                .bg(gpui::transparent_black())
                .cursor_pointer()
                .into_any_element(),
        )
    }

    fn forward_mouse_down(
        &self,
        button: u8,
        pos: Point<Pixels>,
        mods: MouseEventMods,
    ) -> bool {
        if let Some((col, row)) = self.cell_from_event_position(pos) {
            if let Some(terminal) = &self.terminal {
                let inner = terminal.inner();
                if let Some(session) = inner.lock().as_ref() {
                    session.mouse_down(button, col, row, mods);
                    return true;
                }
            }
        }
        false
    }

    fn forward_mouse_drag(&self, pos: Point<Pixels>, mods: MouseEventMods, clamp: bool) {
        let cell = if clamp {
            self.clamped_cell_from_event_position(pos)
        } else {
            self.cell_from_event_position(pos)
        };
        if let Some((col, row)) = cell {
            if let Some(terminal) = &self.terminal {
                let inner = terminal.inner();
                if let Some(session) = inner.lock().as_ref() {
                    session.mouse_drag(col, row, mods);
                }
            }
        }
    }

    fn forward_mouse_up(
        &self,
        button: u8,
        pos: Point<Pixels>,
        mods: MouseEventMods,
        clamp: bool,
    ) {
        let cell = if clamp {
            self.clamped_cell_from_event_position(pos)
        } else {
            self.cell_from_event_position(pos)
        };
        if let Some((col, row)) = cell {
            if let Some(terminal) = &self.terminal {
                let inner = terminal.inner();
                if let Some(session) = inner.lock().as_ref() {
                    session.mouse_up(button, col, row, mods);
                }
            }
        }
    }

    fn forward_scroll(&self, pos: Point<Pixels>, delta: ScrollDelta, mods: MouseEventMods) -> bool {
        let Some(terminal) = &self.terminal else {
            return false;
        };
        let inner = terminal.inner();
        let guard = inner.lock();
        let Some(session) = guard.as_ref() else {
            return false;
        };
        let delta_y_px = match delta {
            ScrollDelta::Pixels(p) => f32::from(p.y),
            ScrollDelta::Lines(p) => {
                let metrics = session.metrics();
                p.y * metrics.cell_height_px.max(1) as f32
            }
        };
        if delta_y_px.abs() < f32::EPSILON {
            return false;
        }

        // Only report wheel to the shell when it has explicitly enabled
        // mouse tracking (SGR). Otherwise scroll Con's own viewport via
        // libghostty-vt. Shift bypasses reporting per xterm convention
        // so the user can scroll Con's scrollback even when a TUI has
        // `set mouse=a`.
        if !session.mouse_tracking_active() || mods.shift {
            session.scroll_viewport_or_alt_screen(delta_y_px, !mods.shift);
            return true;
        }

        let Some((col0, row0)) = self.cell_from_event_position(pos) else {
            return false;
        };
        // `forward_wheel` expects 1-based SGR coordinates.
        session.forward_wheel(col0 + 1, row0 + 1, delta_y_px, mods);
        false
    }

    fn scrollbar_visible(scrollbar: &GhosttyScrollbar) -> bool {
        scrollbar.total > scrollbar.len && scrollbar.len > 0 && scrollbar.total > 0
    }

    fn cached_scrollbar_state(&self) -> Option<GhosttyScrollbar> {
        self.scrollbar_cache.and_then(|cache| cache.state)
    }

    fn scrollbar_layout(&self, scrollbar: GhosttyScrollbar) -> Option<(f32, f32, f32)> {
        let bounds = self.pane_bounds?;
        let height = f32::from(bounds.size.height);
        let track_height = (height - (SCROLLBAR_INSET_PX * 2.0)).max(0.0);
        if track_height <= 0.0 || scrollbar.total <= scrollbar.len || scrollbar.len == 0 {
            return None;
        }
        let thumb_height = ((scrollbar.len as f32 / scrollbar.total as f32) * track_height)
            .clamp(SCROLLBAR_MIN_THUMB_PX.min(track_height), track_height);
        let travel = (track_height - thumb_height).max(0.0);
        let max_offset = scrollbar.total.saturating_sub(scrollbar.len).max(1);
        let offset = scrollbar.offset.min(max_offset);
        let thumb_top = SCROLLBAR_INSET_PX + (offset as f32 / max_offset as f32) * travel;
        Some((track_height, thumb_height, thumb_top))
    }

    fn start_scrollbar_drag(&mut self, pos: Point<Pixels>) {
        self.refresh_scrollbar_cache();
        let Some(scrollbar) = self.cached_scrollbar_state() else {
            return;
        };
        let Some((track_height_px, thumb_height_px, _)) = self.scrollbar_layout(scrollbar) else {
            return;
        };
        self.scrollbar_drag = Some(ScrollbarDrag {
            start_y_px: f32::from(pos.y),
            start_offset: scrollbar.offset,
            total: scrollbar.total,
            len: scrollbar.len,
            track_height_px,
            thumb_height_px,
        });
    }

    fn drag_scrollbar(&mut self, pos: Point<Pixels>) {
        let Some(drag) = self.scrollbar_drag else {
            return;
        };
        let max_offset = drag.total.saturating_sub(drag.len);
        if max_offset == 0 {
            return;
        }
        let travel = (drag.track_height_px - drag.thumb_height_px).max(1.0);
        let delta_px = f32::from(pos.y) - drag.start_y_px;
        let delta_rows = (delta_px / travel) * max_offset as f32;
        let target = (drag.start_offset as f32 + delta_rows)
            .round()
            .clamp(0.0, max_offset as f32) as u64;
        self.scroll_viewport_to_offset(target);
    }

    fn page_scrollbar_toward(&mut self, pos: Point<Pixels>) {
        self.refresh_scrollbar_cache();
        let Some(scrollbar) = self.cached_scrollbar_state() else {
            return;
        };
        let Some((_, thumb_height, thumb_top)) = self.scrollbar_layout(scrollbar) else {
            return;
        };
        let Some(bounds) = self.pane_bounds else {
            return;
        };
        let local_y = f32::from(pos.y) - f32::from(bounds.origin.y);
        let thumb_bottom = thumb_top + thumb_height;
        let rows = scrollbar.len.max(1) as isize;
        if local_y < thumb_top {
            self.scroll_viewport_rows(-rows);
        } else if local_y > thumb_bottom {
            self.scroll_viewport_rows(rows);
        }
    }

    fn scroll_viewport_rows(&mut self, rows: isize) {
        let Some(terminal) = self.terminal.clone() else {
            return;
        };
        let inner = terminal.inner();
        if let Some(session) = inner.lock().as_ref() {
            session.scroll_viewport_rows(rows);
            self.refresh_scrollbar_cache_from_session(session);
        }
    }

    fn scroll_viewport_to_offset(&mut self, offset: u64) {
        let Some(terminal) = self.terminal.clone() else {
            return;
        };
        let inner = terminal.inner();
        if let Some(session) = inner.lock().as_ref() {
            session.scroll_viewport_to_offset(offset);
            self.refresh_scrollbar_cache_from_session(session);
        }
    }

    fn refresh_scrollbar_cache(&mut self) {
        let Some(terminal) = self.terminal.clone() else {
            self.scrollbar_cache = None;
            return;
        };
        let inner = terminal.inner();
        let guard = inner.lock();
        let Some(session) = guard.as_ref() else {
            self.scrollbar_cache = None;
            return;
        };
        self.refresh_scrollbar_cache_from_session(session);
    }

    fn render_terminal_scrollbar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let scrollbar = self.cached_scrollbar_state()?;
        let (_, thumb_height, thumb_top) = self.scrollbar_layout(scrollbar)?;
        let theme = cx.theme();
        let thumb_color = theme.foreground.opacity(0.28);
        let thumb_hover_color = theme.foreground.opacity(0.42);

        Some(
            div()
                .absolute()
                .top(px(SCROLLBAR_INSET_PX))
                .right(px(2.0))
                .bottom(px(SCROLLBAR_INSET_PX))
                .w(px(SCROLLBAR_WIDTH_PX + 4.0))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        window.prevent_default();
                        this.page_scrollbar_toward(event.position);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(thumb_top - SCROLLBAR_INSET_PX))
                        .right(px(2.0))
                        .w(px(SCROLLBAR_WIDTH_PX))
                        .h(px(thumb_height))
                        .rounded(px(SCROLLBAR_WIDTH_PX / 2.0))
                        .bg(thumb_color)
                        .hover(|style| style.bg(thumb_hover_color))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                                window.prevent_default();
                                this.start_scrollbar_drag(event.position);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Translate a GPUI `KeyDownEvent` into bytes and forward to the
    /// ConPTY. Returns `true` if the key was handled (so GPUI can stop
    /// propagation).
    ///
    /// GPUI owns keyboard focus on Windows — there's no child HWND in
    /// the focus chain — so we translate at this layer into byte
    /// sequences a terminal emulator expects. DECCKM-aware arrows live
    /// alongside the printable / Ctrl-letter paths.
    fn handle_key_down(
        &self,
        event: &KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(terminal) = self.terminal.as_ref() else {
            return false;
        };
        let keystroke = &event.keystroke;

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

        // Plain Ctrl+C should copy terminal selection on Windows/Linux when
        // text is selected; otherwise it must keep its shell meaning (^C).
        if keystroke.modifiers.control
            && !keystroke.modifiers.shift
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
            && keystroke.key == "c"
            && copy_selection_to_clipboard(terminal, cx)
        {
            cx.notify();
            return true;
        }

        // Ctrl+Shift+C / Ctrl+Shift+V → clipboard. These must run ahead
        // of the generic Ctrl-letter path below, which would otherwise
        // emit ^C / ^V to the shell.
        if keystroke.modifiers.control
            && keystroke.modifiers.shift
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
        {
            match keystroke.key.as_str() {
                "c" => {
                    if copy_selection_to_clipboard(terminal, cx) {
                        cx.notify();
                    }
                    return true;
                }
                "v" => {
                    paste_from_clipboard(terminal, cx);
                    return true;
                }
                _ => {}
            }
        }

        let decckm = {
            let inner = terminal.inner();
            inner.lock().as_ref().is_some_and(|s| s.is_decckm())
        };

        // Named / special keys — arrows, home/end, page nav, insert,
        // delete, F1-F12, tab/shift-tab, enter, backspace, escape. Each
        // honours DECCKM (arrows) and xterm modifier-parameter encoding
        // for Ctrl/Shift/Alt (CSI 1;m<X> for arrows + F1-F4, CSI n;m~
        // for tilde-terminated keys).
        if let Some(bytes) = encode_special_key(&keystroke.key, &keystroke.modifiers, decckm) {
            terminal.send_text(&bytes);
            return true;
        }

        if event.prefer_character_input
            && !keystroke.modifiers.control
            && keystroke
                .key_char
                .as_deref()
                .is_some_and(|text| !text.is_empty())
        {
            return false;
        }

        // Ctrl + defined ASCII keys → C0 control bytes. This includes
        // punctuation controls terminals rely on, such as Ctrl+] for tmux's
        // `C-]` prefix (GS / 0x1d), not just Ctrl+A-Z.
        if keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
            && (keystroke.key.len() == 1
                || keystroke
                    .key_char
                    .as_deref()
                    .is_some_and(|text| text.len() == 1))
        {
            if let Some(code) = crate::terminal_keys::ctrl_keystroke_to_c0(
                &keystroke.key,
                keystroke.key_char.as_deref(),
                keystroke.modifiers.shift,
            ) {
                let byte = [code];
                let text = std::str::from_utf8(&byte)
                    .expect("terminal C0 control bytes are always valid UTF-8");
                terminal.send_text(text);
                return true;
            }
        }

        // Alt + printable ASCII → ESC + char (meta-prefix convention
        // readline / vim / tmux / fzf all recognise: Alt+B word-back,
        // Alt+F word-forward, Alt+D word-delete, Alt+. last-arg, …).
        // Only when Alt is the only app-consumable modifier — AltGr-
        // produced characters already arrive decoded via `key_char`
        // without the `alt` flag on most layouts, and Ctrl+Alt is left
        // for the terminal's own modifyOtherKeys semantics if added
        // later.
        if keystroke.modifiers.alt && !keystroke.modifiers.control && !keystroke.modifiers.platform
        {
            if let Some(ch) = keystroke.key_char.as_deref().filter(|s| !s.is_empty()) {
                let mut out = String::with_capacity(1 + ch.len());
                out.push('\x1b');
                out.push_str(ch);
                terminal.send_text(&out);
                return true;
            }
        }

        if let Some(ch) = keystroke.key_char.as_deref() {
            if !ch.is_empty() {
                if !keystroke.modifiers.control
                    && !keystroke.modifiers.alt
                    && !keystroke.modifiers.platform
                {
                    return false;
                }
                terminal.send_text(ch);
                return true;
            }
        }
        if keystroke.key.len() == 1 {
            if !keystroke.modifiers.control
                && !keystroke.modifiers.alt
                && !keystroke.modifiers.platform
            {
                return false;
            }
            terminal.send_text(&keystroke.key);
            return true;
        }

        false
    }

    fn ime_cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        let bounds = self.pane_bounds?;
        let terminal = self.terminal.as_ref()?;
        let inner = terminal.inner();
        let guard = inner.lock();
        let session = guard.as_ref()?;
        let snapshot = session.vt().snapshot();
        let metrics = session.metrics();
        if metrics.cell_width_px == 0 || metrics.cell_height_px == 0 {
            return None;
        }

        let scale = self.scale_factor.max(f32::EPSILON);
        let cell_width = metrics.cell_width_px as f32 / scale;
        let cell_height = metrics.cell_height_px as f32 / scale;
        let col = snapshot.cursor.col.min(snapshot.cols.saturating_sub(1)) as f32;
        let row = snapshot.cursor.row.min(snapshot.rows.saturating_sub(1)) as f32;

        Some(Bounds::new(
            point(
                bounds.origin.x + px(col * cell_width),
                bounds.origin.y + px(row * cell_height),
            ),
            size(px(cell_width.max(1.0)), px(cell_height.max(1.0))),
        ))
    }

    fn placeholder_background(&self) -> Option<Hsla> {
        let config = self.app.renderer_config();
        let opacity = config.background_opacity.clamp(0.0, 1.0);
        if opacity <= f32::EPSILON {
            return None;
        }

        Some(
            Rgba {
                r: config.clear_color[0].clamp(0.0, 1.0),
                g: config.clear_color[1].clamp(0.0, 1.0),
                b: config.clear_color[2].clamp(0.0, 1.0),
                a: opacity,
            }
            .into(),
        )
    }
}

/// Wrap a BGRA readback buffer as a `RenderImage`. `RenderImage`
/// internally stores BGRA already (see `3pp/zed/crates/gpui/src/elements/img.rs`,
/// where the loader swaps RGBA→BGRA on decode), so we feed the D3D11
/// `DXGI_FORMAT_B8G8R8A8_UNORM` readback directly without a swap.
fn bgra_frame_to_image(bytes: Vec<u8>, width: u32, height: u32) -> Option<Arc<RenderImage>> {
    let expected = (width as usize) * (height as usize) * 4;
    if bytes.len() != expected {
        log::warn!(
            "bgra_frame_to_image: byte len {} != expected {} ({}x{})",
            bytes.len(),
            expected,
            width,
            height
        );
        return None;
    }
    let buffer = RgbaImage::from_raw(width, height, bytes)?;
    let frame = Frame::new(buffer);
    let data: SmallVec<[Frame; 1]> = SmallVec::from_buf([frame]);
    Some(Arc::new(RenderImage::new(data)))
}

/// xterm modifier parameter (`m`) — `1 + (shift | alt<<1 | ctrl<<2)`,
/// where `1` itself means "no modifier" (so the encoder omits it).
///
/// Used in CSI `1;m{A|B|C|D|H|F|P|Q|R|S}` and CSI `{n};m~` sequences.
/// Source: xterm's "PC-Style Function Keys" encoding.
fn xterm_modifier_param(modifiers: &Modifiers) -> Option<u8> {
    let mask = u8::from(modifiers.shift)
        | (u8::from(modifiers.alt) << 1)
        | (u8::from(modifiers.control) << 2);
    if mask == 0 { None } else { Some(1 + mask) }
}

/// Translate a GPUI key name + modifiers into the byte sequence the
/// shell / TUI expects. Returns `None` for keys that should flow
/// through the printable-character path (letters, digits, symbols).
///
/// Covers arrows (DECCKM-aware and modifier-aware), home/end, pageup/
/// pagedown, insert/delete, F1-F12, enter, backspace, tab/shift-tab,
/// escape. xterm modifier encoding is applied uniformly: any combination
/// of Shift/Alt/Ctrl shifts the sequence into its CSI `1;m<final>`
/// (arrows, home/end, F1-F4) or CSI `n;m~` (tilde-terminated) form.
fn encode_special_key(key: &str, modifiers: &Modifiers, decckm: bool) -> Option<String> {
    let m = xterm_modifier_param(modifiers);

    let tilde = |code: u8| match m {
        Some(m) => format!("\x1b[{};{}~", code, m),
        None => format!("\x1b[{}~", code),
    };

    // CSI-1-final covers arrows + home/end + F1-F4. For the no-modifier
    // case: arrows honour DECCKM, F1-F4 use SS3 (ESC O x), home/end use
    // plain CSI.
    let csi1 = |final_byte: char, ss3_when_plain: bool, decckm_arrow: bool| match m {
        Some(m) => format!("\x1b[1;{}{}", m, final_byte),
        None if decckm_arrow && decckm => format!("\x1bO{}", final_byte),
        None if ss3_when_plain => format!("\x1bO{}", final_byte),
        None => format!("\x1b[{}", final_byte),
    };

    Some(match key {
        "up" => csi1('A', false, true),
        "down" => csi1('B', false, true),
        "right" => csi1('C', false, true),
        "left" => csi1('D', false, true),
        "home" => csi1('H', false, false),
        "end" => csi1('F', false, false),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "f1" => csi1('P', true, false),
        "f2" => csi1('Q', true, false),
        "f3" => csi1('R', true, false),
        "f4" => csi1('S', true, false),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "enter" | "return" => "\r".into(),
        "escape" => "\x1b".into(),
        // Alt+Backspace is readline's word-delete (ESC + DEL).
        "backspace" if modifiers.alt && !modifiers.control && !modifiers.platform => {
            "\x1b\x7f".into()
        }
        "backspace" => "\x7f".into(),
        // Shift+Tab is the xterm "back-tab" CSI Z — bash/zsh completion
        // menus and fzf use it to cycle backwards.
        "tab" if modifiers.shift && !modifiers.control && !modifiers.platform => "\x1b[Z".into(),
        "tab" => "\t".into(),
        _ => return None,
    })
}

fn send_paste(terminal: &GhosttyTerminal, text: &str) {
    // Bracketed paste lets the shell tell pasted text apart from real
    // typing, so vim / readline can disable auto-indent. Hold the lock
    // across both wrapper writes so the session can't be swapped out
    // mid-paste (which would strip the trailing `ESC[201~`).
    let inner = terminal.inner();
    let guard = inner.lock();
    if let Some(session) = guard.as_ref() {
        if session.is_bracketed_paste() {
            session.write_pty_raw(b"\x1b[200~");
            session.write_input(text);
            session.write_pty_raw(b"\x1b[201~");
        } else {
            session.write_input(text);
        }
    }
}

fn send_terminal_paste_payload(terminal: &GhosttyTerminal, payload: TerminalPastePayload) {
    match payload {
        TerminalPastePayload::Text(text) if !text.is_empty() => send_paste(terminal, &text),
        TerminalPastePayload::ForwardCtrlV => terminal.send_text("\x16"),
        TerminalPastePayload::Text(_) => {}
    }
}

fn paste_from_clipboard(terminal: &GhosttyTerminal, cx: &mut App) -> bool {
    let Some(payload) = cx
        .read_from_clipboard()
        .and_then(|item| payload_from_clipboard(&item))
    else {
        return false;
    };

    send_terminal_paste_payload(terminal, payload);
    true
}

impl Focusable for GhosttyView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

type WindowsTerminalInputHandler = TerminalImeInputHandler<GhosttyView>;

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

    fn send_ime_text(&mut self, text: &str, _cx: &mut Context<Self>) {
        if !text.is_empty() {
            self.clear_restored_screen_text();
        }
        if let Some(terminal) = &self.terminal {
            terminal.send_text(text);
        }
    }

    fn ime_cursor_bounds(&self) -> Option<Bounds<Pixels>> {
        GhosttyView::ime_cursor_bounds(self)
    }
}

impl Render for GhosttyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.sync_render(window) {
            SyncRenderResult::Pending => cx.notify(),
            SyncRenderResult::Rendered {
                needs_followup_prepaint: true,
            } => cx.notify(),
            SyncRenderResult::Rendered {
                needs_followup_prepaint: false,
            }
            | SyncRenderResult::Unchanged => {}
        }

        let terminal_background = self.placeholder_background();
        let background = if self.cached_image.is_none() {
            terminal_background
        } else {
            None
        }
        .unwrap_or_else(|| cx.theme().background.opacity(0.0));
        let padding_background =
            terminal_background.unwrap_or_else(|| cx.theme().background.opacity(0.0));
        let entity = cx.entity().downgrade();
        let input_entity = entity.clone();
        let menu_entity = entity.clone();
        let mut terminal_children = self.image_children(terminal_background);
        if let Some(overlay) = self.render_link_cursor_overlay() {
            terminal_children.push(overlay);
        }
        if let Some(scrollbar) = self.render_terminal_scrollbar(cx) {
            terminal_children.push(scrollbar);
        }

        let focus = self.focus_handle.clone();
        let input_focus = focus.clone();
        let context_focus = focus.clone();
        let menu_focus = focus.clone();
        let ui_font = cx.theme().font_family.clone();

        div()
            .flex()
            .flex_col()
            .size_full()
            .font_family(ui_font)
            .min_w_0()
            .min_h_0()
            .key_context("GhosttyTerminal")
            .track_focus(&self.focus_handle)
            .id(&self.focus_handle)
            .bg(background)
            .on_action(cx.listener(|this, _: &ConsumeTab, window, _cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                this.clear_restored_screen_text();
                if let Some(terminal) = &this.terminal {
                    terminal.send_text("\t");
                }
            }))
            .on_action(cx.listener(|this, _: &ConsumeTabPrev, window, _cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                this.clear_restored_screen_text();
                if let Some(terminal) = &this.terminal {
                    terminal.send_text("\x1b[Z");
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Copy, _window, cx| {
                if let Some(terminal) = &this.terminal {
                    if copy_selection_to_clipboard(terminal, cx) {
                        cx.notify();
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &crate::Paste, _window, cx| {
                this.clear_restored_screen_text();
                if let Some(terminal) = &this.terminal
                    && paste_from_clipboard(terminal, cx)
                {
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
                this.clear_restored_screen_text();
                if let Some(terminal) = &this.terminal {
                    send_terminal_paste_payload(terminal, payload);
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if !this.focus_handle.is_focused(window) {
                    return;
                }
                let special_key_writes =
                    encode_special_key(&event.keystroke.key, &event.keystroke.modifiers, false)
                        .is_some();
                if key_down_may_write_terminal(event, special_key_writes) {
                    this.clear_restored_screen_text();
                }
                if this.handle_key_down(event, window, cx) {
                    window.prevent_default();
                    cx.stop_propagation();
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
                    this.last_mouse_position = Some(event.position);
                    this.mouse_down_link = None;
                    this.suppress_link_mouse_up = false;
                    let _ = this.update_hovered_link(&event.modifiers);
                    // SGR button 2 = right; unconsumed when tracking is off.
                    let active = this
                        .forward_mouse_down(2, event.position, mouse_mods_from(&event.modifiers));
                    this.terminal_mouse_sequence_active = active;
                    if active {
                        this.terminal_mouse_sequence_button = Some(2);
                    }
                    cx.emit(GhosttyFocusChanged);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&focus, cx);
                    this.last_mouse_position = Some(event.position);
                    this.terminal_mouse_sequence_active = false;
                    this.terminal_mouse_sequence_button = None;
                    this.mouse_down_link = None;
                    this.suppress_link_mouse_up = false;
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
                    this.terminal_mouse_sequence_active =
                        this.forward_mouse_down(0, event.position, mouse_mods_from(&event.modifiers));
                    if this.terminal_mouse_sequence_active {
                        this.terminal_mouse_sequence_button = Some(0);
                    }
                    cx.emit(GhosttyFocusChanged);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                this.last_mouse_position = Some(event.position);
                if this.scrollbar_drag.is_some() {
                    if event.pressed_button != Some(MouseButton::Left) {
                        this.scrollbar_drag = None;
                        this.update_hovered_link(&event.modifiers);
                        cx.notify();
                        return;
                    }
                    this.drag_scrollbar(event.position);
                    cx.notify();
                    return;
                }
                if this.suppress_link_mouse_up {
                    let mut changed = this.update_hovered_link(&event.modifiers);
                    if event.pressed_button != Some(MouseButton::Left) {
                        changed |= this.mouse_down_link.take().is_some();
                        this.suppress_link_mouse_up = false;
                    } else if let Some(down_link) = this.mouse_down_link.as_ref() {
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
                let hover_changed = this.update_hovered_link(&event.modifiers);
                if event.pressed_button == Some(MouseButton::Left) {
                    if this.terminal_mouse_sequence_active {
                        this.forward_mouse_drag(
                            event.position,
                            mouse_mods_from(&event.modifiers),
                            true,
                        );
                        cx.notify();
                    } else if hover_changed {
                        cx.notify();
                    }
                } else if this.terminal_mouse_sequence_active {
                    let button = this.terminal_mouse_sequence_button.unwrap_or(0);
                    this.forward_mouse_up(button, event.position, mouse_mods_from(&event.modifiers), true);
                    this.terminal_mouse_sequence_active = false;
                    this.terminal_mouse_sequence_button = None;
                    cx.notify();
                } else if hover_changed {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.last_mouse_position = Some(event.position);
                    if this.scrollbar_drag.take().is_some() {
                        this.update_hovered_link(&event.modifiers);
                        cx.notify();
                        return;
                    }
                    let had_terminal_mouse_sequence = this.terminal_mouse_sequence_active;
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
                        this.update_hovered_link(&event.modifiers);
                        cx.notify();
                        return;
                    }
                    if had_terminal_mouse_sequence {
                        let button = this.terminal_mouse_sequence_button.unwrap_or(0);
                        this.forward_mouse_up(
                            button,
                            event.position,
                            mouse_mods_from(&event.modifiers),
                            true,
                        );
                    }
                    this.terminal_mouse_sequence_active = false;
                    this.terminal_mouse_sequence_button = None;
                    let _ = this.update_hovered_link(&event.modifiers);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                    this.last_mouse_position = Some(event.position);
                    if this.terminal_mouse_sequence_active {
                        let button = this.terminal_mouse_sequence_button.unwrap_or(2);
                        this.forward_mouse_up(
                            button,
                            event.position,
                            mouse_mods_from(&event.modifiers),
                            true,
                        );
                        this.terminal_mouse_sequence_active = false;
                        this.terminal_mouse_sequence_button = None;
                    }
                    let _ = this.update_hovered_link(&event.modifiers);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                this.last_mouse_position = Some(event.position);
                let _ = this.update_hovered_link(&event.modifiers);
                let scrolled_viewport = this.forward_scroll(
                    event.position,
                    event.delta,
                    mouse_mods_from(&event.modifiers),
                );
                if scrolled_viewport && this.terminal_mouse_sequence_active {
                    this.forward_mouse_drag(
                        event.position,
                        mouse_mods_from(&event.modifiers),
                        true,
                    );
                }
                cx.notify();
            }))
            .child(
                div()
                    .relative()
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
                                if view.update_pane_bounds(bounds, scale) {
                                    cx.notify();
                                }
                            });
                        }
                    })
                    // Keep the terminal content as the first measured child:
                    // prepaint uses bounds_list.first() to size the D3D grid,
                    // while the gutter fills below only cover the inset edges.
                    .child(
                        div()
                            .absolute()
                            .left(px(TERMINAL_PADDING_X_PX))
                            .right(px(TERMINAL_PADDING_X_PX))
                            .top(px(TERMINAL_PADDING_Y_PX))
                            .bottom(px(TERMINAL_PADDING_Y_PX))
                            .overflow_hidden()
                            .children(terminal_children)
                            .child(
                                canvas(
                                    |_, _, _| {},
                                    move |_, _, window, cx| {
                                        window.handle_input(
                                            &input_focus,
                                            WindowsTerminalInputHandler::new(input_entity.clone()),
                                            cx,
                                        );
                                    },
                                )
                                .absolute()
                                .size_full(),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top_0()
                            .h(px(TERMINAL_PADDING_Y_PX))
                            .bg(padding_background),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(TERMINAL_PADDING_Y_PX))
                            .bg(padding_background),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(TERMINAL_PADDING_Y_PX))
                            .bottom(px(TERMINAL_PADDING_Y_PX))
                            .w(px(TERMINAL_PADDING_X_PX))
                            .bg(padding_background),
                    )
                    .child(
                        div()
                            .absolute()
                            .right_0()
                            .top(px(TERMINAL_PADDING_Y_PX))
                            .bottom(px(TERMINAL_PADDING_Y_PX))
                            .w(px(TERMINAL_PADDING_X_PX))
                            .bg(padding_background),
                    ),
            )
            .context_menu(move |menu, window, cx| {
                // Empty PopupMenu renders nothing; suppress con's menu while
                // mouse reporting is active (right-click went to the app).
                let mouse_tracking_active = menu_entity.upgrade().is_some_and(|view| {
                    let Some(terminal) = view.read(cx).terminal() else {
                        return false;
                    };
                    let inner = terminal.inner().lock();
                    inner
                        .as_ref()
                        .is_some_and(RenderSession::mouse_tracking_active)
                });
                if mouse_tracking_active {
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
        if let Some(terminal) = &self.terminal {
            terminal.request_close();
        }
    }
}

fn perf_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("CON_GHOSTTY_PROFILE").is_some_and(|v| !v.is_empty() && v != "0")
    })
}
