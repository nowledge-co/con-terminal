//! D3D11 + DirectWrite renderer for the Windows terminal pane.
//!
//! Structure:
//!
//! ```text
//! Renderer
//!   ├── device (ID3D11Device) + context
//!   ├── rt_texture (ID3D11Texture2D, offscreen B8G8R8A8_UNORM)
//!   ├── rtv (ID3D11RenderTargetView onto rt_texture)
//!   ├── staging (ID3D11Texture2D, USAGE_STAGING | CPU_ACCESS_READ)
//!   ├── dwrite (IDWriteFactory)
//!   ├── atlas (GlyphCache — etagere skyline + Direct2D DrawGlyphRun)
//!   └── pipeline (VS/PS, IA layout, instance + index + cbuffer)
//! ```
//!
//! We render into an offscreen texture instead of an HWND swapchain so
//! the caller can CPU-read the BGRA bytes and hand them to GPUI as an
//! `ImageSource::Render(Arc<RenderImage>)`. GPUI then composes the pane
//! into its own DirectComposition tree — no child HWND on top of the
//! app's modals, no z-order glitches on tab switch.
//!
//! Dirty frames use layered cell and RGBA image passes so Kitty placements
//! can render below backgrounds, below text, or above text. See the HLSL
//! sources and pipeline modules for the D3D11 plumbing.

mod atlas;
mod font_loader;
mod image_pipeline;
mod pipeline;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_RENDER_TARGET_VIEW_DESC, D3D11_RTV_DIMENSION_TEXTURE2D,
    D3D11_SDK_VERSION, D3D11_TEX2D_RTV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIEWPORT, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
    ID3D11RenderTargetView, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_WAS_STILL_DRAWING;

use super::profile::{perf_trace_enabled, perf_trace_verbose};
use super::vt::{ATTR_INVERSE, ATTR_STRIKE, ATTR_UNDERLINE, Cell, KittyPlacement, ScreenSnapshot};
use atlas::{GlyphCache, GlyphKey};
use image_pipeline::{ImageLayer, ImagePipeline};
use pipeline::{Globals, Instance, Pipeline, instance_for_cell};

pub use super::vt::ThemeColors;
pub use atlas::CellMetrics;

const ATLAS_SIZE_PX: u32 = 2048;
const ALTERNATE_SCREEN_BACKGROUND_OPACITY_FLOOR: f32 = 1.0;
/// Initial instance-buffer capacity. 16 384 covers a 200×80 grid
/// without reallocation; panes larger than that grow via
/// `Pipeline::ensure_instance_capacity` in the hot path.
const INITIAL_INSTANCE_CAPACITY: u32 = 16 * 1024;
const RENDER_ATTR_DEFAULT_BG: u32 = 1 << 8;
const RENDER_ATTR_CURSOR: u32 = 1 << 9;

#[derive(Debug, Clone)]
pub struct RendererConfig {
    pub font_family: String,
    pub font_size_px: f32,
    pub initial_width: u32,
    pub initial_height: u32,
    /// RGB clear-target color. Alpha is taken from `background_opacity`
    /// at render time so opacity changes don't have to thread back into
    /// here.
    pub clear_color: [f32; 4],
    /// 0.0 (fully see-through) … 1.0 (opaque). Multiplied into the
    /// clear-color alpha and the per-cell default-bg alpha so that
    /// unstyled cells composite over Mica / DComp visuals beneath.
    /// Cells with explicit SGR backgrounds stay solid.
    pub background_opacity: f32,
    /// Theme handed to libghostty so SGR colors resolve to the user's
    /// palette. `None` keeps libghostty's built-in defaults.
    pub theme: Option<ThemeColors>,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self {
            font_family: font_loader::BUNDLED_FONT_FAMILY.to_string(),
            font_size_px: 14.0,
            initial_width: 800,
            initial_height: 600,
            clear_color: [0.06, 0.06, 0.07, 1.0],
            background_opacity: 1.0,
            theme: None,
        }
    }
}

pub struct Renderer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    rt_texture: ID3D11Texture2D,
    rtv: ID3D11RenderTargetView,
    /// 2-slot staging ring. We CopyResource the freshly drawn rt_texture
    /// into the next ring slot, then in the FOLLOWING `render()` call
    /// drain the OLDEST in-flight slot non-blocking. This trades ~1
    /// frame of latency for an unblocked render thread — the GPU has a
    /// full prepaint cycle (~16ms) to finish the copy before we Map(),
    /// so `D3D11_MAP_FLAG_DO_NOT_WAIT` almost always succeeds first try.
    /// See `StagingRing` for the state machine.
    staging_ring: Mutex<StagingRing>,
    _dwrite: IDWriteFactory,

    pipeline: std::sync::Mutex<Pipeline>,
    image_pipeline: Mutex<ImagePipeline>,
    atlas: Mutex<GlyphCache>,

    instances: Mutex<Vec<Instance>>,
    has_full_frame: Mutex<bool>,
    kitty_readback: Mutex<KittyReadbackState>,
    /// Generation fingerprint of the last frame we actually rendered.
    /// It includes VT generation and snapshot/view geometry so resize
    /// catch-up frames are not mistaken for unchanged content. Seeded
    /// with `u64::MAX` so the very first call — which sees `generation =
    /// 0` on a quiet VT — still produces a frame (the cleared background),
    /// giving the pane something to show before the shell has printed anything.
    last_generation: Mutex<u64>,
    /// Wall-clock time of the last `Rendered` outcome. Drives the
    /// `MIN_FALLBACK_PRESENT_INTERVAL` cap that decouples presentation
    /// rate from the VT update rate, so continuous-output TUIs (issue
    /// #114) don't churn one full-frame image-handoff per prepaint.
    /// `None` until the first frame is presented.
    last_presented_at: Mutex<Option<Instant>>,

    width_px: u32,
    height_px: u32,
}

/// Minimum wall-clock spacing between two `Rendered` outcomes from the
/// continuous-output fallback path. Set to one 60 Hz vsync interval —
/// presenting faster than this is wasted work because GPUI only paints
/// once per refresh, and each present pays a full-frame Map + memcpy
/// + `bgra_frame_to_image` + sprite-atlas upload.
///
/// Short bursts (`ls`, `dir`, `clear`) finish well under one interval,
/// so the fallback never fires for them and the established mailbox
/// "snap to final" behavior is preserved exactly. Sustained TUIs
/// (codex, watch, top) hit the cap and update at 60 Hz instead of
/// freezing. User-driven paths (mouse / key / paste / generation
/// target) bypass the cap via the `block_drain` branch.
const MIN_FALLBACK_PRESENT_INTERVAL: Duration = Duration::from_millis(16);

/// Freshly rendered BGRA patch. Coordinates and sizes are physical pixels.
pub struct FramePatchBgra {
    pub bytes: Vec<u8>,
    pub y: u32,
    pub height: u32,
}

/// Freshly rendered BGRA frame patches. Width/height are physical pixels.
pub struct FrameBgra {
    pub width: u32,
    pub height: u32,
    pub patches: Vec<FramePatchBgra>,
}

/// Result of [`Renderer::render`].
pub enum RenderOutcome {
    /// No change since the previous call — reuse the prior image.
    Unchanged,
    /// Fresh BGRA bytes, ready to hand to GPUI as an `ImageSource`.
    Rendered {
        frame: FrameBgra,
        /// A newer copy was submitted while this frame was drained from
        /// an older staging slot. The caller should publish this frame
        /// now, then schedule one more prepaint so the fresher in-flight
        /// copy can be drained even if VT output stops immediately after
        /// the fallback present.
        needs_followup_prepaint: bool,
    },
    /// GPU work was submitted but the staging ring has nothing older
    /// ready to drain without waiting. Caller should keep the previous
    /// image and schedule another prepaint unless this frame was marked
    /// latency-critical.
    Pending,
}

unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    pub fn new(config: &RendererConfig) -> Result<Self> {
        log::info!("Renderer: creating D3D11 device");
        let (device, context) = create_device()?;
        log::info!("Renderer: D3D11 device created");

        let width = config.initial_width.max(1);
        let height = config.initial_height.max(1);

        log::info!("Renderer: creating offscreen render-target texture {width}x{height}");
        let (rt_texture, rtv) = create_rt_texture(&device, width, height)?;
        log::info!("Renderer: RT texture + RTV created");

        let staging_ring = StagingRing::new(&device, width, height)?;
        log::info!(
            "Renderer: staging ring ({} slots) created",
            StagingRing::DEPTH
        );

        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
            .context("DWriteCreateFactory failed")?;
        log::info!("Renderer: DWrite factory created");

        let bundled_collection =
            font_loader::build_bundled_collection(&dwrite).unwrap_or_else(|err| {
                log::warn!("font bundling failed; using system fallback: {err:#}");
                None
            });

        log::info!(
            "Renderer: creating GlyphCache (font={})",
            config.font_family
        );
        let atlas = GlyphCache::new(
            &device,
            &context,
            &dwrite,
            bundled_collection,
            &config.font_family,
            config.font_size_px,
            ATLAS_SIZE_PX,
        )
        .context("GlyphCache::new failed")?;
        log::info!("Renderer: GlyphCache created");

        log::info!("Renderer: creating D3D11 pipeline (HLSL compile)");
        let pipeline =
            Pipeline::new(&device, INITIAL_INSTANCE_CAPACITY).context("Pipeline::new failed")?;
        let image_pipeline = ImagePipeline::new(&device).context("ImagePipeline::new failed")?;
        log::info!("Renderer: pipeline ready");

        Ok(Self {
            device,
            context,
            rt_texture,
            rtv,
            staging_ring: Mutex::new(staging_ring),
            _dwrite: dwrite,
            pipeline: std::sync::Mutex::new(pipeline),
            image_pipeline: Mutex::new(image_pipeline),
            atlas: Mutex::new(atlas),
            instances: Mutex::new(Vec::with_capacity(INITIAL_INSTANCE_CAPACITY as usize)),
            has_full_frame: Mutex::new(false),
            kitty_readback: Mutex::new(KittyReadbackState::default()),
            last_generation: Mutex::new(u64::MAX),
            last_presented_at: Mutex::new(None),
            width_px: width,
            height_px: height,
        })
    }

    pub fn resize(&mut self, width_px: u32, height_px: u32) -> Result<()> {
        if width_px == 0 || height_px == 0 {
            return Ok(());
        }
        if width_px == self.width_px && height_px == self.height_px {
            return Ok(());
        }

        // Build the new RT first, but defer the swap. If the staging
        // ring recreate fails after we've already replaced rt_texture /
        // rtv, render() would CopyResource between mismatched-size
        // textures (UB in D3D11). Recreate the ring while the old RT
        // is still active, then commit both in one go.
        let (rt_texture, rtv) = create_rt_texture(&self.device, width_px, height_px)?;
        self.staging_ring
            .lock()
            .expect("staging_ring mutex poisoned in resize()")
            .recreate(&self.device, width_px, height_px)?;
        self.rt_texture = rt_texture;
        self.rtv = rtv;
        self.width_px = width_px;
        self.height_px = height_px;
        *self
            .last_generation
            .lock()
            .expect("last_generation mutex poisoned in resize()") = u64::MAX;
        *self
            .has_full_frame
            .lock()
            .expect("has_full_frame mutex poisoned in resize()") = false;
        Ok(())
    }

    pub fn metrics(&self) -> CellMetrics {
        self.atlas
            .lock()
            .expect("atlas mutex poisoned in metrics()")
            .metrics()
    }

    pub fn grid_for_dimensions(&self, _config: &RendererConfig) -> (u16, u16) {
        let m = self.metrics();
        let cols = (self.width_px / m.cell_width_px.max(1)).max(1) as u16;
        let rows = (self.height_px / m.cell_height_px.max(1)).max(1) as u16;
        (cols, rows)
    }

    pub fn dimensions_px(&self) -> (u32, u32) {
        (self.width_px, self.height_px)
    }

    pub fn rebuild_atlas(&self, font_size_px: f32) -> Result<()> {
        self.atlas
            .lock()
            .expect("atlas mutex poisoned in rebuild_atlas()")
            .rebuild(font_size_px)?;
        *self
            .last_generation
            .lock()
            .expect("last_generation mutex poisoned in rebuild_atlas()") = u64::MAX;
        *self
            .has_full_frame
            .lock()
            .expect("has_full_frame mutex poisoned in rebuild_atlas()") = false;
        Ok(())
    }

    /// Render one frame and return BGRA bytes (full or row-patched)
    /// for the caller to publish through GPUI.
    ///
    /// # State machine
    ///
    /// Inputs read each call:
    /// * `snapshot.generation`, viewport geometry —
    ///   combined into a fingerprint that decides `needs_draw`.
    /// * Staging ring's `oldest_in_flight()` — the GPU copy queued by
    ///   a prior call that may now have completed.
    /// * `prefer_latest` — set by `host_view` when the user is
    ///   actively waiting on a specific frame (mouse / key / paste /
    ///   `low_latency_generation_target`).
    ///
    /// Outputs:
    /// * `Rendered { frame, needs_followup_prepaint }` — fresh BGRA
    ///   bytes; caller publishes. The follow-up flag is set only when
    ///   an older drained slot was presented while a fresher submitted
    ///   copy remains in flight.
    /// * `Pending` — GPU work was submitted (or is still in flight)
    ///   but no frame is presentable on this call. Caller must
    ///   schedule another prepaint (`cx.notify()`).
    /// * `Unchanged` — nothing to draw and nothing in flight; caller
    ///   keeps the previous image.
    ///
    /// Decisions, in priority order:
    ///
    /// 1. **User-driven fast path.** When `prefer_latest && needs_draw`
    ///    and `can_block_for_latest` is satisfied, `block_drain` waits
    ///    for the freshly submitted slot and returns `Rendered` with
    ///    the freshest possible bytes. This is what makes key echo,
    ///    paste, and `low_latency_generation_target` deterministic.
    ///
    /// 2. **Continuous-output fallback.** When `needs_draw` is true
    ///    but the user is not waiting on a target, present the prior
    ///    submit's already-completed readback (gated on
    ///    `presentation_due`). Without this, TUIs that never let the
    ///    VT quiesce (codex spinner, watch, top, btop) leave
    ///    `needs_draw=true` on every prepaint, the just-submitted
    ///    copy never gets a chance to drain, and the on-screen image
    ///    freezes until a click flips `prefer_latest=true` (issue
    ///    #114). Capping the fallback at `MIN_FALLBACK_PRESENT_INTERVAL`
    ///    preserves the established mailbox snap-to-final behavior
    ///    for short bursts (`ls`, `dir`, `clear`) — they finish under
    ///    one cap interval, so the fallback never fires for them.
    ///
    /// 3. **Quiet-VT catch-up.** When `!needs_draw` and a prior
    ///    submit's readback is ready, present it. This is the path
    ///    that lands the final frame after a burst.
    ///
    /// 4. **Pure pending.** When something is still in flight and
    ///    nothing else is presentable, return `Pending` so the caller
    ///    schedules another prepaint.
    ///
    /// 5. **Idle.** Nothing to do.
    ///
    /// # Mailbox semantics
    ///
    /// The ring is a mailbox at the *submit* level: when both slots
    /// are in flight, a fresh submit reuses the oldest, evicting its
    /// pending readback. The drain side prefers the oldest in-flight
    /// slot because it is the most likely to be GPU-ready (it has
    /// had the longest time to complete). The discard step that
    /// drops the oldest in-flight readback before draining only runs
    /// when `!needs_draw`; under continuous output the oldest slot
    /// is the only ready one, and discarding it would force the
    /// drain onto the in-flight newer slot and re-create the freeze.
    pub fn render(
        &self,
        snapshot: &ScreenSnapshot,
        config: &RendererConfig,
        prefer_latest: bool,
    ) -> Result<RenderOutcome> {
        let prof_started = perf_trace_enabled().then(Instant::now);
        let geometry_hash =
            geometry_fingerprint(snapshot.cols, snapshot.rows, self.width_px, self.height_px);
        let combined = snapshot
            .generation
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(geometry_hash.rotate_left(13));
        let needs_draw = *self
            .last_generation
            .lock()
            .expect("last_generation mutex poisoned in render()")
            != combined;

        let mut ring = self
            .staging_ring
            .lock()
            .expect("staging_ring mutex poisoned in render()");

        let in_flight_before_submit = ring.in_flight_count();

        // A completed readback from an earlier submit is the freshest
        // pixels we can currently put on screen. If `needs_draw` is
        // false, draining is the only way to advance the displayed
        // image at all. If `needs_draw` is true, the just-submitted
        // copy is even fresher but isn't ready yet — and refusing to
        // drain the older slot here strands the on-screen image on
        // whatever frame was last presented. With continuous-output
        // TUIs (codex spinner / streaming, watch, top, btop), every
        // prepaint sees a new VT generation, so `needs_draw` stays
        // true forever and the screen freezes until a click flips
        // `prefer_latest=true`. Drain in both cases; the non-blocking
        // `try_drain` is cheap, and presenting one-frame-old pixels is
        // strictly better than freezing.
        //
        // Discarding the oldest in-flight slot stays gated on
        // `!needs_draw`. Under `needs_draw` the oldest slot is the
        // most likely to be GPU-ready, and discarding it would force
        // the drain onto the newer (less likely ready) slot — on
        // GPUs where copies take more than one prepaint that re-
        // creates the freeze this fix is meant to remove.
        if !needs_draw && in_flight_before_submit > 1 {
            ring.discard_oldest_in_flight();
        }
        let drain_target = ring.oldest_in_flight();

        let drain_started = perf_trace_enabled().then(Instant::now);
        let drained: Option<Readback> = if let Some(idx) = drain_target {
            ring.try_drain(&self.context, idx)?
        } else {
            None
        };
        let drain_ms = drain_started
            .map(|started| started.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);
        let backlog = drain_target.is_some() && drained.is_none();

        let mut submitted: Option<SubmittedCopy> = None;
        let mut draw_ms = 0.0;
        let mut submit_ms = 0.0;
        if needs_draw {
            let draw_started = perf_trace_enabled().then(Instant::now);
            let vp = D3D11_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: self.width_px as f32,
                Height: self.height_px as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            // Multiply alpha by background_opacity so transparent cells
            // composite with the backdrop (Mica / DComp). Pre-multiply
            // RGB so the BGRA readback handed to GPUI behaves correctly
            // under GPUI's premultiplied-alpha blend (otherwise
            // translucent pixels look washed out / haloed against the
            // visual beneath).
            let opacity = effective_background_opacity(snapshot, config);
            let clear = [
                config.clear_color[0] * opacity,
                config.clear_color[1] * opacity,
                config.clear_color[2] * opacity,
                config.clear_color[3] * opacity,
            ];
            unsafe {
                self.context.RSSetViewports(Some(&[vp]));
                self.context
                    .OMSetRenderTargets(Some(&[Some(self.rtv.clone())]), None);
                self.context.ClearRenderTargetView(&self.rtv, &clear);
            }

            let cell_metrics = self.draw_terminal(snapshot, config)?;
            draw_ms = draw_started
                .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0);

            // Submit GPU CopyResource into the ring's next slot. The
            // command joins the same context queue as the draws above,
            // so by the next prepaint the GPU will have run them all
            // and the staging texture will be ready to Map().
            let submit_started = perf_trace_enabled().then(Instant::now);
            // Image damage is relative to the last frame handed to GPUI.
            // Partial patches are safe only when no older mailbox slot can be
            // presented first and change that baseline.
            let can_partial_readback = in_flight_before_submit == 0
                && *self
                    .has_full_frame
                    .lock()
                    .expect("has_full_frame mutex poisoned checking partial readback");
            let kitty_visuals = KittyVisualState::from_placements(
                &snapshot.kitty_placements,
                [cell_metrics.cell_width_px, cell_metrics.cell_height_px],
            );
            let readback_regions =
                self.readback_regions(snapshot, can_partial_readback, &kitty_visuals);
            submitted = Some(ring.submit_copy_mailbox(
                &self.context,
                &self.rt_texture,
                &readback_regions,
                kitty_visuals,
            ));
            *self
                .last_generation
                .lock()
                .expect("last_generation mutex poisoned after submit") = combined;
            submit_ms = submit_started
                .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
        }

        let mut block_drain_ms = 0.0;
        if prefer_latest
            && needs_draw
            && can_block_for_latest(in_flight_before_submit, backlog, submitted)
            && let Some(submitted) = submitted
            && !submitted.replaced_in_flight
            && let Some(frame) = {
                let block_started = perf_trace_enabled().then(Instant::now);
                let readback = ring.block_drain(&self.context, submitted.idx)?;
                block_drain_ms = block_started
                    .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                    .unwrap_or(0.0);
                readback.map(|readback| self.frame_from_readback(readback))
            }
        {
            let outcome = RenderOutcome::Rendered {
                frame,
                needs_followup_prepaint: false,
            };
            self.mark_presented();
            log_render_profile(
                prof_started,
                snapshot,
                prefer_latest,
                needs_draw,
                in_flight_before_submit,
                drain_target,
                drained.is_some(),
                backlog,
                Some(submitted),
                draw_ms,
                submit_ms,
                drain_ms,
                block_drain_ms,
                "rendered",
                "blocked_latest",
            );
            return Ok(outcome);
        }

        if needs_draw {
            // Continuous-output fallback: if a prior submit's readback
            // is ready right now AND we haven't presented within the
            // last `MIN_FALLBACK_PRESENT_INTERVAL`, present it instead
            // of stalling on `Pending`. Without this, TUIs that never
            // let the VT quiesce (codex, watch, top) leave
            // `needs_draw=true` on every prepaint, the just-submitted
            // copy never gets a chance to drain, and the on-screen
            // image freezes until a click flips `prefer_latest=true`
            // (issue #114). The freshly submitted frame is still in
            // flight and will be picked up by the next prepaint, so
            // we lag the VT by at most one frame instead of
            // indefinitely.
            //
            // Two gates keep this from regressing the perf work in
            // postmortem 2026-04-26-windows-command-render-latency:
            //
            // - `!prefer_latest`: the user is waiting on a specific
            //   fresh frame (mouse/key input, paste echo, low-latency
            //   generation target). The `block_drain` branch above
            //   already tried; if it didn't fire (backlog or replaced
            //   slot), returning a stale readback would prematurely
            //   satisfy `host_view`'s low-latency target tracking and
            //   clear the target generation without ever presenting
            //   the targeted frame.
            //
            // - `presentation_due()`: caps presentation rate to one
            //   60 Hz vsync. Short bursts (`ls`, `dir`, `clear`)
            //   finish under one interval, so the fallback never
            //   fires for them and the established mailbox snap-to-
            //   final behavior is preserved exactly. Sustained TUIs
            //   hit the cap and update at 60 Hz with bounded image-
            //   handoff cost.
            if !prefer_latest
                && self.presentation_due()
                && let Some(readback) = drained
            {
                let outcome = RenderOutcome::Rendered {
                    frame: self.frame_from_readback(readback),
                    needs_followup_prepaint: submitted.is_some(),
                };
                self.mark_presented();
                log_render_profile(
                    prof_started,
                    snapshot,
                    prefer_latest,
                    needs_draw,
                    in_flight_before_submit,
                    drain_target,
                    true,
                    backlog,
                    submitted,
                    draw_ms,
                    submit_ms,
                    drain_ms,
                    block_drain_ms,
                    "rendered",
                    "drained_during_submit",
                );
                return Ok(outcome);
            }

            // No prior readback ready yet — wait for the just-submitted
            // copy. The next prepaint (driven by the `Pending`
            // `cx.notify()` in `GhosttyView::render`) will pick it up.
            let outcome = RenderOutcome::Pending;
            log_render_profile(
                prof_started,
                snapshot,
                prefer_latest,
                needs_draw,
                in_flight_before_submit,
                drain_target,
                drained.is_some(),
                backlog,
                submitted,
                draw_ms,
                submit_ms,
                drain_ms,
                block_drain_ms,
                "pending",
                "fresh_submitted_waiting",
            );
            return Ok(outcome);
        }

        if let Some(readback) = drained {
            let outcome = RenderOutcome::Rendered {
                frame: self.frame_from_readback(readback),
                needs_followup_prepaint: false,
            };
            self.mark_presented();
            log_render_profile(
                prof_started,
                snapshot,
                prefer_latest,
                needs_draw,
                in_flight_before_submit,
                drain_target,
                true,
                backlog,
                submitted,
                draw_ms,
                submit_ms,
                drain_ms,
                block_drain_ms,
                "rendered",
                "drained_oldest",
            );
            return Ok(outcome);
        }

        // No new draw and nothing was ready yet, but there is still GPU
        // work outstanding. Ask the caller for another prepaint instead
        // of blocking the UI thread on Map().
        if ring.oldest_in_flight().is_some() {
            let outcome = RenderOutcome::Pending;
            log_render_profile(
                prof_started,
                snapshot,
                prefer_latest,
                needs_draw,
                in_flight_before_submit,
                drain_target,
                false,
                backlog,
                submitted,
                draw_ms,
                submit_ms,
                drain_ms,
                block_drain_ms,
                "pending",
                "waiting_in_flight",
            );
            return Ok(outcome);
        }

        let outcome = RenderOutcome::Unchanged;
        log_render_profile(
            prof_started,
            snapshot,
            prefer_latest,
            needs_draw,
            in_flight_before_submit,
            drain_target,
            false,
            backlog,
            submitted,
            draw_ms,
            submit_ms,
            drain_ms,
            block_drain_ms,
            "unchanged",
            "unchanged",
        );
        Ok(outcome)
    }

    fn readback_regions(
        &self,
        snapshot: &ScreenSnapshot,
        allow_partial: bool,
        kitty_visuals: &KittyVisualState,
    ) -> Vec<ReadbackRegion> {
        if !allow_partial
            || snapshot.dirty_rows.is_empty()
            || snapshot.dirty_rows.len() >= snapshot.rows as usize
        {
            return vec![ReadbackRegion::full(self.height_px)];
        }

        let cell_h = kitty_visuals.cell_size[1].max(1);
        let mut regions = self
            .kitty_readback
            .lock()
            .expect("kitty_readback mutex poisoned")
            .damage_regions(kitty_visuals, self.height_px);
        for row in snapshot.dirty_rows.iter().copied() {
            let y = u32::from(row).saturating_mul(cell_h).min(self.height_px);
            let bottom = y.saturating_add(cell_h).min(self.height_px);
            if y >= bottom {
                continue;
            }
            regions.push(ReadbackRegion {
                y,
                height: bottom - y,
            });
        }
        merge_readback_regions(&mut regions, self.height_px);
        if regions.is_empty() {
            vec![ReadbackRegion::full(self.height_px)]
        } else {
            regions
        }
    }

    /// Returns true when the continuous-output fallback in `render`
    /// is allowed to publish a fresh image. Always true on the very
    /// first frame (no prior present) and otherwise gated on
    /// `MIN_FALLBACK_PRESENT_INTERVAL` since the last `Rendered`
    /// outcome.
    fn presentation_due(&self) -> bool {
        let last = self
            .last_presented_at
            .lock()
            .expect("last_presented_at mutex poisoned in presentation_due()");
        match *last {
            None => true,
            Some(t) => t.elapsed() >= MIN_FALLBACK_PRESENT_INTERVAL,
        }
    }

    /// Stamp the wall-clock time of a fresh present. Called from every
    /// `Rendered` exit point in `render` so `presentation_due` reflects
    /// the true on-screen update cadence — including user-driven
    /// `block_drain` presents — rather than just fallback presents.
    fn mark_presented(&self) {
        *self
            .last_presented_at
            .lock()
            .expect("last_presented_at mutex poisoned in mark_presented()") = Some(Instant::now());
    }

    fn frame_from_readback(&self, mut readback: Readback) -> FrameBgra {
        self.kitty_readback
            .lock()
            .expect("kitty_readback mutex poisoned in frame_from_readback()")
            .presented(std::mem::take(&mut readback.kitty_visuals));
        if readback.regions.len() == 1 && readback.regions[0].is_full(self.height_px) {
            *self
                .has_full_frame
                .lock()
                .expect("has_full_frame mutex poisoned in frame_from_readback()") = true;
            return FrameBgra {
                width: self.width_px,
                height: self.height_px,
                patches: vec![FramePatchBgra {
                    bytes: readback.bytes,
                    y: 0,
                    height: self.height_px,
                }],
            };
        }

        let row_bytes = self.width_px as usize * 4;
        let mut src_offset = 0usize;
        let mut patches = Vec::with_capacity(readback.regions.len());
        for region in readback.regions {
            let top = region.y.min(self.height_px);
            let bottom = region.y.saturating_add(region.height).min(self.height_px);
            let height = bottom.saturating_sub(top);
            let bytes_len = height as usize * row_bytes;
            if bytes_len == 0 || src_offset.saturating_add(bytes_len) > readback.bytes.len() {
                continue;
            }

            let bytes = if src_offset == 0 && bytes_len == readback.bytes.len() {
                std::mem::take(&mut readback.bytes)
            } else {
                readback.bytes[src_offset..src_offset + bytes_len].to_vec()
            };
            patches.push(FramePatchBgra {
                bytes,
                y: top,
                height,
            });
            src_offset += bytes_len;
        }

        FrameBgra {
            width: self.width_px,
            height: self.height_px,
            patches,
        }
    }

    fn draw_terminal(
        &self,
        snapshot: &ScreenSnapshot,
        config: &RendererConfig,
    ) -> Result<CellMetrics> {
        // Sentinel alpha=0 in cell.bg means "default theme background"
        // (set by the VT layer); rewrite it to the configured opacity
        // so the shader composes the cell over Mica with the right
        // see-through level. Cells with explicit SGR backgrounds carry
        // alpha=0xFF and aren't touched.
        //
        // Invariant: any non-sentinel bg value MUST carry alpha=0xFF.
        // Producers in `vt.rs::read_cell` enforce this — explicit RGB
        // and palette colors hard-set the alpha byte. The benign edge
        // case is `background_opacity == 0.0`: opacity_byte is also 0,
        // so the rewrite is a no-op (still fully transparent default
        // bg) and there is no observable ambiguity. If a future code
        // path injects an `0x......00` value into cell.bg without
        // meaning the sentinel, it would be silently rewritten — keep
        // the producers honest.
        let background_opacity = effective_background_opacity(snapshot, config);
        let opacity_byte: u8 = (background_opacity * 255.0).round() as u8;
        let apply_opacity = |bg: u32| -> u32 {
            if (bg & 0xFF) == 0 {
                (bg & 0xFFFFFF00) | opacity_byte as u32
            } else {
                bg
            }
        };

        let mut atlas = self
            .atlas
            .lock()
            .expect("atlas mutex poisoned in draw_terminal() glyph pass");
        let mut instances = self
            .instances
            .lock()
            .expect("instances mutex poisoned in draw_terminal()");
        instances.clear();
        instances.reserve(snapshot.cells.len().saturating_add(1));
        let cell_w_px = atlas.metrics().cell_width_px;

        let cursor_pos = if snapshot.cursor.visible {
            Some((snapshot.cursor.col, snapshot.cursor.row))
        } else {
            None
        };
        let mut has_wide_glyph = false;

        for (i, cell) in snapshot.cells.iter().enumerate() {
            let col = (i % snapshot.cols as usize) as u16;
            let row = (i / snapshot.cols as usize) as u16;

            let in_sel = snapshot
                .selection_ranges
                .get(row as usize)
                .copied()
                .flatten()
                .is_some_and(|range| range.contains(col));
            let effective_attrs = if in_sel {
                cell.attrs ^ 0x10
            } else {
                cell.attrs
            };

            let is_cursor_cell = cursor_pos == Some((col, row));
            let render_attrs = effective_attrs as u32
                | if (cell.bg & 0xFF) == 0 {
                    RENDER_ATTR_DEFAULT_BG
                } else {
                    0
                }
                | if is_cursor_cell {
                    RENDER_ATTR_CURSOR
                } else {
                    0
                };

            // The render target clear already paints the pane's default
            // background. Avoid emitting a bg-only instance for blank
            // cells when it would be pixel-identical to the clear. This
            // preserves explicit SGR backgrounds, selection / inverse,
            // and underline / strike decorations on spaces.
            if !is_cursor_cell
                && is_default_blank_cell(cell, effective_attrs, config, background_opacity)
            {
                continue;
            }

            if cell.codepoint == 0 || cell.codepoint == 0x20 {
                let instance = Instance {
                    cell_pos: [col as u32, row as u32],
                    atlas_pos: [0, 0],
                    atlas_size: [0, 0],
                    fg: cell.fg,
                    bg: apply_opacity(cell.bg),
                    attrs: render_attrs,
                };
                instances.push(instance);
                continue;
            }

            let key = GlyphKey {
                codepoint: cell.codepoint,
                bold: (cell.attrs & 1) != 0,
                italic: (cell.attrs & 2) != 0,
            };
            let glyph = match atlas.get_or_rasterize(key) {
                Some(g) => g,
                None => {
                    log::debug!(
                        "atlas overflow at cell ({col},{row}) U+{:04X}; purging and retrying",
                        cell.codepoint
                    );
                    atlas.purge();
                    match atlas.get_or_rasterize(key) {
                        Some(g) => g,
                        None => {
                            log::warn!(
                                "glyph larger than atlas capacity at U+{:04X}; skipping",
                                cell.codepoint
                            );
                            let instance = Instance {
                                cell_pos: [col as u32, row as u32],
                                atlas_pos: [0, 0],
                                atlas_size: [0, 0],
                                fg: cell.fg,
                                bg: apply_opacity(cell.bg),
                                attrs: render_attrs,
                            };
                            instances.push(instance);
                            continue;
                        }
                    }
                }
            };

            let instance = instance_for_cell(
                col,
                row,
                glyph,
                cell.fg,
                apply_opacity(cell.bg),
                render_attrs,
            );
            has_wide_glyph |= glyph.w as u32 > cell_w_px;
            instances.push(instance);
        }

        // Sort so oversized PUA icons render LAST within the grid
        // pass. Their atlas slots are wider than a cell, so their
        // quad overflows into the neighbour column. DX11 guarantees
        // in-order per-pixel writes within a single draw call, so
        // placing wide instances at the end of the buffer makes
        // their pixels win over the narrow bg-only instance that
        // would otherwise paint on top. Stable partition keeps the
        // grid's row-major order within the narrow and wide groups
        // independently — bad ordering would show up as flicker.
        if has_wide_glyph || cursor_pos.is_some() {
            instances.sort_by_key(|instance| {
                if instance.attrs & RENDER_ATTR_CURSOR != 0 {
                    2
                } else if instance.atlas_size[0] > cell_w_px {
                    1
                } else {
                    0
                }
            });
        }

        let text_instance_count = instances.len() as u32;
        let cursor_instance = instances
            .iter()
            .position(|instance| instance.attrs & RENDER_ATTR_CURSOR != 0)
            .map(|index| index as u32);
        // Background instances share the same GPU buffer but occupy a compact
        // suffix. This avoids rasterizing every ordinary glyph cell through a
        // discard-only background pass while retaining a single map/upload.
        instances.reserve(text_instance_count as usize);
        for index in 0..text_instance_count as usize {
            let instance = instances[index];
            if instance.attrs & RENDER_ATTR_CURSOR == 0
                && (instance.attrs & RENDER_ATTR_DEFAULT_BG == 0
                    || instance.attrs & ATTR_INVERSE as u32 != 0)
            {
                instances.push(instance);
            }
        }
        let background_instance_count = instances.len() as u32 - text_instance_count;

        let metrics = atlas.metrics();
        let atlas_size = atlas.atlas_size() as f32;
        let atlas_srv = atlas.atlas_srv().clone();
        drop(atlas);

        let mut pipeline = self.pipeline.lock().expect("pipeline mutex poisoned");
        let needed = instances.len() as u32;
        if needed > pipeline.instance_capacity {
            let new_capacity = (needed + needed / 2).max(INITIAL_INSTANCE_CAPACITY);
            log::debug!(
                "growing instance buffer: {} -> {} (need {})",
                pipeline.instance_capacity,
                new_capacity,
                needed
            );
            pipeline
                .ensure_instance_capacity(&self.device, new_capacity)
                .context("ensure_instance_capacity failed")?;
        }

        pipeline
            .upload_instances(&self.context, &instances)
            .context("upload_instances failed")?;

        let inv_viewport = [
            2.0 / self.width_px.max(1) as f32,
            -2.0 / self.height_px.max(1) as f32,
        ];
        let globals = Globals {
            inv_viewport,
            cell_size: [metrics.cell_width_px as f32, metrics.cell_height_px as f32],
            grid_cols: snapshot.cols as u32,
            grid_rows: snapshot.rows as u32,
            inv_atlas_size: [1.0 / atlas_size, 1.0 / atlas_size],
        };
        pipeline
            .upload_globals(&self.context, &globals)
            .context("upload_globals failed")?;

        drop(instances);

        let mut image_pipeline = self
            .image_pipeline
            .lock()
            .expect("image_pipeline mutex poisoned");
        image_pipeline.prepare(
            &self.device,
            &self.context,
            &snapshot.kitty_placements,
            [metrics.cell_width_px, metrics.cell_height_px],
            [self.width_px, self.height_px],
        )?;

        image_pipeline.draw_layer(&self.context, ImageLayer::BelowBackground);
        pipeline.draw_backgrounds(
            &self.context,
            text_instance_count,
            background_instance_count,
        );
        image_pipeline.draw_layer(&self.context, ImageLayer::BelowText);
        pipeline.draw_cursor(&self.context, cursor_instance);
        pipeline.draw_text(&self.context, &atlas_srv, text_instance_count);
        image_pipeline.draw_layer(&self.context, ImageLayer::AboveText);
        drop(image_pipeline);
        drop(pipeline);
        Ok(metrics)
    }
}

#[allow(clippy::too_many_arguments)]
fn log_render_profile(
    started: Option<Instant>,
    snapshot: &ScreenSnapshot,
    prefer_latest: bool,
    needs_draw: bool,
    in_flight_before_submit: usize,
    drain_target: Option<usize>,
    drained_ready: bool,
    backlog: bool,
    submitted: Option<SubmittedCopy>,
    draw_ms: f64,
    submit_ms: f64,
    drain_ms: f64,
    block_drain_ms: f64,
    outcome: &str,
    outcome_source: &str,
) {
    let Some(started) = started else {
        return;
    };
    let total_ms = started.elapsed().as_secs_f64() * 1000.0;
    let should_log = perf_trace_verbose()
        || outcome != "unchanged"
        || needs_draw
        || prefer_latest
        || in_flight_before_submit > 0
        || backlog
        || total_ms >= 2.0;
    if !should_log {
        return;
    }
    log::info!(
        target: "con::perf",
        "win_renderer generation={} rows={} cols={} prefer_latest={} needs_draw={} in_flight_before={} drain_target={:?} drained_ready={} backlog={} submitted_idx={:?} replaced_in_flight={} readback_regions={} readback_rows={} draw_ms={:.3} submit_ms={:.3} drain_ms={:.3} block_drain_ms={:.3} outcome={} source={} total_ms={:.3}",
        snapshot.generation,
        snapshot.rows,
        snapshot.cols,
        prefer_latest,
        needs_draw,
        in_flight_before_submit,
        drain_target,
        drained_ready,
        backlog,
        submitted.map(|s| s.idx),
        submitted.is_some_and(|s| s.replaced_in_flight),
        submitted.map(|s| s.readback_regions).unwrap_or(0),
        submitted.map(|s| s.readback_rows).unwrap_or(0),
        draw_ms,
        submit_ms,
        drain_ms,
        block_drain_ms,
        outcome,
        outcome_source,
        total_ms,
    );
}

fn effective_background_opacity(snapshot: &ScreenSnapshot, config: &RendererConfig) -> f32 {
    let opacity = config.background_opacity.clamp(0.0, 1.0);
    // Full-screen TUIs use the terminal default background as their
    // application canvas. Match Windows Terminal/Ghostty behavior
    // here: ordinary shells keep the configured glass level, but
    // alternate-screen apps paint a solid canvas. If the user
    // explicitly sets the terminal to fully transparent, respect that.
    if snapshot.alternate_screen && opacity > f32::EPSILON {
        opacity.max(ALTERNATE_SCREEN_BACKGROUND_OPACITY_FLOOR)
    } else {
        opacity
    }
}

fn is_default_blank_cell(
    cell: &Cell,
    effective_attrs: u8,
    config: &RendererConfig,
    background_opacity: f32,
) -> bool {
    let is_blank = cell.codepoint == 0 || cell.codepoint == 0x20;
    if !is_blank {
        return false;
    }

    // Default-background cells use alpha=0 as a sentinel. Explicit SGR
    // backgrounds are opaque and still need a quad, even for spaces.
    if (cell.bg & 0xFF) != 0 {
        return false;
    }

    // Bold / italic have no visual effect on a blank cell, but these
    // attributes do: underline/strike draw bands, inverse/selection
    // swaps fg/bg and paints a highlight.
    if (effective_attrs & (ATTR_UNDERLINE | ATTR_STRIKE | ATTR_INVERSE)) != 0 {
        return false;
    }

    if background_opacity <= f32::EPSILON {
        return true;
    }

    let clear_rgb = [
        color_channel_byte(config.clear_color[0]),
        color_channel_byte(config.clear_color[1]),
        color_channel_byte(config.clear_color[2]),
    ];
    let bg_rgb = [
        ((cell.bg >> 24) & 0xFF) as u8,
        ((cell.bg >> 16) & 0xFF) as u8,
        ((cell.bg >> 8) & 0xFF) as u8,
    ];
    clear_rgb == bg_rgb
}

fn color_channel_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn geometry_fingerprint(cols: u16, rows: u16, width_px: u32, height_px: u32) -> u64 {
    let mut hash = 0x9E37_79B9_7F4A_7C15u64;
    hash ^= cols as u64;
    hash = hash.rotate_left(13).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash ^= rows as u64;
    hash = hash.rotate_left(17).wrapping_mul(0x94D0_49BB_1331_11EB);
    hash ^= width_px as u64;
    hash = hash.rotate_left(29).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    hash ^= height_px as u64;
    hash.rotate_left(31)
}

fn can_block_for_latest(
    in_flight_before_submit: usize,
    backlog: bool,
    submitted: Option<SubmittedCopy>,
) -> bool {
    if !backlog {
        return true;
    }

    // One older slot still being copied after a resize is common and
    // acceptable: if the fresh interactive frame lands in the other,
    // clean slot we can still wait for it without inheriting the full
    // backlog tail. Once both slots are already busy, or we had to
    // reuse an in-flight slot, keep the UI thread non-blocking.
    in_flight_before_submit <= 1 && submitted.is_some_and(|copy| !copy.replaced_in_flight)
}

/// One slot in the readback ring.
struct StagingSlot {
    texture: ID3D11Texture2D,
    /// `true` once a `CopyResource` has been queued into this slot but
    /// before the next `Map()` drains it. Drained slots can be safely
    /// overwritten by the next `submit_copy`.
    in_flight: bool,
    /// Monotonic submit counter — used to find the OLDEST in-flight
    /// slot when picking a drain target. (`next_idx` alone wraps and
    /// can't disambiguate "oldest" from "newest" once both are in
    /// flight.)
    seq: u64,
    regions: Vec<ReadbackRegion>,
    kitty_visuals: KittyVisualState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KittyVisual {
    image_id: u32,
    image_generation: u64,
    image_size: [u32; 2],
    z: i32,
    viewport: [i32; 2],
    cell_offset: [u32; 2],
    pixel_size: [u32; 2],
    source: [u32; 4],
}

impl From<&KittyPlacement> for KittyVisual {
    fn from(placement: &KittyPlacement) -> Self {
        Self {
            image_id: placement.image.id,
            image_generation: placement.image.generation,
            image_size: [placement.image.width, placement.image.height],
            z: placement.z,
            viewport: [placement.viewport_col, placement.viewport_row],
            cell_offset: [placement.cell_x_offset, placement.cell_y_offset],
            pixel_size: [placement.pixel_width, placement.pixel_height],
            source: [
                placement.source_x,
                placement.source_y,
                placement.source_width,
                placement.source_height,
            ],
        }
    }
}

impl KittyVisual {
    fn vertical_region(self, cell_height: u32, viewport_height: u32) -> Option<ReadbackRegion> {
        let top = i64::from(self.viewport[1])
            .saturating_mul(i64::from(cell_height))
            .saturating_add(i64::from(self.cell_offset[1]));
        let bottom = top.saturating_add(i64::from(self.pixel_size[1]));
        let viewport_height = i64::from(viewport_height);
        let top = top.clamp(0, viewport_height) as u32;
        let bottom = bottom.clamp(0, viewport_height) as u32;
        (top < bottom).then_some(ReadbackRegion {
            y: top,
            height: bottom - top,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct KittyVisualState {
    cell_size: [u32; 2],
    placements: Arc<[KittyVisual]>,
}

impl KittyVisualState {
    fn from_placements(placements: &[KittyPlacement], cell_size: [u32; 2]) -> Self {
        Self {
            cell_size,
            placements: placements
                .iter()
                .map(KittyVisual::from)
                .collect::<Vec<_>>()
                .into(),
        }
    }
}

#[derive(Debug, Default)]
struct KittyReadbackState {
    /// Exact image state represented by the last readback handed to GPUI.
    /// Submitted states are deliberately not authoritative: staging slots are
    /// a lossy mailbox and may be replaced before presentation.
    presented: KittyVisualState,
}

impl KittyReadbackState {
    fn damage_regions(
        &self,
        current: &KittyVisualState,
        viewport_height: u32,
    ) -> Vec<ReadbackRegion> {
        let mut regions = Vec::new();
        append_kitty_transition_regions(&self.presented, current, viewport_height, &mut regions);
        regions
    }

    fn presented(&mut self, visuals: KittyVisualState) {
        self.presented = visuals;
    }
}

fn append_kitty_transition_regions(
    previous: &KittyVisualState,
    current: &KittyVisualState,
    viewport_height: u32,
    regions: &mut Vec<ReadbackRegion>,
) {
    if previous == current {
        return;
    }

    let (prefix, suffix) = if previous.cell_size == current.cell_size {
        let prefix = previous
            .placements
            .iter()
            .zip(current.placements.iter())
            .take_while(|(left, right)| left == right)
            .count();
        let suffix_limit = previous
            .placements
            .len()
            .min(current.placements.len())
            .saturating_sub(prefix);
        let suffix = previous
            .placements
            .iter()
            .rev()
            .zip(current.placements.iter().rev())
            .take(suffix_limit)
            .take_while(|(left, right)| left == right)
            .count();
        (prefix, suffix)
    } else {
        (0, 0)
    };

    let previous_end = previous.placements.len().saturating_sub(suffix);
    let current_end = current.placements.len().saturating_sub(suffix);
    regions.reserve(
        previous_end
            .saturating_sub(prefix)
            .saturating_add(current_end.saturating_sub(prefix)),
    );
    for visual in &previous.placements[prefix..previous_end] {
        if let Some(region) = visual.vertical_region(previous.cell_size[1], viewport_height) {
            regions.push(region);
        }
    }
    for visual in &current.placements[prefix..current_end] {
        if let Some(region) = visual.vertical_region(current.cell_size[1], viewport_height) {
            regions.push(region);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SubmittedCopy {
    idx: usize,
    replaced_in_flight: bool,
    readback_regions: usize,
    readback_rows: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadbackRegion {
    y: u32,
    height: u32,
}

impl ReadbackRegion {
    fn full(height: u32) -> Self {
        Self { y: 0, height }
    }

    fn is_full(&self, height: u32) -> bool {
        self.y == 0 && self.height >= height
    }
}

fn merge_readback_regions(regions: &mut Vec<ReadbackRegion>, viewport_height: u32) {
    regions.sort_unstable_by_key(|region| region.y);
    let mut write = 0usize;
    for read in 0..regions.len() {
        let y = regions[read].y.min(viewport_height);
        let bottom = regions[read]
            .y
            .saturating_add(regions[read].height)
            .min(viewport_height);
        if y >= bottom {
            continue;
        }

        if write > 0 {
            let previous = &mut regions[write - 1];
            let previous_bottom = previous.y.saturating_add(previous.height);
            if previous_bottom >= y {
                previous.height = previous_bottom.max(bottom) - previous.y;
                continue;
            }
        }
        regions[write] = ReadbackRegion {
            y,
            height: bottom - y,
        };
        write += 1;
    }
    regions.truncate(write);
}

struct Readback {
    bytes: Vec<u8>,
    regions: Vec<ReadbackRegion>,
    kitty_visuals: KittyVisualState,
}

/// Two-slot staging ring. See `Renderer::render` for the state machine.
///
/// The ring behaves like a mailbox: each slot is a readback cache of a
/// previously rendered frame, not authoritative terminal state. When
/// the GPU falls behind we would rather reclaim the oldest unread slot
/// than block GPUI's thread trying to preserve stale pixels.
struct StagingRing {
    slots: Vec<StagingSlot>,
    next_idx: usize,
    next_seq: u64,
    width: u32,
    height: u32,
}

impl StagingRing {
    const DEPTH: usize = 2;

    fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Self> {
        let mut slots = Vec::with_capacity(Self::DEPTH);
        for _ in 0..Self::DEPTH {
            slots.push(StagingSlot {
                texture: create_staging_texture(device, width, height)?,
                in_flight: false,
                seq: 0,
                regions: vec![ReadbackRegion::full(height)],
                kitty_visuals: KittyVisualState::default(),
            });
        }
        Ok(Self {
            slots,
            next_idx: 0,
            next_seq: 0,
            width,
            height,
        })
    }

    fn recreate(&mut self, device: &ID3D11Device, width: u32, height: u32) -> Result<()> {
        // Allocate the new slots locally first; only swap into `self`
        // after every fallible call has succeeded so a mid-loop failure
        // leaves the ring usable at its old dimensions.
        let mut new_slots = Vec::with_capacity(Self::DEPTH);
        for _ in 0..Self::DEPTH {
            new_slots.push(StagingSlot {
                texture: create_staging_texture(device, width, height)?,
                in_flight: false,
                seq: 0,
                regions: vec![ReadbackRegion::full(height)],
                kitty_visuals: KittyVisualState::default(),
            });
        }
        self.slots = new_slots;
        self.next_idx = 0;
        self.next_seq = 0;
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn submit_copy_mailbox(
        &mut self,
        ctx: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        regions: &[ReadbackRegion],
        kitty_visuals: KittyVisualState,
    ) -> SubmittedCopy {
        let clean_idx = self
            .slots
            .iter()
            .enumerate()
            .find_map(|(i, slot)| (!slot.in_flight).then_some(i));
        let idx = if let Some(idx) = clean_idx {
            idx
        } else {
            self.oldest_in_flight().unwrap_or(self.next_idx)
        };
        let replaced_in_flight = self.slots[idx].in_flight;
        let regions = if regions.is_empty() {
            vec![ReadbackRegion::full(self.height)]
        } else {
            regions.to_vec()
        };
        if regions.len() == 1 && regions[0].is_full(self.height) {
            unsafe {
                ctx.CopyResource(&self.slots[idx].texture, source);
            }
        } else {
            for region in &regions {
                let top = region.y.min(self.height);
                let bottom = top.saturating_add(region.height).min(self.height);
                if top >= bottom {
                    continue;
                }
                let src_box = D3D11_BOX {
                    left: 0,
                    top,
                    front: 0,
                    right: self.width,
                    bottom,
                    back: 1,
                };
                unsafe {
                    ctx.CopySubresourceRegion(
                        &self.slots[idx].texture,
                        0,
                        0,
                        top,
                        0,
                        source,
                        0,
                        Some(&src_box as *const D3D11_BOX),
                    );
                }
            }
        }
        let slot = &mut self.slots[idx];
        let readback_regions = regions.len();
        let readback_rows = regions.iter().map(|region| region.height).sum();
        slot.in_flight = true;
        slot.seq = self.next_seq;
        slot.regions = regions;
        slot.kitty_visuals = kitty_visuals;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.next_idx = (idx + 1) % self.slots.len();
        SubmittedCopy {
            idx,
            replaced_in_flight,
            readback_regions,
            readback_rows,
        }
    }

    fn in_flight_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.in_flight).count()
    }

    /// Slot with the lowest `seq` among in-flight slots, or `None` when
    /// the ring is idle.
    fn oldest_in_flight(&self) -> Option<usize> {
        let mut best: Option<(usize, u64)> = None;
        for (i, slot) in self.slots.iter().enumerate() {
            if !slot.in_flight {
                continue;
            }
            match best {
                None => best = Some((i, slot.seq)),
                Some((_, s)) if slot.seq < s => best = Some((i, slot.seq)),
                _ => {}
            }
        }
        best.map(|(i, _)| i)
    }

    fn discard_oldest_in_flight(&mut self) -> Option<usize> {
        let idx = self.oldest_in_flight()?;
        let slot = &mut self.slots[idx];
        slot.in_flight = false;
        Some(idx)
    }

    /// Non-blocking drain: returns `Ok(Some(readback))` if the slot is
    /// ready, `Ok(None)` if the GPU is still drawing into it.
    fn try_drain(&mut self, ctx: &ID3D11DeviceContext, idx: usize) -> Result<Option<Readback>> {
        self.drain_with_flags(ctx, idx, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32)
    }

    /// Blocking drain: waits for the GPU to finish the slot's copy,
    /// then maps it. Used as a fallback when the GPU somehow exceeds a
    /// full prepaint cycle to drain `try_drain`'s target.
    fn block_drain(&mut self, ctx: &ID3D11DeviceContext, idx: usize) -> Result<Option<Readback>> {
        self.drain_with_flags(ctx, idx, 0)
    }

    fn drain_with_flags(
        &mut self,
        ctx: &ID3D11DeviceContext,
        idx: usize,
        flags: u32,
    ) -> Result<Option<Readback>> {
        let width = self.width as usize;
        let height = self.height as usize;
        let slot = &mut self.slots[idx];
        if !slot.in_flight {
            return Ok(None);
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        let map_result =
            unsafe { ctx.Map(&slot.texture, 0, D3D11_MAP_READ, flags, Some(&mut mapped)) };
        if let Err(err) = map_result {
            if err.code() == DXGI_ERROR_WAS_STILL_DRAWING {
                return Ok(None);
            }
            return Err(anyhow::anyhow!("Map(staging slot {idx}) failed: {err}"));
        }

        let regions = slot.regions.clone();
        let full = regions.len() == 1 && regions[0].is_full(self.height);
        let row_bytes = width * 4;
        let len = if full {
            row_bytes * height
        } else {
            regions
                .iter()
                .map(|region| region.height.min(self.height.saturating_sub(region.y)) as usize)
                .sum::<usize>()
                * row_bytes
        };
        let mut out: Vec<u8> = Vec::with_capacity(len);
        let src_pitch = mapped.RowPitch as usize;
        let mut dst_offset = 0usize;
        for region in &regions {
            let top = region.y.min(self.height) as usize;
            let bottom = region.y.saturating_add(region.height).min(self.height) as usize;
            for y in top..bottom {
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        (mapped.pData as *const u8).add(src_pitch * y),
                        out.as_mut_ptr().add(dst_offset),
                        row_bytes,
                    );
                }
                dst_offset += row_bytes;
            }
        }
        // Every byte has been filled by the row-copy loop above.
        // Avoiding `vec![0; len]` saves a full-frame memset on the hot
        // readback path before we immediately overwrite the buffer.
        unsafe {
            out.set_len(len);
        }
        unsafe {
            ctx.Unmap(&slot.texture, 0);
        }
        slot.in_flight = false;
        let kitty_visuals = std::mem::take(&mut slot.kitty_visuals);
        Ok(Some(Readback {
            bytes: out,
            regions,
            kitty_visuals,
        }))
    }
}

fn create_device() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    let mut feature_level = D3D_FEATURE_LEVEL_11_0;

    let result = unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            Some(&mut feature_level),
            Some(&mut context),
        )
    };

    if result.is_err() {
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_WARP,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )
        }
        .context("D3D11CreateDevice failed for both HARDWARE and WARP")?;
    }

    let device = device.context("D3D11CreateDevice produced no device")?;
    let context = context.context("D3D11CreateDevice produced no context")?;
    Ok((device, context))
}

fn create_rt_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<(ID3D11Texture2D, ID3D11RenderTargetView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }
        .context("CreateTexture2D(rt) failed")?;
    let texture = texture.context("rt CreateTexture2D produced no texture")?;

    let rtv_desc = D3D11_RENDER_TARGET_VIEW_DESC {
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
        Anonymous: windows::Win32::Graphics::Direct3D11::D3D11_RENDER_TARGET_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
        },
    };
    let mut rtv = None;
    unsafe { device.CreateRenderTargetView(&texture, Some(&rtv_desc), Some(&mut rtv)) }
        .context("CreateRenderTargetView(rt) failed")?;
    let rtv = rtv.context("CreateRenderTargetView produced no view")?;
    Ok((texture, rtv))
}

fn create_staging_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut texture)) }
        .context("CreateTexture2D(staging) failed")?;
    texture.context("staging CreateTexture2D produced no texture")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        KittyReadbackState, KittyVisual, KittyVisualState, ReadbackRegion, merge_readback_regions,
    };

    const VIEWPORT_HEIGHT: u32 = 100;

    fn visual(row: i32) -> KittyVisual {
        KittyVisual {
            image_id: 1,
            image_generation: 2,
            image_size: [8, 8],
            z: 0,
            viewport: [0, row],
            cell_offset: [0, 0],
            pixel_size: [8, 10],
            source: [0, 0, 8, 8],
        }
    }

    fn visual_state(placements: Vec<KittyVisual>) -> KittyVisualState {
        KittyVisualState {
            cell_size: [8, 10],
            placements: Arc::from(placements),
        }
    }

    fn damage(state: &KittyReadbackState, current: &KittyVisualState) -> Vec<ReadbackRegion> {
        let mut regions = state.damage_regions(current, VIEWPORT_HEIGHT);
        merge_readback_regions(&mut regions, VIEWPORT_HEIGHT);
        regions
    }

    #[test]
    fn unchanged_kitty_images_add_no_readback_damage() {
        let current = visual_state(vec![visual(2)]);
        let state = KittyReadbackState {
            presented: current.clone(),
        };

        assert!(damage(&state, &current).is_empty());
    }

    #[test]
    fn moved_kitty_image_damages_old_and_new_rows() {
        let previous = visual_state(vec![visual(1)]);
        let current = visual_state(vec![visual(5)]);
        let state = KittyReadbackState {
            presented: previous,
        };

        assert_eq!(
            damage(&state, &current),
            vec![
                ReadbackRegion { y: 10, height: 10 },
                ReadbackRegion { y: 50, height: 10 },
            ]
        );
    }

    #[test]
    fn inserted_kitty_image_preserves_common_prefix_and_suffix() {
        let previous = visual_state(vec![visual(1), visual(7)]);
        let current = visual_state(vec![visual(1), visual(4), visual(7)]);
        let state = KittyReadbackState {
            presented: previous,
        };

        assert_eq!(
            damage(&state, &current),
            vec![ReadbackRegion { y: 40, height: 10 }]
        );
    }

    #[test]
    fn removed_kitty_image_damages_its_previous_rows() {
        let previous = visual_state(vec![visual(7)]);
        let current = visual_state(Vec::new());
        let state = KittyReadbackState {
            presented: previous,
        };

        assert_eq!(
            damage(&state, &current),
            vec![ReadbackRegion { y: 70, height: 10 }]
        );
    }
}
