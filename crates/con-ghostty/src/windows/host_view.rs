//! Terminal pane session: Renderer + VT parser + ConPTY.
//!
//! No child HWND is created. The renderer draws into an offscreen D3D11
//! texture; the caller reads back BGRA bytes each dirty frame and hands
//! them to GPUI as an `ImageSource::Render(Arc<RenderImage>)`. That puts
//! terminal content inside GPUI's own DirectComposition tree, which
//! eliminates the z-order problems the old WS_CHILD HWND had with
//! modals (settings / command palette painted under the HWND) and with
//! brand-new panes (the HWND would render one transparent frame before
//! its first Present).
//!
//! Thread model:
//! - All `Renderer` calls happen on GPUI's main thread via
//!   [`RenderSession::render_frame`] et al.
//! - The VT parser is fed from the ConPTY reader thread
//!   (`conpty.rs`) and snapshotted read-only on the main thread under
//!   its own Mutex.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use parking_lot::Mutex;

use crate::stub::GhosttyScrollbar;
use crate::transcript::{TranscriptBuffer, snapshot_to_lines};

use super::conpty::{ConPty, PtySize};
use super::profile::{perf_trace_enabled, perf_trace_verbose};
use super::render::{RenderOutcome, Renderer, RendererConfig, Selection, ThemeColors};
use super::vt::{ScreenSnapshot, VtScreen};

use super::render::CellMetrics;

/// Owns the D3D11 renderer, the VT parser, and the ConPTY child shell
/// for a single terminal pane. Exposes methods the GPUI view calls to
/// feed input, query state, and pull the latest rendered frame.
pub struct RenderSession {
    renderer: Mutex<Renderer>,
    vt: Arc<VtScreen>,
    transcript: Arc<Mutex<TranscriptBuffer>>,
    conpty: Arc<ConPty>,
    shell_cwd: Option<PathBuf>,
    config: Mutex<RendererConfig>,
    base_font_size_px: f32,
    dpi: AtomicU32,
    /// When a local user action mutates terminal state (typing, paste,
    /// mouse selection), the next render should prefer the freshest
    /// frame over the lowest-latency non-blocking staging drain. This
    /// avoids showing the stale pre-input frame for one more prepaint.
    low_latency_requested: AtomicBool,
    /// PTY-driven visible updates often arrive one frame after the user
    /// input that triggered them. Keep low-latency mode armed until the
    /// VT generation reaches this target so shell echo/prompt redraws
    /// can still take the freshest-frame path.
    low_latency_generation_target: AtomicU64,
    /// Typing and paste arrive as short bursts, not isolated edges.
    /// Keep the freshest-frame path enabled briefly across that burst so
    /// follow-on echoed generations don't fall back to the stale-frame
    /// path in the middle of one interactive run.
    low_latency_burst_until: Mutex<Option<Instant>>,
    /// Pixel remainder from high-resolution wheels / trackpads. We only
    /// ask libghostty-vt or alternate-screen apps to scroll whole rows,
    /// so fractional deltas accumulate here instead of turning every
    /// tiny touchpad event into a full-row jump.
    scroll_remainder: Mutex<ScrollRemainder>,
    drag_anchor: Mutex<Option<(u16, u64)>>,
}

unsafe impl Send for RenderSession {}
unsafe impl Sync for RenderSession {}

#[derive(Debug, Default)]
struct ScrollRemainder {
    px: f32,
    alternate_screen: Option<bool>,
}

impl ScrollRemainder {
    fn reset(&mut self) {
        self.px = 0.0;
        self.alternate_screen = None;
    }

    fn rows_for_delta(
        &mut self,
        delta_y_px: f32,
        cell_height_px: f32,
        alternate_screen: bool,
    ) -> isize {
        if self.alternate_screen != Some(alternate_screen) {
            self.px = 0.0;
            self.alternate_screen = Some(alternate_screen);
        }

        self.px += delta_y_px;
        let rows = (self.px / cell_height_px).trunc() as isize;
        if rows != 0 {
            self.px -= rows as f32 * cell_height_px;
        }
        rows
    }
}

/// Keyboard modifiers held at the time of a mouse event.
///
/// We don't import GPUI's `Modifiers` here because `con-ghostty` must
/// stay independent of the UI crate on Windows. The view layer copies
/// the three bits we care about (shift/alt/control) into this struct.
/// `platform` (the Win key / cmd key) is not reported in SGR and not
/// meaningful for xterm shift-bypass semantics, so it's deliberately
/// omitted.
#[derive(Debug, Default, Clone, Copy)]
pub struct MouseEventMods {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
}

impl RenderSession {
    const LOW_LATENCY_BURST_WINDOW: Duration = Duration::from_millis(750);
    const UNCHANGED_FRAME_LOG_THRESHOLD_MS: f64 = 2.0;

    /// Build a renderer + VT parser + ConPTY child shell.
    ///
    /// `wake` is invoked from the ConPTY reader thread after every
    /// chunk of bytes is fed into the VT parser. The view passes a
    /// closure that pokes a GPUI prepaint via `cx.notify()`, so freshly
    /// arrived shell output paints on the next frame instead of waiting
    /// for the next user input event. Without this hook, the prompt
    /// pwsh prints after `Enter` would sit in the grid until something
    /// else woke the view (mouse move, key press, focus change).
    pub fn new<W>(
        width_px: u32,
        height_px: u32,
        dpi: u32,
        config: RendererConfig,
        cwd: Option<PathBuf>,
        initial_output: Option<Vec<u8>>,
        wake: W,
    ) -> Result<Self>
    where
        W: Fn() + Send + Sync + 'static,
    {
        let base_font_size_px = config.font_size_px;
        let current_dpi = if dpi == 0 { 96 } else { dpi };

        let mut renderer_config = config.clone();
        renderer_config.initial_width = width_px.max(1);
        renderer_config.initial_height = height_px.max(1);
        renderer_config.font_size_px = scale_font_size(base_font_size_px, current_dpi);

        log::info!(
            "RenderSession::new size={}x{} dpi={} font_px={:.2}",
            renderer_config.initial_width,
            renderer_config.initial_height,
            current_dpi,
            renderer_config.font_size_px,
        );

        let renderer = Renderer::new(&renderer_config).context("Renderer::new failed")?;
        let (cols, rows) = renderer.grid_for_dimensions(&renderer_config);
        log::info!("RenderSession: grid {cols}x{rows}");

        let vt = Arc::new(
            VtScreen::new(cols, rows, renderer_config.theme.as_ref())
                .context("VtScreen::new failed")?,
        );
        let transcript = Arc::new(Mutex::new(TranscriptBuffer::default()));
        if let Some(output) = initial_output
            .as_deref()
            .filter(|output| !output.is_empty())
        {
            vt.feed(output);
            let text = String::from_utf8_lossy(output);
            transcript.lock().push(text.as_ref());
        }

        let vt_for_pty = vt.clone();
        let transcript_for_pty = transcript.clone();
        let wake_for_pty: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let shell = super::conpty::default_shell_command();
        let shell_cwd = resolve_shell_cwd(cwd);
        log::info!("RenderSession: spawning ConPTY shell={shell} cwd={shell_cwd:?}");
        let conpty = ConPty::spawn(
            &shell,
            shell_cwd.as_deref(),
            PtySize { cols, rows },
            move |bytes| {
                let text = String::from_utf8_lossy(bytes);
                transcript_for_pty.lock().push(text.as_ref());
                vt_for_pty.feed(bytes);
                wake_for_pty();
            },
        )
        .context("ConPty::spawn failed")?;
        let conpty = Arc::new(conpty);

        Ok(Self {
            renderer: Mutex::new(renderer),
            vt,
            transcript,
            conpty,
            shell_cwd,
            config: Mutex::new(renderer_config),
            base_font_size_px,
            dpi: AtomicU32::new(current_dpi),
            low_latency_requested: AtomicBool::new(false),
            low_latency_generation_target: AtomicU64::new(0),
            low_latency_burst_until: Mutex::new(None),
            scroll_remainder: Mutex::new(ScrollRemainder::default()),
            drag_anchor: Mutex::new(None),
        })
    }

    /// Render one frame. `Rendered` returns freshly-read BGRA bytes;
    /// `Unchanged` means "nothing moved, reuse the last image".
    pub fn render_frame(&self) -> Result<RenderOutcome> {
        let prof_started = perf_trace_enabled().then(Instant::now);
        let renderer = self.renderer.lock();
        let config = self.config.lock().clone();
        let snapshot_started = perf_trace_enabled().then(Instant::now);
        let snapshot = self.vt.snapshot();
        let snapshot_ms = snapshot_started
            .map(|started| started.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        let immediate = self.low_latency_requested.swap(false, Ordering::AcqRel);
        let target_generation = self.low_latency_generation_target.load(Ordering::Acquire);
        let generation_ready = target_generation != 0 && snapshot.generation >= target_generation;
        let burst_active = self.burst_low_latency_active();
        let prefer_latest = immediate || generation_ready || burst_active;
        let render_started = perf_trace_enabled().then(Instant::now);
        let outcome = renderer.render(&snapshot, &config, prefer_latest)?;
        let render_ms = render_started
            .map(|started| started.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        if generation_ready && !matches!(outcome, RenderOutcome::Pending) {
            self.low_latency_generation_target
                .store(0, Ordering::Release);
        }
        if let Some(started) = prof_started {
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            let outcome_name = match &outcome {
                RenderOutcome::Rendered { .. } => "rendered",
                RenderOutcome::Pending => "pending",
                RenderOutcome::Unchanged => "unchanged",
            };
            let should_log = perf_trace_verbose()
                || !matches!(outcome, RenderOutcome::Unchanged)
                || prefer_latest
                || total_ms >= Self::UNCHANGED_FRAME_LOG_THRESHOLD_MS;
            if should_log {
                log::info!(
                    target: "con::perf",
                    "win_render_frame generation={} rows={} cols={} prefer_latest={} outcome={} snapshot_ms={:.3} render_ms={:.3} total_ms={:.3}",
                    snapshot.generation,
                    snapshot.rows,
                    snapshot.cols,
                    prefer_latest,
                    outcome_name,
                    snapshot_ms,
                    render_ms,
                    total_ms,
                );
            }
        }
        Ok(outcome)
    }

    /// Apply a new physical-pixel size. Idempotent for same dimensions.
    pub fn resize(&self, width_px: u32, height_px: u32) -> Result<()> {
        if width_px == 0 || height_px == 0 {
            return Ok(());
        }
        let mut renderer = self.renderer.lock();
        renderer
            .resize(width_px, height_px)
            .context("Renderer::resize failed")?;
        let metrics = renderer.metrics();
        let config = self.config.lock();
        let (cols, rows) = renderer.grid_for_dimensions(&config);
        drop(config);
        let cell_w = metrics.cell_width_px.max(1);
        let cell_h = metrics.cell_height_px.max(1);
        drop(renderer);

        self.vt
            .resize(cols, rows, cell_w, cell_h)
            .context("VtScreen::resize failed")?;
        self.conpty
            .resize(PtySize { cols, rows })
            .context("ConPty::resize failed")?;
        log::debug!(
            "RenderSession::resize -> {width_px}x{height_px} grid={cols}x{rows} cell={cell_w}x{cell_h}"
        );
        Ok(())
    }

    /// Live update of the user-visible theme + window opacity.
    ///
    /// `theme` (when present) replaces libghostty's default fg/bg/palette
    /// so SGR colors resolve to the user's palette without restarting
    /// the pane. `background_opacity` is stored on the renderer config
    /// and read on every frame — the renderer rewrites the sentinel
    /// alpha=0 default-bg cells to `opacity*255` and pre-multiplies the
    /// clear color, so the cell grid composites over Mica / DComp at the
    /// requested level. Bumping the VT generation forces the next
    /// prepaint to repaint with the new colors / opacity.
    pub fn set_appearance(&self, theme: Option<&ThemeColors>, background_opacity: f32) {
        let clamped_opacity = background_opacity.clamp(0.0, 1.0);
        let mut config = self.config.lock();
        let opacity_changed = (config.background_opacity - clamped_opacity).abs() > f32::EPSILON;
        config.background_opacity = clamped_opacity;
        if let Some(theme) = theme {
            // Margins (pixels outside the cell grid) paint from
            // `clear_color`, so a theme switch that only rewrites the
            // palette would leave the border showing the previous
            // theme's background. Mirror what `WindowsGhosttyApp::
            // update_appearance` does at session construction.
            config.clear_color = [
                theme.bg[0] as f32 / 255.0,
                theme.bg[1] as f32 / 255.0,
                theme.bg[2] as f32 / 255.0,
                1.0,
            ];
            config.theme = Some(theme.clone());
            // `set_theme` bumps the VT generation itself, so the next
            // prepaint re-runs draw_cells with the new palette + new
            // clear_color + any new opacity.
            self.vt.set_theme(theme);
        } else {
            config.theme = None;
            // An opacity-only change doesn't touch the VT screen, so
            // the renderer's `needs_draw` gate (keyed on
            // snapshot.generation ⨁ selection) would otherwise serve
            // a stale cached frame until the next VT byte arrives.
            // Force a generation bump so the change is visible now.
            if opacity_changed {
                self.vt.bump_generation();
            }
        }
    }

    /// Notify of a DPI change. Rebuilds the glyph atlas at the new
    /// physical font size and re-derives the cell grid. Follow with a
    /// `resize` to match the new physical dimensions.
    pub fn set_dpi(&self, dpi: u32) -> Result<()> {
        let new_dpi = dpi.max(1);
        let prev = self.dpi.swap(new_dpi, Ordering::AcqRel);
        if prev == new_dpi {
            return Ok(());
        }
        let new_font = scale_font_size(self.base_font_size_px, new_dpi);
        let renderer = self.renderer.lock();
        renderer
            .rebuild_atlas(new_font)
            .context("rebuild_atlas on DPI change failed")?;
        let mut config = self.config.lock();
        config.font_size_px = new_font;
        log::info!("RenderSession::set_dpi {prev} -> {new_dpi} font_px={new_font:.2}");
        Ok(())
    }

    /// Current cell metrics (in physical pixels). Used by the GPUI view
    /// to translate mouse coordinates to cell addresses.
    pub fn metrics(&self) -> CellMetrics {
        self.renderer.lock().metrics()
    }

    pub fn vt(&self) -> &Arc<VtScreen> {
        &self.vt
    }

    pub fn is_alive(&self) -> bool {
        self.conpty.is_alive()
    }

    pub fn is_bracketed_paste(&self) -> bool {
        self.vt.is_bracketed_paste()
    }

    pub fn is_decckm(&self) -> bool {
        self.vt.is_decckm()
    }

    pub fn mouse_tracking_active(&self) -> bool {
        self.vt.mouse_tracking_active()
    }

    /// Send UTF-8 text to the child shell. Handles the ConPTY Enter
    /// quirk (shell expects CR, not LF).
    pub fn write_input(&self, text: &str) {
        self.scroll_viewport_to_bottom();
        self.request_low_latency_after_next_generation();
        let bytes: std::borrow::Cow<[u8]> = if text.as_bytes().contains(&b'\n') {
            std::borrow::Cow::Owned(text.replace('\n', "\r").into_bytes())
        } else {
            std::borrow::Cow::Borrowed(text.as_bytes())
        };
        let _ = self.conpty.write(&bytes);
    }

    /// Raw PTY write — no CR/LF normalization. Used for bracketed-paste
    /// wrappers (ESC [200~ / ESC [201~) whose bytes mustn't be touched.
    pub fn write_pty_raw(&self, data: &[u8]) {
        self.scroll_viewport_to_bottom();
        self.request_low_latency_after_next_generation();
        let _ = self.conpty.write(data);
    }

    pub fn clear_screen_and_scrollback(&self) {
        self.vt.clear_screen_and_scrollback();
        self.request_low_latency_present();
    }

    /// Mouse-down at the given cell.
    ///
    /// Xterm convention: Shift bypasses mouse tracking so the user can
    /// always select text, even when a TUI has `set mouse=a` on. When
    /// tracking is off or Shift is held, we drive local selection;
    /// otherwise we emit an SGR button-press report and leave selection
    /// alone. Shift+click with an existing selection extends from the
    /// original anchor (matches every other terminal). `button` follows
    /// the SGR button index (0=Left, 1=Middle, 2=Right). Returns `true`
    /// when the event was consumed by the terminal app (an SGR report
    /// was emitted) — the view uses this to suppress its own context
    /// menu on right-click.
    pub fn mouse_down(&self, button: u8, col: u16, row: u16, mods: MouseEventMods) -> bool {
        if self.vt.mouse_tracking_active() && !mods.shift {
            self.request_low_latency_after_next_generation();
            self.report_sgr_button(button, col, row, mods, true);
            return true;
        }
        if button != 0 {
            // Non-left buttons never drive local selection; when tracking
            // is off the click is simply not consumed by the terminal.
            return false;
        }
        self.request_low_latency_present();
        let point = self.selection_point(col, row);
        if mods.shift {
            let renderer = self.renderer.lock();
            let existing_anchor = renderer.selection().map(|s| s.anchor).unwrap_or(point);
            *self.drag_anchor.lock() = Some(existing_anchor);
            renderer.set_selection(Some(Selection {
                anchor: existing_anchor,
                extent: point,
            }));
            return false;
        }
        *self.drag_anchor.lock() = Some(point);
        self.renderer.lock().set_selection(Some(Selection {
            anchor: point,
            extent: point,
        }));
        false
    }

    /// Mouse-moved at the given cell while a button is held.
    ///
    /// When mouse tracking is active and the shell requested motion
    /// (BUTTON / ANY mode), we emit an SGR motion report with the
    /// motion bit (+32) set. Otherwise we extend the local drag.
    pub fn mouse_drag(&self, col: u16, row: u16, mods: MouseEventMods) {
        if self.vt.mouse_tracking_active() && !mods.shift {
            self.request_low_latency_after_next_generation();
            // Button 0 (LMB) + 32 = motion-with-button bit per SGR spec.
            self.report_sgr_button(32, col, row, mods, true);
            return;
        }
        self.request_low_latency_present();
        let anchor = *self.drag_anchor.lock();
        if let Some(anchor) = anchor {
            self.renderer.lock().set_selection(Some(Selection {
                anchor,
                extent: self.selection_point(col, row),
            }));
        }
    }

    /// Mouse-up at the given cell.
    ///
    /// Emits an SGR release when mouse tracking is active (unless Shift
    /// is held to keep selection). Otherwise clears a transient 1-cell
    /// selection — a click without drag shouldn't leave a lone cell
    /// highlighted. `button` follows the SGR button index (0=Left,
    /// 1=Middle, 2=Right). Returns `true` when the release was consumed
    /// by the terminal app (an SGR report was emitted).
    pub fn mouse_up(&self, button: u8, col: u16, row: u16, mods: MouseEventMods) -> bool {
        if self.vt.mouse_tracking_active() && !mods.shift {
            self.request_low_latency_after_next_generation();
            self.report_sgr_button(button, col, row, mods, false);
            return true;
        }
        if button != 0 {
            return false;
        }
        self.request_low_latency_present();
        let anchor = self.drag_anchor.lock().take();
        if let Some(anchor) = anchor
            && anchor == self.selection_point(col, row)
        {
            self.renderer.lock().set_selection(None);
        }
        false
    }

    fn selection_point(&self, col: u16, row: u16) -> (u16, u64) {
        let viewport_offset = self.vt.scrollbar().map_or(0, |scrollbar| scrollbar.offset);
        (col, viewport_offset.saturating_add(row as u64))
    }

    fn report_sgr_button(
        &self,
        base_button: u8,
        col: u16,
        row: u16,
        mods: MouseEventMods,
        pressed: bool,
    ) {
        let seq = sgr_button_sequence(base_button, col, row, mods, pressed);
        let _ = self.conpty.write(seq.as_bytes());
    }

    /// Cancel any in-flight drag (used on focus loss).
    pub fn cancel_drag(&self) {
        *self.drag_anchor.lock() = None;
    }

    /// SGR mouse-wheel report. Only used when the shell has enabled
    /// mouse tracking — see `mouse_tracking_active`. `col`/`row` are
    /// 1-based cell coordinates per the SGR spec. Alt/Ctrl are encoded
    /// into the button byte; Shift is handled upstream by the view,
    /// which bypasses reporting entirely when Shift is held so the user
    /// can scroll Con's own scrollback without the TUI seeing it.
    pub fn forward_wheel(&self, col: u16, row: u16, delta_y: f32, mods: MouseEventMods) {
        if delta_y.abs() < f32::EPSILON {
            return;
        }
        self.request_low_latency_after_next_generation();
        let mut button = sgr_wheel_button_for_delta(delta_y);
        if mods.alt {
            button |= 0x08;
        }
        if mods.control {
            button |= 0x10;
        }
        let col = col.max(1);
        let row = row.max(1);
        let seq = format!("\x1b[<{button};{col};{row}M");
        let _ = self.conpty.write(seq.as_bytes());
    }

    /// Scroll the terminal viewport when the shell did not request
    /// mouse-wheel events. Mirrors Ghostty's native behavior:
    ///
    /// - alternate screen + alternate-scroll mode sends cursor keys to
    ///   apps such as less/vim that did not enable explicit mouse
    ///   tracking
    /// - otherwise the primary-screen viewport scrolls through
    ///   libghostty-vt's scrollback
    ///
    /// GPUI follows the terminal/editor convention used by Zed: a
    /// positive vertical scroll delta means "scroll up". libghostty-vt's
    /// viewport API uses the opposite sign: negative rows are up.
    pub fn scroll_viewport_or_alt_screen(&self, delta_y_px: f32, allow_alt_screen_keys: bool) {
        if delta_y_px.abs() < f32::EPSILON {
            return;
        }

        let alternate_screen = self.vt.is_alternate_screen();
        let rows = self.scroll_rows_for_delta(delta_y_px, alternate_screen);
        if rows == 0 {
            return;
        }

        if allow_alt_screen_keys && alternate_screen && self.vt.is_alt_scroll() {
            self.send_scroll_as_cursor_keys(rows);
            return;
        }

        if self
            .vt
            .scroll_viewport_delta(viewport_delta_for_scroll_rows(rows))
        {
            self.request_low_latency_present();
        }
    }

    pub fn scrollbar(&self) -> Option<GhosttyScrollbar> {
        self.vt.scrollbar()
    }

    pub fn generation(&self) -> u64 {
        self.vt.generation()
    }

    /// Scroll by `rows` in libghostty-vt's viewport convention.
    ///
    /// `vt.scroll_viewport_delta` treats negative rows as up / older history
    /// and positive rows as down / newer history. GPUI wheel rows use the
    /// opposite sign, so GPUI callers must convert with
    /// `viewport_delta_for_scroll_rows` before calling this method.
    pub fn scroll_viewport_rows(&self, rows: isize) {
        self.scroll_remainder.lock().reset();
        if self.vt.scroll_viewport_delta(rows) {
            self.request_low_latency_present();
        }
    }

    /// Return the viewport to the prompt before writing user input.
    pub fn scroll_viewport_to_bottom(&self) {
        self.scroll_remainder.lock().reset();
        if self.vt.scroll_viewport_bottom() {
            self.request_low_latency_present();
        }
    }

    /// Snap to a scrollbar offset and forward the resulting libghostty-vt
    /// signed delta through `scroll_viewport_rows`.
    ///
    /// Offsets use scrollbar coordinates: `0` is oldest scrollback and
    /// `total - len` is the live tail.
    pub fn scroll_viewport_to_offset(&self, target_offset: u64) {
        let Some(scrollbar) = self.vt.scrollbar() else {
            return;
        };
        let max_offset = scrollbar.total.saturating_sub(scrollbar.len);
        let target = target_offset.min(max_offset);
        let delta = target as i128 - scrollbar.offset.min(max_offset) as i128;
        if delta == 0 {
            return;
        }
        let rows = delta.clamp(isize::MIN as i128, isize::MAX as i128) as isize;
        self.scroll_viewport_rows(rows);
    }

    fn scroll_rows_for_delta(&self, delta_y_px: f32, alternate_screen: bool) -> isize {
        let cell_h = self.metrics().cell_height_px.max(1) as f32;
        self.scroll_remainder
            .lock()
            .rows_for_delta(delta_y_px, cell_h, alternate_screen)
    }

    fn send_scroll_as_cursor_keys(&self, rows: isize) {
        let Some(seq) = cursor_key_for_scroll_rows(rows, self.vt.is_decckm()) else {
            return;
        };
        self.request_low_latency_after_next_generation();
        for _ in 0..rows.unsigned_abs() {
            let _ = self.conpty.write(seq.as_bytes());
        }
    }

    pub fn has_selection(&self) -> bool {
        self.renderer.lock().selection().is_some()
    }

    /// Extract the current selection as text. Returns `None` when
    /// nothing is selected.
    pub fn selection_text(&self) -> Option<String> {
        let selection = self.renderer.lock().selection()?;
        let snapshot = self.vt.snapshot();
        Some(extract_selection_text(&snapshot, selection))
    }

    pub fn read_screen_text(&self, max_lines: usize) -> Vec<String> {
        snapshot_to_lines(&self.vt.snapshot(), max_lines)
    }

    pub fn current_dir(&self) -> Option<String> {
        self.vt.current_dir().or_else(|| {
            self.shell_cwd
                .as_ref()
                .map(|cwd| cwd.to_string_lossy().to_string())
        })
    }

    pub fn read_recent_lines(&self, max_lines: usize) -> Vec<String> {
        self.transcript.lock().recent_lines(max_lines)
    }

    pub fn search_text(&self, pattern: &str, limit: usize) -> Vec<(usize, String)> {
        self.transcript.lock().search(pattern, limit)
    }

    pub fn clear_selection(&self) {
        self.renderer.lock().set_selection(None);
    }

    pub fn dimensions_px(&self) -> (u32, u32) {
        self.renderer.lock().dimensions_px()
    }

    fn request_low_latency_present(&self) {
        self.arm_low_latency_burst();
        self.low_latency_requested.store(true, Ordering::Release);
    }

    fn request_low_latency_after_next_generation(&self) {
        self.arm_low_latency_burst();
        let target = self.vt.generation().wrapping_add(1).max(1);
        self.low_latency_generation_target
            .store(target, Ordering::Release);
    }

    fn arm_low_latency_burst(&self) {
        *self.low_latency_burst_until.lock() =
            Some(Instant::now() + Self::LOW_LATENCY_BURST_WINDOW);
    }

    fn burst_low_latency_active(&self) -> bool {
        let now = Instant::now();
        let mut guard = self.low_latency_burst_until.lock();
        match *guard {
            Some(deadline) if now <= deadline => true,
            Some(_) => {
                *guard = None;
                false
            }
            None => false,
        }
    }
}

fn resolve_shell_cwd(cwd: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(cwd) = cwd {
        if is_valid_shell_cwd(&cwd) {
            return Some(cwd);
        }
        log::warn!("Ignoring invalid ConPTY cwd: {cwd:?}");
    }
    default_shell_cwd()
}

fn default_shell_cwd() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            let mut full = PathBuf::from(drive);
            full.push(Path::new(&path));
            Some(full)
        })
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from));
    home.filter(|path| is_valid_shell_cwd(path)).or_else(|| {
        let tmp = std::env::temp_dir();
        is_valid_shell_cwd(&tmp).then_some(tmp)
    })
}

fn is_valid_shell_cwd(path: &Path) -> bool {
    path.is_absolute() && path.is_dir()
}

fn sgr_wheel_button_for_delta(delta_y: f32) -> u8 {
    if delta_y > 0.0 { 64 } else { 65 }
}

/// Build an SGR (1006) mouse button report escape sequence for the
/// given 0-based cell coordinates. Alt (0x08) and Ctrl (0x10) bits are
/// folded into the button byte per the SGR spec; `pressed` selects the
/// press (`M`) or release (`m`) terminator.
fn sgr_button_sequence(
    base_button: u8,
    col: u16,
    row: u16,
    mods: MouseEventMods,
    pressed: bool,
) -> String {
    let col = col.saturating_add(1);
    let row = row.saturating_add(1);
    let mut cb = base_button;
    if mods.alt {
        cb |= 0x08;
    }
    if mods.control {
        cb |= 0x10;
    }
    let terminator = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{cb};{col};{row}{terminator}")
}

fn cursor_key_for_scroll_rows(rows: isize, decckm: bool) -> Option<&'static str> {
    match rows.cmp(&0) {
        std::cmp::Ordering::Greater => Some(if decckm { "\x1bOA" } else { "\x1b[A" }),
        std::cmp::Ordering::Less => Some(if decckm { "\x1bOB" } else { "\x1b[B" }),
        std::cmp::Ordering::Equal => None,
    }
}

fn viewport_delta_for_scroll_rows(rows: isize) -> isize {
    -rows
}

fn scale_font_size(logical_px: f32, dpi: u32) -> f32 {
    logical_px * (dpi as f32) / 96.0
}

fn extract_selection_text(snapshot: &ScreenSnapshot, sel: Selection) -> String {
    let cols = snapshot.cols;
    if cols == 0 || snapshot.cells.is_empty() {
        return String::new();
    }
    let viewport_offset = snapshot.scrollbar.map_or(0, |scrollbar| scrollbar.offset);
    let mut out = String::new();
    let rows = snapshot.rows;
    for row in 0..rows {
        let mut row_buf = String::new();
        let mut row_has_cell = false;
        let mut last_non_blank: i32 = -1;
        for col in 0..cols {
            if !sel.contains(col, row, cols, viewport_offset) {
                continue;
            }
            row_has_cell = true;
            let idx = row as usize * cols as usize + col as usize;
            let cell = snapshot.cells.get(idx).copied().unwrap_or_default();
            let ch = if cell.codepoint == 0 {
                ' '
            } else {
                char::from_u32(cell.codepoint).unwrap_or(' ')
            };
            row_buf.push(ch);
            if cell.codepoint != 0 && cell.codepoint != 0x20 {
                last_non_blank = row_buf.chars().count() as i32 - 1;
            }
        }
        if !row_has_cell {
            continue;
        }
        if last_non_blank >= 0 {
            let trimmed: String = row_buf.chars().take(last_non_blank as usize + 1).collect();
            out.push_str(&trimmed);
        }
        out.push('\n');
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sgr_wheel_delta_matches_gpui_scroll_direction() {
        assert_eq!(sgr_wheel_button_for_delta(1.0), 64);
        assert_eq!(sgr_wheel_button_for_delta(-1.0), 65);
    }

    #[test]
    fn alt_scroll_rows_match_cursor_direction() {
        assert_eq!(cursor_key_for_scroll_rows(2, false), Some("\x1b[A"));
        assert_eq!(cursor_key_for_scroll_rows(-2, false), Some("\x1b[B"));
        assert_eq!(cursor_key_for_scroll_rows(2, true), Some("\x1bOA"));
        assert_eq!(cursor_key_for_scroll_rows(-2, true), Some("\x1bOB"));
        assert_eq!(cursor_key_for_scroll_rows(0, false), None);
    }

    #[test]
    fn viewport_scroll_inverts_gpui_rows_for_libghostty_vt() {
        assert_eq!(viewport_delta_for_scroll_rows(3), -3);
        assert_eq!(viewport_delta_for_scroll_rows(-3), 3);
        assert_eq!(viewport_delta_for_scroll_rows(0), 0);
    }

    #[test]
    fn sgr_right_button_press_and_release() {
        let mods = MouseEventMods::default();
        assert_eq!(
            sgr_button_sequence(2, 0, 0, mods, true),
            "\x1b[<2;1;1M"
        );
        assert_eq!(
            sgr_button_sequence(2, 5, 9, mods, false),
            "\x1b[<2;6;10m"
        );
    }

    #[test]
    fn sgr_button_sequence_folds_alt_and_ctrl_bits() {
        let mods = MouseEventMods {
            alt: true,
            control: true,
            shift: false,
        };
        // base 0 (left) | alt 0x08 | ctrl 0x10 = 0x18
        assert_eq!(sgr_button_sequence(0, 3, 7, mods, true), "\x1b[<24;4;8M");
    }
}
