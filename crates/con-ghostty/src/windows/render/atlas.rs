//! Glyph atlas.
//!
//! `BGRA8_UNORM` texture with skyline packing (`etagere`). Glyphs are
//! rasterized via Direct2D `DrawText` onto a `ID2D1RenderTarget` that
//! aliases the atlas texture through a DXGI surface. We intentionally
//! use grayscale antialiasing, not ClearType: this atlas is an offscreen
//! texture that is later scaled, copied, composited with transparency,
//! and often inspected through screenshots. Subpixel RGB coverage looks
//! sharp on a physical LCD panel but becomes colored fringe in that
//! pipeline.

use std::collections::HashMap;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use etagere::{AllocId, AtlasAllocator, size2};
use unicode_width::UnicodeWidthChar;
use windows::Win32::Globalization::GetUserDefaultLocaleName;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_ALIASED, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_OPTIONS, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Factory, ID2D1RenderTarget,
    ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D::D3D11_SRV_DIMENSION_TEXTURE2D;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_SHADER_RESOURCE_VIEW_DESC,
    D3D11_TEX2D_SRV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, ID3D11Device, ID3D11DeviceContext,
    ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_METRICS, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_BOLD,
    DWRITE_FONT_WEIGHT_MEDIUM, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_GLYPH_METRICS,
    DWRITE_LINE_METRICS, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PIXEL_GEOMETRY_FLAT,
    DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC, IDWriteFactory, IDWriteFactory1,
    IDWriteFontCollection, IDWriteFontFace, IDWriteFontFallback, IDWriteRenderingParams,
    IDWriteRenderingParams1, IDWriteTextFormat, IDWriteTextFormat1,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGISurface;
use windows::core::Interface;
use windows_numerics::Matrix3x2;

const LOCALE_NAME_BUFFER_LENGTH: usize = 85;
const FALLBACK_LOCALE_NAME: &str = "en-US";
const DEFAULT_TEXT_GAMMA: f32 = 1.8;
const DEFAULT_GRAYSCALE_CONTRAST: f32 = 1.0;
const CJK_GRAYSCALE_CONTRAST_FLOOR: f32 = 1.45;

static USER_LOCALE_NAME: LazyLock<Vec<u16>> = LazyLock::new(user_locale_name);

fn user_locale_name() -> Vec<u16> {
    // LOCALE_NAME_MAX_LENGTH is 85 UTF-16 code units including the terminator.
    let mut locale = vec![0; LOCALE_NAME_BUFFER_LENGTH];
    let length = unsafe { GetUserDefaultLocaleName(&mut locale) };
    if length > 1 && (length as usize) <= locale.len() {
        locale.truncate(length as usize);
        log::info!(
            "DirectWrite user locale: {}",
            String::from_utf16_lossy(&locale[..locale.len() - 1])
        );
        return locale;
    }

    log::warn!("GetUserDefaultLocaleName failed; using DirectWrite locale {FALLBACK_LOCALE_NAME}");
    FALLBACK_LOCALE_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // offset_x/offset_y are wired in Phase 3b-2 (glyph bearing).
pub struct GlyphRect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub offset_x: i16,
    pub offset_y: i16,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct GlyphKey {
    pub codepoint: u32,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Debug, Clone, Copy)]
struct GlyphMetricsPx {
    /// Natural ink-box width (advance − |lsb| − |rsb| in design units
    /// → pixels). Used to decide whether to widen the atlas slot.
    ink_w_px: f32,
    /// Natural ink-box height (advanceHeight − topSB − bottomSB in
    /// design units → pixels). Used together with `ink_w_px` to scale
    /// square-authored Nerd-Font icons so they fill the cell's longer
    /// dimension instead of collapsing to the shorter one.
    ink_h_px: f32,
    /// Left-side bearing in physical pixels. Negative values mean the
    /// ink box extends leftward past the advance origin — we shift the
    /// draw rect right by `-lsb_px` so that overhang fits in the slot.
    lsb_px: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    pub cell_width_px: u32,
    pub cell_height_px: u32,
    pub baseline_px: u32,
}

/// DirectWrite's color-dependent grayscale correction inputs. The atlas keeps
/// reusable coverage; the final text pass applies these values after the real
/// foreground color is known.
#[derive(Debug, Clone, Copy)]
pub(super) struct GlyphRenderingParams {
    pub gamma_ratios: [f32; 4],
    pub grayscale_contrast: f32,
    pub cjk_grayscale_contrast: f32,
}

impl GlyphRenderingParams {
    const LEGACY_ATLAS: Self = Self {
        gamma_ratios: [0.0; 4],
        grayscale_contrast: 0.0,
        cjk_grayscale_contrast: 0.0,
    };
}

#[derive(Debug, Clone, Copy)]
struct TextFormatBaselines {
    regular: f32,
    bold: f32,
    italic: f32,
    bold_italic: f32,
}

impl TextFormatBaselines {
    fn measure(
        dwrite: &IDWriteFactory,
        fallback_baseline: f32,
        regular: &IDWriteTextFormat,
        bold: &IDWriteTextFormat,
        italic: &IDWriteTextFormat,
        bold_italic: &IDWriteTextFormat,
    ) -> Self {
        let baseline = |format: &IDWriteTextFormat| {
            text_layout_baseline(dwrite, format, &[b'M' as u16]).unwrap_or(fallback_baseline)
        };
        Self {
            regular: baseline(regular),
            bold: baseline(bold),
            italic: baseline(italic),
            bold_italic: baseline(bold_italic),
        }
    }

    fn for_style(self, bold: bool, italic: bool) -> f32 {
        match (bold, italic) {
            (true, true) => self.bold_italic,
            (true, false) => self.bold,
            (false, true) => self.italic,
            (false, false) => self.regular,
        }
    }
}

pub struct GlyphCache {
    device: ID3D11Device,
    _context: ID3D11DeviceContext,
    dwrite: IDWriteFactory,
    /// The bundled collection remains available even when the currently
    /// selected family comes from the system collection, so a live settings
    /// change can switch back to IoskeleyMono without recreating the renderer.
    bundled_font_collection: Option<IDWriteFontCollection>,
    /// Font collection that owns `font_family`. `Some` only when the resolved
    /// family comes from our bundled collection; `None` means DirectWrite
    /// resolves it from the system collection.
    font_collection: Option<IDWriteFontCollection>,
    /// Ordered custom cascade attached to each `IDWriteTextFormat1`:
    /// user-selected installed families, bundled PUA icons, then Windows
    /// system fallback. `None` only when the runtime cannot construct a
    /// fallback object.
    font_fallback: Option<IDWriteFontFallback>,
    _d2d_factory: ID2D1Factory,

    atlas_size: u32,
    _atlas_texture: ID3D11Texture2D,
    atlas_srv: ID3D11ShaderResourceView,
    d2d_rt: ID2D1RenderTarget,
    /// Linear grayscale rasterization parameters. The atlas stores neutral
    /// coverage; foreground-aware gamma and contrast correction happens in
    /// the pixel shader when the actual text color is known.
    _text_rendering_params: Option<IDWriteRenderingParams>,
    rendering_params: GlyphRenderingParams,
    white_brush: ID2D1SolidColorBrush,
    /// Opaque-black brush. Used to clear each slot before `DrawText` so
    /// any stale pixels — from a neighbouring scaled-PUA glyph that bled
    /// past its own slot via grayscale AA fringe or wide layoutRect —
    /// don't show through as speckles on thin glyphs (hyphens,
    /// box-drawing).
    black_brush: ID2D1SolidColorBrush,

    allocator: AtlasAllocator,
    entries: HashMap<GlyphKey, (AllocId, GlyphRect)>,

    text_format_regular: IDWriteTextFormat,
    text_format_bold: IDWriteTextFormat,
    text_format_italic: IDWriteTextFormat,
    text_format_bold_italic: IDWriteTextFormat,
    text_format_cjk_regular: IDWriteTextFormat,
    text_format_cjk_italic: IDWriteTextFormat,

    /// Regular-weight face of the primary family, kept around so we can
    /// query per-glyph design metrics at rasterize time and scale wide
    /// Nerd-Font icons to fit a single cell. `None` means the family
    /// couldn't be resolved (logged at init) — we then skip scaling and
    /// DrawText renders glyphs as-is (legacy behavior).
    primary_face: Option<IDWriteFontFace>,
    /// Design units per em for `primary_face`. Cached to avoid calling
    /// `GetMetrics` on every rasterize. Only meaningful when
    /// `primary_face.is_some()`.
    primary_upm: f32,

    metrics: CellMetrics,
    layout_baselines: TextFormatBaselines,
    font_size_px: f32,
    font_family: String,
}

impl GlyphCache {
    pub fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        dwrite: &IDWriteFactory,
        bundled_collection: Option<IDWriteFontCollection>,
        font_family: &str,
        font_fallback_families: &[String],
        font_size_px: f32,
        atlas_size: u32,
    ) -> Result<Self> {
        // Ordered user / bundled-icon / system cascade. Built once here and
        // shared across all text-format weights.
        let font_fallback = super::font_loader::font_fallback(
            dwrite,
            bundled_collection.as_ref(),
            font_fallback_families,
        );

        // Resolve the family and the collection as a pair. Passing the
        // bundled collection for a user-selected system font makes
        // DirectWrite fail the primary lookup inside `CreateTextFormat`
        // while `measure_cell` may still measure the system face. That
        // split produces visibly wide cells with narrow glyph ink. Keep
        // text formats, cell metrics, and primary-face probing on the
        // same resolved source.
        let resolved = resolve_font_family(dwrite, bundled_collection.as_ref(), font_family)?;
        let resolved_family = resolved.family;
        let text_collection = resolved.collection;
        let font_collection = text_collection.cloned();

        let text_format_regular = make_text_format(
            dwrite,
            text_collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            false,
            false,
        )?;
        let text_format_bold = make_text_format(
            dwrite,
            text_collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            true,
            false,
        )?;
        let text_format_italic = make_text_format(
            dwrite,
            text_collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            false,
            true,
        )?;
        let text_format_bold_italic = make_text_format(
            dwrite,
            text_collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            true,
            true,
        )?;
        let text_format_cjk_regular = make_text_format_with_weight(
            dwrite,
            text_collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            DWRITE_FONT_WEIGHT_MEDIUM,
            false,
        )?;
        let text_format_cjk_italic = make_text_format_with_weight(
            dwrite,
            text_collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            DWRITE_FONT_WEIGHT_MEDIUM,
            true,
        )?;

        let metrics = measure_cell(dwrite, text_collection, &resolved_family, font_size_px)?;
        let layout_baselines = TextFormatBaselines::measure(
            dwrite,
            metrics.baseline_px as f32,
            &text_format_regular,
            &text_format_bold,
            &text_format_italic,
            &text_format_bold_italic,
        );

        // Resolve a face for per-glyph design-metrics lookups. The face
        // comes from the same collection `measure_cell` walked, so
        // bundled > system > last-resort Segoe. We intentionally ignore
        // errors here — on failure `primary_face` stays `None` and the
        // rasterize path skips scale-to-fit, matching pre-fix behavior.
        let (primary_face, primary_upm) =
            match resolve_font_face(dwrite, text_collection, &resolved_family) {
                Ok((face, _src)) => {
                    let mut fm = DWRITE_FONT_METRICS::default();
                    // SAFETY: windows-rs writes through the out pointer.
                    unsafe { face.GetMetrics(&mut fm) };
                    (Some(face), fm.designUnitsPerEm as f32)
                }
                Err(err) => {
                    log::warn!(
                        "GlyphCache::new: primary face resolution failed ({err:?}); \
                         wide Nerd-Font icons will render clipped at the cell edge"
                    );
                    (None, 1.0)
                }
            };

        let atlas_desc = D3D11_TEXTURE2D_DESC {
            Width: atlas_size,
            Height: atlas_size,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            // D2D's CreateDxgiSurfaceRenderTarget requires the backing
            // texture to be a render target — D2D composites glyph
            // rasterization into it. We also need SHADER_RESOURCE so
            // the D3D11 pixel shader can sample the same atlas.
            BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut atlas_texture: Option<ID3D11Texture2D> = None;
        // SAFETY: desc is stack-local; out param owned by us.
        unsafe { device.CreateTexture2D(&atlas_desc, None, Some(&mut atlas_texture)) }
            .context("CreateTexture2D failed for atlas")?;
        let atlas_texture = atlas_texture.context("atlas CreateTexture2D produced no texture")?;

        let srv_desc = D3D11_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2D,
            Anonymous: windows::Win32::Graphics::Direct3D11::D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_SRV {
                    MostDetailedMip: 0,
                    MipLevels: 1,
                },
            },
        };
        let mut atlas_srv: Option<ID3D11ShaderResourceView> = None;
        // SAFETY: descriptor valid; texture owned.
        unsafe {
            device.CreateShaderResourceView(&atlas_texture, Some(&srv_desc), Some(&mut atlas_srv))
        }
        .context("CreateShaderResourceView failed for atlas")?;
        let atlas_srv = atlas_srv.context("atlas CreateShaderResourceView produced no view")?;

        let factory_options = D2D1_FACTORY_OPTIONS::default();
        // 0.61's D2D1CreateFactory is generic over the factory interface.
        // SAFETY: options are stack-local; generic return is owned.
        let d2d_factory: ID2D1Factory = unsafe {
            D2D1CreateFactory::<ID2D1Factory>(
                D2D1_FACTORY_TYPE_SINGLE_THREADED,
                Some(&factory_options),
            )
        }
        .context("D2D1CreateFactory failed")?;

        let dxgi_surface: IDXGISurface = atlas_texture
            .cast()
            .context("atlas texture -> IDXGISurface cast failed")?;
        // D2D draws grayscale glyph coverage into RGB channels on an
        // opaque-alpha render target. Alpha stays unused; the shader
        // derives final output alpha from coverage and the cell colors.
        let rt_props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_IGNORE,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        // SAFETY: surface + props valid.
        let d2d_rt = unsafe { d2d_factory.CreateDxgiSurfaceRenderTarget(&dxgi_surface, &rt_props) }
            .context("CreateDxgiSurfaceRenderTarget failed")?;

        // Store neutral grayscale coverage in the atlas. Baking gamma and
        // contrast here would bake the white-on-black DrawText colors into
        // every glyph, then incorrectly reuse those values for dark text on
        // light themes. The pixel shader applies DirectWrite-compatible,
        // foreground-aware correction when it knows the real colors.
        let (text_rendering_params, rendering_params) = text_rendering_params(dwrite);
        // SAFETY: grayscale AA. Setting the mode is cheap; if a driver
        // clamps it, the shader still collapses coverage to one scalar
        // so colored subpixel fringe cannot escape to the final frame.
        unsafe {
            d2d_rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            if let Some(params) = text_rendering_params.as_ref() {
                d2d_rt.SetTextRenderingParams(params);
            }
        }

        let color = D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        // SAFETY: color is stack-local; brush owned by us.
        let white_brush: ID2D1SolidColorBrush =
            unsafe { d2d_rt.CreateSolidColorBrush(&color, None) }
                .context("CreateSolidColorBrush failed")?;

        let black = D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        // SAFETY: color is stack-local; brush owned by us.
        let black_brush: ID2D1SolidColorBrush =
            unsafe { d2d_rt.CreateSolidColorBrush(&black, None) }
                .context("CreateSolidColorBrush(black) failed")?;

        // Seed the atlas with black so glyph coverage starts from a
        // defined blend target. D3D11 zero-inits the texture but the
        // 0-alpha of (0,0,0,0) can confuse D2D's internal state
        // assertions in some drivers; an explicit Clear sidesteps that.
        // SAFETY: d2d_rt owned by us and targets the atlas texture.
        unsafe {
            d2d_rt.BeginDraw();
            d2d_rt.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }));
            let _ = d2d_rt.EndDraw(None, None);
        }

        let allocator = AtlasAllocator::new(size2(atlas_size as i32, atlas_size as i32));

        Ok(Self {
            device: device.clone(),
            _context: context.clone(),
            dwrite: dwrite.clone(),
            bundled_font_collection: bundled_collection,
            font_collection,
            font_fallback,
            _d2d_factory: d2d_factory,
            atlas_size,
            _atlas_texture: atlas_texture,
            atlas_srv,
            d2d_rt,
            _text_rendering_params: text_rendering_params,
            rendering_params,
            white_brush,
            black_brush,
            allocator,
            entries: HashMap::with_capacity(1024),
            text_format_regular,
            text_format_bold,
            text_format_italic,
            text_format_bold_italic,
            text_format_cjk_regular,
            text_format_cjk_italic,
            primary_face,
            primary_upm,
            metrics,
            layout_baselines,
            font_size_px,
            // Store the RESOLVED family and collection — downstream
            // rebuilds must use the same source to keep metrics and
            // rasterization in lockstep.
            font_family: resolved_family,
        })
    }

    pub fn metrics(&self) -> CellMetrics {
        self.metrics
    }
    pub fn atlas_srv(&self) -> &ID3D11ShaderResourceView {
        &self.atlas_srv
    }
    #[allow(dead_code)] // used once atlas-grow lands.
    pub fn atlas_size(&self) -> u32 {
        self.atlas_size
    }

    pub fn rendering_params(&self) -> GlyphRenderingParams {
        self.rendering_params
    }

    /// Return the glyph rect for a (codepoint, style) key, rasterizing
    /// on first sight. Returns `None` if the atlas is full.
    pub fn get_or_rasterize(&mut self, key: GlyphKey) -> Option<GlyphRect> {
        if let Some((_, rect)) = self.entries.get(&key).copied() {
            return Some(rect);
        }

        let cell_w = self.metrics.cell_width_px as i32;
        let cell_h = self.metrics.cell_height_px as i32;

        // Nerd-Font PUA icons (BMP and supplementary private-use ranges) are authored as roughly
        // square glyphs (~1000du × ~1000du) with an advance of one
        // monospace cell (~600du). In a tall-narrow cell (e.g. 17×35
        // px at font 28) fitting them by WIDTH collapses to a ~15×15
        // icon adrift in ~20px of vertical air. That's "icons look
        // tiny". Fit by the LONGER cell dimension (height) so the
        // icon renders at full natural size; let it overflow
        // horizontally into the (virtually always blank) next cell.
        //
        // NB: Powerline "extra symbols" (U+E0A0..U+E0D4) — arrows,
        // separators, branch — are authored flush-to-advance with ink
        // that already fits a 1-cell slot. Stretching them leaves
        // gaps between consecutive separators. Exempt the block and
        // draw at 1:1 with CLIP.
        //
        // Overflow z-order: when an icon's slot is wider than 1 cell,
        // the neighbour's bg-only instance would otherwise overdraw
        // the icon's right half. `Renderer::draw_cells` sorts
        // instances so wide ones trail — DX11 orders per-pixel writes
        // by instance ID within a draw call, so the wide icon wins.
        // Here we just widen the slot and let the VS use
        // `atlasSize.x` as the quad width (via the `max(cellSize.x,
        // atlasSize.x)` branch in `shaders.hlsl::vs_main`).
        let codepoint = key.codepoint;
        let is_scalable_pua = matches!(
            codepoint,
            0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD
        ) && !matches!(codepoint, 0xE0A0..=0xE0D4);
        let is_wide_text = is_wide_codepoint(codepoint);
        let is_cjk_text = is_cjk_codepoint(codepoint);
        let metrics = if is_scalable_pua {
            self.primary_glyph_metrics_px(codepoint)
        } else {
            None
        };

        // Decide (slot width, lsb shift, scale). Three cases:
        //   1. fits cell   → cell_w slot, no shift, no scale (fast path)
        //   2. width-only  → widen slot, shift layoutRect right by
        //                    |lsb_overhang| so negative-lsb ink lands
        //                    flush at slot.left, no scale
        //   3. height too  → scale-to-fit around slot centre + oversized
        //                    layoutRect so DirectWrite doesn't pre-clip
        //                    the natural ink before the transform shrinks
        //                    it back into the slot
        // Case 2 covers IoskeleyMono's full NF set at the default font
        // size (natural_h ≈ cell_h); case 3 is only for pathological
        // configurations where the cell is very short.
        let (glyph_w, lsb_shift_px, icon_scale): (i32, f32, Option<f32>) = match metrics {
            Some(m) => {
                let lsb_overhang = (-m.lsb_px).max(0.0);
                let natural_w = m.ink_w_px + lsb_overhang;
                let natural_h = m.ink_h_px.max(1.0);
                let fits_cell_w = natural_w <= (cell_w as f32) + 1.0;
                let fits_cell_h = natural_h <= (cell_h as f32) + 1.0;
                if fits_cell_w && fits_cell_h {
                    (cell_w, 0.0, None)
                } else if fits_cell_h {
                    let w = (natural_w.ceil() as i32).clamp(cell_w, 2 * cell_w);
                    (w, lsb_overhang, None)
                } else {
                    let by_h = (cell_h as f32) / natural_h;
                    let by_w_cap = (2.0 * cell_w as f32) / natural_w;
                    let scale = by_h.min(by_w_cap).clamp(0.5, 1.0);
                    let w = ((natural_w * scale).ceil() as i32).clamp(cell_w, 2 * cell_w);
                    (w, 0.0, Some(scale))
                }
            }
            // When DirectWrite resolves a PUA glyph from the configured or
            // bundled fallback, `primary_face` cannot provide its metrics.
            // Reserve the same two-cell overflow budget rather than clipping
            // the fallback icon to a narrow one-cell atlas slot.
            None if is_scalable_pua || is_wide_text => (cell_w.saturating_mul(2), 0.0, None),
            None => (cell_w, 0.0, None),
        };
        let alloc = self.allocator.allocate(size2(glyph_w, cell_h))?;
        let rect = alloc.rectangle;

        let ch = char::from_u32(key.codepoint).unwrap_or('\u{FFFD}');
        let mut utf16 = [0u16; 2];
        let utf16_slice = ch.encode_utf16(&mut utf16);

        let base_format = match (key.bold, key.italic) {
            (true, true) => &self.text_format_bold_italic,
            (true, false) => &self.text_format_bold,
            (false, true) => &self.text_format_italic,
            (false, false) => &self.text_format_regular,
        };
        let format = if is_cjk_text && !key.bold {
            if key.italic {
                &self.text_format_cjk_italic
            } else {
                &self.text_format_cjk_regular
            }
        } else {
            base_format
        };
        let target_baseline = self.layout_baselines.for_style(key.bold, key.italic);

        let slot_rect = D2D_RECT_F {
            left: rect.min.x as f32,
            top: rect.min.y as f32,
            right: rect.max.x as f32,
            bottom: rect.max.y as f32,
        };

        // Scale transform for pathological PUA glyphs whose natural
        // ink height exceeds the cell. Scale uniformly around the
        // slot's geometric center so DrawText's natural glyph
        // placement stays roughly centered in the slot. Standard-
        // sized icons take the width-only path (below) instead.
        let transform = icon_scale.map(|scale| {
            let cx = (slot_rect.left + slot_rect.right) * 0.5;
            let cy = (slot_rect.top + slot_rect.bottom) * 0.5;
            Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: (1.0 - scale) * cx,
                M32: (1.0 - scale) * cy,
            }
        });

        if metrics.is_some() && (glyph_w != cell_w || icon_scale.is_some()) {
            log::info!(
                "atlas: PUA U+{:04X} cell={}x{} slot_w={} lsb_shift={:.1} scale={:.3}",
                codepoint,
                cell_w,
                cell_h,
                glyph_w,
                lsb_shift_px,
                icon_scale.unwrap_or(1.0),
            );
        }

        let identity = Matrix3x2 {
            M11: 1.0,
            M12: 0.0,
            M21: 0.0,
            M22: 1.0,
            M31: 0.0,
            M32: 0.0,
        };

        // SAFETY: d2d_rt, format, brushes owned by self. BeginDraw /
        // EndDraw bracket a valid D2D scene; DrawText writes into the
        // atlas via the DXGI-backed RT. PushAxisAlignedClip pins all
        // writes to `slot_rect` so (a) antialias fringe at the slot
        // edges can't leak into adjacent atlas entries, and (b) the
        // scaled-PUA path — which uses an oversized layoutRect so
        // DirectWrite doesn't prematurely clip the natural ink — can't
        // overshoot into a neighbour's slot either. FillRectangle with
        // opaque black pre-clears any stale pixels (either zero-init
        // or left over from a previous glyph that bled in before we
        // added this clip).
        unsafe {
            self.d2d_rt.BeginDraw();
            self.d2d_rt
                .PushAxisAlignedClip(&slot_rect, D2D1_ANTIALIAS_MODE_ALIASED);
            self.d2d_rt.FillRectangle(&slot_rect, &self.black_brush);
            if let Some(tf) = transform {
                // Oversized layoutRect so DirectWrite's internal text-
                // layout clip doesn't cut off the natural ink before our
                // scale transform maps it back into the slot. The outer
                // PushAxisAlignedClip keeps any overshoot contained.
                let layout = D2D_RECT_F {
                    left: slot_rect.left - (cell_w as f32),
                    top: slot_rect.top - (cell_h as f32),
                    right: slot_rect.right + (cell_w as f32),
                    bottom: slot_rect.bottom + (cell_h as f32),
                };
                self.d2d_rt.SetTransform(&tf);
                self.d2d_rt.DrawText(
                    utf16_slice,
                    format,
                    &layout,
                    &self.white_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
                self.d2d_rt.SetTransform(&identity);
            } else if lsb_shift_px > 0.0 {
                // Width-overflow PUA: widened slot, pen shifted right
                // by |lsb_overhang| so the glyph's ink left edge lands
                // at slot.left. Skip CLIP so DirectWrite doesn't re-
                // clip to the shifted layoutRect — the outer
                // PushAxisAlignedClip already bounds writes to the
                // slot.
                let shifted = D2D_RECT_F {
                    left: slot_rect.left + lsb_shift_px,
                    top: slot_rect.top,
                    right: slot_rect.right + lsb_shift_px,
                    bottom: slot_rect.bottom,
                };
                self.d2d_rt.DrawText(
                    utf16_slice,
                    format,
                    &shifted,
                    &self.white_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            } else if is_wide_text {
                // CJK fallback glyphs need the two terminal cells the VT
                // reserves for them. Drawing into a one-cell layout rect
                // clips or visually narrows the fallback font, then the
                // second spacer cell makes the text look like it has
                // inserted spaces.
                //
                // DrawText lays this one codepoint as an isolated line.
                // Fallback CJK fonts do not necessarily share the primary
                // terminal face's baseline, so top-aligning the layout rect
                // lets glyphs drift vertically between characters and
                // fallback faces. Anchor the isolated layout's baseline back
                // to the terminal cell baseline before clipping to the slot.
                let layout = self.baseline_aligned_layout_rect(
                    slot_rect,
                    format,
                    utf16_slice,
                    target_baseline,
                );
                self.d2d_rt.DrawText(
                    utf16_slice,
                    format,
                    &layout,
                    &self.white_brush,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            } else {
                self.d2d_rt.DrawText(
                    utf16_slice,
                    format,
                    &slot_rect,
                    &self.white_brush,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }
            self.d2d_rt.PopAxisAlignedClip();
            let _ = self.d2d_rt.EndDraw(None, None);
        }

        let glyph_rect = GlyphRect {
            x: rect.min.x as u16,
            y: rect.min.y as u16,
            w: glyph_w as u16,
            h: cell_h as u16,
            offset_x: 0,
            offset_y: 0,
        };
        self.entries.insert(key, (alloc.id, glyph_rect));
        Some(glyph_rect)
    }

    fn baseline_aligned_layout_rect(
        &self,
        slot_rect: D2D_RECT_F,
        format: &IDWriteTextFormat,
        utf16: &[u16],
        target_baseline: f32,
    ) -> D2D_RECT_F {
        let Some(layout_baseline) = text_layout_baseline(&self.dwrite, format, utf16) else {
            return slot_rect;
        };
        let shift = target_baseline - layout_baseline;
        D2D_RECT_F {
            left: slot_rect.left,
            top: slot_rect.top + shift,
            right: slot_rect.right,
            bottom: slot_rect.bottom + shift,
        }
    }

    /// Glyph metrics in physical pixels for `codepoint` in the primary
    /// face, or `None` when we can't measure (face absent, glyph not in
    /// the primary face so DWrite fallback will handle it, or metrics
    /// call failed). Used at rasterize time to widen the atlas slot and
    /// offset the draw origin for overflow glyphs like Nerd-Font PUA
    /// icons whose ink boxes extend past the advance.
    fn primary_glyph_metrics_px(&self, codepoint: u32) -> Option<GlyphMetricsPx> {
        let face = self.primary_face.as_ref()?;
        let mut indices = [0u16; 1];
        let cps = [codepoint];
        // SAFETY: inputs sized 1; writes through out-pointer.
        let hr = unsafe { face.GetGlyphIndices(cps.as_ptr(), 1, indices.as_mut_ptr()) };
        if hr.is_err() || indices[0] == 0 {
            return None;
        }
        let mut gm = [DWRITE_GLYPH_METRICS::default(); 1];
        // SAFETY: gid + gm both length 1; horizontal mode.
        let hr = unsafe { face.GetDesignGlyphMetrics(indices.as_ptr(), 1, gm.as_mut_ptr(), false) };
        if hr.is_err() {
            return None;
        }
        let m = gm[0];
        if self.primary_upm <= 0.0 {
            return None;
        }
        let du_to_px = self.font_size_px / self.primary_upm;
        let ink_w_du = (m.advanceWidth as i32 - m.leftSideBearing - m.rightSideBearing) as f32;
        let ink_h_du = (m.advanceHeight as i32 - m.topSideBearing - m.bottomSideBearing) as f32;
        if ink_w_du <= 0.0 || ink_h_du <= 0.0 {
            return None;
        }
        Some(GlyphMetricsPx {
            ink_w_px: ink_w_du * du_to_px,
            ink_h_px: ink_h_du * du_to_px,
            lsb_px: (m.leftSideBearing as f32) * du_to_px,
        })
    }

    /// Evict every cached glyph and reset the skyline allocator without
    /// touching the text formats. Used by the renderer when a frame's
    /// glyph set exceeds the atlas capacity: drop the old set, try
    /// again. Re-rasterizing the live frame is O(cells) and cheap
    /// compared to the D3D stall a texture recreate would cause.
    pub fn purge(&mut self) {
        self.entries.clear();
        self.allocator = AtlasAllocator::new(size2(self.atlas_size as i32, self.atlas_size as i32));
        // SAFETY: d2d_rt owned by self and aliases the atlas texture.
        // Re-clear so stale glyph coverage doesn't bleed into freshly-
        // allocated slots.
        unsafe {
            self.d2d_rt.BeginDraw();
            self.d2d_rt.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }));
            let _ = self.d2d_rt.EndDraw(None, None);
        }
    }

    pub fn rebuild(
        &mut self,
        font_family: &str,
        font_fallback_families: &[String],
        font_size_px: f32,
    ) -> Result<()> {
        // Resolve both values together. In particular, the collection must
        // change when settings switch between a bundled and a system font.
        let resolved = resolve_font_family(
            &self.dwrite,
            self.bundled_font_collection.as_ref(),
            font_family,
        )?;
        let resolved_family = resolved.family;
        let font_collection = resolved.collection.cloned();
        let collection = font_collection.as_ref();
        let font_fallback = super::font_loader::font_fallback(
            &self.dwrite,
            self.bundled_font_collection.as_ref(),
            font_fallback_families,
        );

        // Build every fallible DirectWrite object before mutating the cache,
        // leaving the previous font fully usable if the requested font fails.
        let text_format_regular = make_text_format(
            &self.dwrite,
            collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            false,
            false,
        )?;
        let text_format_bold = make_text_format(
            &self.dwrite,
            collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            true,
            false,
        )?;
        let text_format_italic = make_text_format(
            &self.dwrite,
            collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            false,
            true,
        )?;
        let text_format_bold_italic = make_text_format(
            &self.dwrite,
            collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            true,
            true,
        )?;
        let text_format_cjk_regular = make_text_format_with_weight(
            &self.dwrite,
            collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            DWRITE_FONT_WEIGHT_MEDIUM,
            false,
        )?;
        let text_format_cjk_italic = make_text_format_with_weight(
            &self.dwrite,
            collection,
            font_fallback.as_ref(),
            &resolved_family,
            font_size_px,
            DWRITE_FONT_WEIGHT_MEDIUM,
            true,
        )?;
        let metrics = measure_cell(&self.dwrite, collection, &resolved_family, font_size_px)?;
        let layout_baselines = TextFormatBaselines::measure(
            &self.dwrite,
            metrics.baseline_px as f32,
            &text_format_regular,
            &text_format_bold,
            &text_format_italic,
            &text_format_bold_italic,
        );
        // Refresh the primary face + upm so per-glyph scale-to-fit
        // works after a font-size change (the face itself doesn't
        // depend on size, but re-resolving keeps the field in lockstep
        // with the current collection / family).
        let (primary_face, primary_upm) =
            match resolve_font_face(&self.dwrite, collection, &resolved_family) {
                Ok((face, _src)) => {
                    let mut fm = DWRITE_FONT_METRICS::default();
                    // SAFETY: windows-rs writes through the out pointer.
                    unsafe { face.GetMetrics(&mut fm) };
                    (Some(face), fm.designUnitsPerEm as f32)
                }
                Err(err) => {
                    log::warn!(
                        "GlyphCache::rebuild: primary face resolution failed ({err:?}); \
                     wide Nerd-Font icons will render clipped at the cell edge"
                    );
                    (None, 1.0)
                }
            };

        self.font_family = resolved_family;
        self.font_collection = font_collection;
        self.font_fallback = font_fallback;
        self.font_size_px = font_size_px;
        self.entries.clear();
        self.allocator = AtlasAllocator::new(size2(self.atlas_size as i32, self.atlas_size as i32));
        self.text_format_regular = text_format_regular;
        self.text_format_bold = text_format_bold;
        self.text_format_italic = text_format_italic;
        self.text_format_bold_italic = text_format_bold_italic;
        self.text_format_cjk_regular = text_format_cjk_regular;
        self.text_format_cjk_italic = text_format_cjk_italic;
        self.metrics = metrics;
        self.layout_baselines = layout_baselines;
        self.primary_face = primary_face;
        self.primary_upm = primary_upm;

        // Clear the atlas texture so fresh rasterization doesn't blend
        // with stale pixels from the previous size. BeginDraw/Clear/End
        // is a single D2D op — the DXGI-backed RT aliases the same
        // texture the D3D11 pixel shader samples, so the clear is
        // visible to the next frame.
        //
        // Black (not transparent) — D2D blends glyph coverage against
        // the RT's current pixels. Starting from (0,0,0) + drawing with
        // a white brush yields atlas values equal to coverage, which is
        // what the PS expects.
        // SAFETY: d2d_rt is owned by self and targets the atlas texture.
        unsafe {
            self.d2d_rt.BeginDraw();
            let black = D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            };
            self.d2d_rt.Clear(Some(&black));
            let _ = self.d2d_rt.EndDraw(None, None);
        }
        let _ = &self.device; // placeholder use so `device` field isn't dead
        Ok(())
    }
}

fn make_text_format(
    dwrite: &IDWriteFactory,
    collection: Option<&IDWriteFontCollection>,
    fallback: Option<&IDWriteFontFallback>,
    family: &str,
    size_px: f32,
    bold: bool,
    italic: bool,
) -> Result<IDWriteTextFormat> {
    let weight = if bold {
        DWRITE_FONT_WEIGHT_BOLD
    } else {
        DWRITE_FONT_WEIGHT_NORMAL
    };
    make_text_format_with_weight(
        dwrite, collection, fallback, family, size_px, weight, italic,
    )
}

fn make_text_format_with_weight(
    dwrite: &IDWriteFactory,
    collection: Option<&IDWriteFontCollection>,
    fallback: Option<&IDWriteFontFallback>,
    family: &str,
    size_px: f32,
    weight: DWRITE_FONT_WEIGHT,
    italic: bool,
) -> Result<IDWriteTextFormat> {
    let family_w: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
    let locale_w = &*USER_LOCALE_NAME;

    let style = if italic {
        DWRITE_FONT_STYLE_ITALIC
    } else {
        DWRITE_FONT_STYLE_NORMAL
    };

    // Pass a private collection only when the resolved family lives
    // there. User-selected system fonts must use `None`; otherwise
    // DirectWrite looks only inside our bundled collection and silently
    // falls through a different fallback path than the metrics probe.
    // SAFETY: both buffers are NUL-terminated wide strings.
    let format = unsafe {
        dwrite.CreateTextFormat(
            windows::core::PCWSTR(family_w.as_ptr()),
            collection,
            weight,
            style,
            DWRITE_FONT_STRETCH_NORMAL,
            size_px,
            windows::core::PCWSTR(locale_w.as_ptr()),
        )
    }
    .context("CreateTextFormat failed")?;

    // Attach the custom fallback cascade via IDWriteTextFormat1 (Win8.1+).
    // Treat failure as a format-construction error rather than silently
    // displaying missing-glyph boxes while claiming the setting was applied.
    if let Some(fb) = fallback {
        let fmt1 = format
            .cast::<IDWriteTextFormat1>()
            .context("IDWriteTextFormat1 is unavailable for font fallback")?;
        // SAFETY: fmt1 and fb are live COM references owned by this cache.
        unsafe { fmt1.SetFontFallback(fb) }.context("SetFontFallback failed")?;
    }

    Ok(format)
}

fn text_rendering_params(
    dwrite: &IDWriteFactory,
) -> (Option<IDWriteRenderingParams>, GlyphRenderingParams) {
    match foreground_aware_text_rendering_params(dwrite) {
        Ok(params) => params,
        Err(error) => {
            // Windows 10+ exposes IDWriteFactory1, but preserve a usable
            // terminal if a remote/compatibility environment rejects it. In
            // that case DirectWrite bakes correction into the atlas and the
            // shader's identity parameters avoid correcting it twice.
            log::warn!(
                "foreground-aware DirectWrite correction unavailable ({error:#}); using legacy atlas correction"
            );
            let legacy = unsafe {
                dwrite.CreateCustomRenderingParams(
                    DEFAULT_TEXT_GAMMA,
                    1.15,
                    0.0,
                    DWRITE_PIXEL_GEOMETRY_FLAT,
                    DWRITE_RENDERING_MODE_NATURAL_SYMMETRIC,
                )
            }
            .ok();
            (legacy, GlyphRenderingParams::LEGACY_ATLAS)
        }
    }
}

fn foreground_aware_text_rendering_params(
    dwrite: &IDWriteFactory,
) -> Result<(Option<IDWriteRenderingParams>, GlyphRenderingParams)> {
    // This follows Windows Terminal's AtlasEngine contract: read the user's
    // DirectWrite parameters once, rasterize neutral grayscale coverage, and
    // defer color-dependent gamma/contrast correction to the pixel shader.
    let factory = dwrite
        .cast::<IDWriteFactory1>()
        .context("IDWriteFactory1 is unavailable")?;
    let defaults = unsafe { factory.CreateRenderingParams() }
        .context("IDWriteFactory1::CreateRenderingParams failed")?;
    let defaults_v1 = defaults
        .cast::<IDWriteRenderingParams1>()
        .context("IDWriteRenderingParams1 is unavailable")?;

    let gamma = finite_or(unsafe { defaults.GetGamma() }, DEFAULT_TEXT_GAMMA).clamp(1.0, 2.2);
    let grayscale_contrast = finite_or(
        unsafe { defaults_v1.GetGrayscaleEnhancedContrast() },
        DEFAULT_GRAYSCALE_CONTRAST,
    )
    .max(0.0);
    let params = unsafe {
        factory.CreateCustomRenderingParams(
            1.0,
            0.0,
            0.0,
            defaults.GetClearTypeLevel(),
            defaults.GetPixelGeometry(),
            defaults.GetRenderingMode(),
        )
    }
    .context("IDWriteFactory1::CreateCustomRenderingParams failed")?;
    let params = params
        .cast::<IDWriteRenderingParams>()
        .context("IDWriteRenderingParams1 -> IDWriteRenderingParams failed")?;

    Ok((
        Some(params),
        GlyphRenderingParams {
            gamma_ratios: gamma_ratios(gamma),
            grayscale_contrast,
            cjk_grayscale_contrast: grayscale_contrast.max(CJK_GRAYSCALE_CONTRAST_FLOOR),
        },
    ))
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

/// Polynomial coefficients used by DirectWrite's grayscale alpha correction.
/// The source table and normalization are from Windows Terminal's
/// MIT-licensed `dwrite_helpers.cpp`.
fn gamma_ratios(gamma: f32) -> [f32; 4] {
    const TARGETS: [[f32; 4]; 13] = [
        [0.0000, 0.0000, 0.0000, 0.0000],
        [0.0166, -0.0807, 0.2227, -0.0751],
        [0.0350, -0.1760, 0.4325, -0.1370],
        [0.0543, -0.2821, 0.6302, -0.1876],
        [0.0739, -0.3963, 0.8167, -0.2287],
        [0.0933, -0.5161, 0.9926, -0.2616],
        [0.1121, -0.6395, 1.1588, -0.2877],
        [0.1300, -0.7649, 1.3159, -0.3080],
        [0.1469, -0.8911, 1.4644, -0.3234],
        [0.1627, -1.0170, 1.6051, -0.3347],
        [0.1773, -1.1420, 1.7385, -0.3426],
        [0.1908, -1.2652, 1.8650, -0.3476],
        [0.2031, -1.3864, 1.9851, -0.3501],
    ];
    let index =
        ((finite_or(gamma, DEFAULT_TEXT_GAMMA) * 10.0 + 0.5) as i32).clamp(10, 22) as usize - 10;
    let target = TARGETS[index];
    let norm_13 = 65_536.0 / (255.0 * 255.0);
    let norm_24 = 256.0 / 255.0;
    [
        target[0] * norm_13,
        target[1] * norm_24,
        target[2] * norm_13,
        target[3] * norm_24,
    ]
}

fn text_layout_baseline(
    dwrite: &IDWriteFactory,
    format: &IDWriteTextFormat,
    utf16: &[u16],
) -> Option<f32> {
    if utf16.is_empty() {
        return None;
    }
    // SAFETY: `utf16` lives for the call; DirectWrite copies the text into
    // the layout object. Infinite bounds mirror GPUI/Zed's Windows text
    // layout path and avoid wrapping a single glyph.
    let layout = unsafe {
        dwrite
            .CreateTextLayout(utf16, format, f32::INFINITY, f32::INFINITY)
            .ok()?
    };
    let mut line_metrics = [DWRITE_LINE_METRICS::default(); 1];
    let mut line_count = 0u32;
    // SAFETY: output buffer contains one slot; a one-glyph layout has at
    // most one line. If DirectWrite reports no line, caller falls back.
    unsafe {
        layout
            .GetLineMetrics(Some(&mut line_metrics), &mut line_count)
            .ok()?;
    }
    (line_count > 0).then_some(line_metrics[0].baseline)
}

fn measure_cell(
    dwrite: &IDWriteFactory,
    collection: Option<&IDWriteFontCollection>,
    family: &str,
    font_size_px: f32,
) -> Result<CellMetrics> {
    // Resolve the advance directly from the font's design metrics.
    //
    // We previously used IDWriteTextLayout::GetMetrics, which returns
    // layout-rounded dimensions from whichever font DWrite's shaper
    // eventually picked. When our bundled IoskeleyMono collection
    // silently fails to match the requested family name (e.g. a
    // mis-patched `name` table or unsupported cast path), DWrite
    // happily falls back to the system collection and returns the
    // width of Segoe UI's 'M' glyph — that's ~28 px at font size 28,
    // roughly 1.6× IoskeleyMono's actual advance, and the whole grid
    // stretches visibly ("W i n d o w s  PowerShell" instead of
    // tightly packed).
    //
    // Design metrics bypass the layout engine entirely: we get the
    // literal advanceWidth stored in the TTF 'hmtx' table, converted
    // from design units to pixels via the font's UPM. For a strict
    // monospace font every glyph has the same advance, so measuring
    // 'M' is representative; a missing-glyph fallback doesn't factor
    // in.
    let (face, resolved) = resolve_font_face(dwrite, collection, family)?;

    // Font-level design metrics (UPM, ascent, descent, lineGap).
    // SAFETY: windows-rs signature takes a raw out pointer; we pass a
    // sized stack slot.
    let mut fm = DWRITE_FONT_METRICS::default();
    unsafe { face.GetMetrics(&mut fm) };
    let upm = fm.designUnitsPerEm as f32;

    // Resolve 'M' to its glyph index in this face.
    let codepoints: [u32; 1] = ['M' as u32];
    let mut indices: [u16; 1] = [0];
    // SAFETY: both arrays sized 1; DWrite writes one glyph index.
    unsafe { face.GetGlyphIndices(codepoints.as_ptr(), 1, indices.as_mut_ptr()) }
        .context("IDWriteFontFace::GetGlyphIndices failed")?;

    let mut gm = [DWRITE_GLYPH_METRICS::default(); 1];
    // SAFETY: indices and gm are both length 1; issideways=false picks
    // horizontal advance.
    unsafe { face.GetDesignGlyphMetrics(indices.as_ptr(), 1, gm.as_mut_ptr(), false) }
        .context("IDWriteFontFace::GetDesignGlyphMetrics failed")?;

    let advance_du = gm[0].advanceWidth as f32;
    let cell_width_px = ((advance_du * font_size_px) / upm).ceil().max(1.0) as u32;
    // Height from ascent + descent + lineGap, converted to pixels.
    // lineGap can legitimately be negative on some fonts; clamp below.
    let line_metric_du = fm.ascent as f32 + fm.descent as f32 + fm.lineGap as f32;
    let cell_height_px = ((line_metric_du * font_size_px) / upm).ceil().max(1.0) as u32;
    let baseline_px = ((fm.ascent as f32 * font_size_px) / upm).ceil() as u32;

    log::info!(
        "measure_cell: resolved='{}' upm={} M_advance_du={} glyph_idx={} \
         -> cell {}x{} baseline={} @ {}px",
        resolved,
        upm as u32,
        advance_du as u32,
        indices[0],
        cell_width_px,
        cell_height_px,
        baseline_px,
        font_size_px,
    );

    Ok(CellMetrics {
        cell_width_px,
        cell_height_px,
        baseline_px,
    })
}

/// Resolve `family` to a concrete [`IDWriteFontFace`] for measurement.
///
/// Order:
/// 1. The resolved private collection, if any (our IoskeleyMono blobs).
/// 2. System collection.
/// 3. System collection → "Segoe UI" (last-resort so we return *some*
///    metrics instead of bailing the entire render setup).
///
/// Returns the face plus a human-readable "where it came from" string
/// so the log line makes the fallback path obvious when we're debugging
/// "cells are twice as wide as they should be" issues.
fn resolve_font_face(
    dwrite: &IDWriteFactory,
    collection: Option<&IDWriteFontCollection>,
    family: &str,
) -> Result<(IDWriteFontFace, String)> {
    if let Some(coll) = collection {
        if let Some(face) = find_face_in_collection(coll, family)? {
            return Ok((face, format!("{family} (private collection)")));
        }
        log::warn!(
            "measure_cell: '{family}' not found in private font collection; \
             falling back to system collection"
        );
    }

    let sys = system_font_collection(dwrite)?;

    if let Some(face) = find_face_in_collection(&sys, family)? {
        return Ok((face, format!("{family} (system)")));
    }

    log::warn!(
        "measure_cell: '{family}' not found in system collection either; \
         using Segoe UI for metrics (cells will be sized for Segoe UI, not {family})"
    );
    if let Some(face) = find_face_in_collection(&sys, "Segoe UI")? {
        return Ok((face, "Segoe UI (last-resort fallback)".to_string()));
    }

    anyhow::bail!(
        "measure_cell: neither '{family}' nor 'Segoe UI' resolved in any font collection"
    );
}

fn find_face_in_collection(
    collection: &IDWriteFontCollection,
    family: &str,
) -> Result<Option<IDWriteFontFace>> {
    let family_w: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
    let mut index: u32 = 0;
    let mut exists = windows::core::BOOL(0);
    // SAFETY: buffers sized above; out params are stack-local.
    unsafe {
        collection.FindFamilyName(
            windows::core::PCWSTR(family_w.as_ptr()),
            &mut index,
            &mut exists,
        )
    }
    .context("IDWriteFontCollection::FindFamilyName failed")?;
    if !exists.as_bool() {
        return Ok(None);
    }

    // SAFETY: index is valid per DWrite contract when exists=TRUE.
    let family_obj = unsafe { collection.GetFontFamily(index) }.context("GetFontFamily failed")?;
    // SAFETY: we want regular/normal for the measurement probe — bold
    // and italic have the same advance on a monospace face, so the
    // regular weight is representative and always present.
    let font = unsafe {
        family_obj.GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )
    }
    .context("GetFirstMatchingFont failed")?;
    // SAFETY: font owned above.
    let face = unsafe { font.CreateFontFace() }.context("IDWriteFont::CreateFontFace failed")?;
    Ok(Some(face))
}

struct ResolvedFontFamily<'a> {
    family: String,
    collection: Option<&'a IDWriteFontCollection>,
}

/// Pick the concrete DirectWrite family and collection used by both
/// `CreateTextFormat` and metric probing.
///
/// The config/UI default is the display name `"Ioskeley Mono"`, while
/// the bundled TTFs advertise `"IoskeleyMono"`. For bundled fonts we
/// must pass the private collection. For every user-selected system
/// font we must pass `None`, otherwise DirectWrite searches only the
/// bundled collection and draws via a fallback that no longer matches
/// the metrics.
fn resolve_font_family<'a>(
    dwrite: &IDWriteFactory,
    bundled: Option<&'a IDWriteFontCollection>,
    family: &str,
) -> Result<ResolvedFontFamily<'a>> {
    let trimmed = family.trim();
    let requested = if trimmed.is_empty() {
        "Segoe UI"
    } else {
        trimmed
    };
    let stripped: String = requested.chars().filter(|c| !c.is_whitespace()).collect();

    if let Some(coll) = bundled {
        if family_exists_in(coll, requested) {
            log::info!("resolve_font_family: '{requested}' matched bundled collection");
            return Ok(ResolvedFontFamily {
                family: requested.to_string(),
                collection: Some(coll),
            });
        }
        if stripped != requested && family_exists_in(coll, &stripped) {
            log::info!(
                "resolve_font_family: '{requested}' missed; '{stripped}' matched bundled \
                 collection — using that for DWrite lookups"
            );
            return Ok(ResolvedFontFamily {
                family: stripped,
                collection: Some(coll),
            });
        }
    }

    let sys = system_font_collection(dwrite)?;
    if family_exists_in(&sys, requested) {
        log::info!("resolve_font_family: '{requested}' matched system collection");
        return Ok(ResolvedFontFamily {
            family: requested.to_string(),
            collection: None,
        });
    }
    if stripped != requested && family_exists_in(&sys, &stripped) {
        log::info!(
            "resolve_font_family: '{requested}' missed; '{stripped}' matched system collection"
        );
        return Ok(ResolvedFontFamily {
            family: stripped,
            collection: None,
        });
    }

    let fallback = system_monospace_fallback(&sys).unwrap_or("Segoe UI");
    if family_exists_in(&sys, fallback) {
        log::warn!(
            "resolve_font_family: '{requested}' not found in bundled or system collections; \
             using '{fallback}' so text formats and metrics stay consistent"
        );
        return Ok(ResolvedFontFamily {
            family: fallback.to_string(),
            collection: None,
        });
    }

    anyhow::bail!("resolve_font_family: neither '{requested}' nor fallback '{fallback}' resolved");
}

fn system_monospace_fallback(sys: &IDWriteFontCollection) -> Option<&'static str> {
    [
        "Cascadia Mono",
        "Cascadia Code",
        "Consolas",
        "Lucida Console",
        "Courier New",
        "Segoe UI",
    ]
    .into_iter()
    .find(|family| family_exists_in(sys, family))
}

fn system_font_collection(dwrite: &IDWriteFactory) -> Result<IDWriteFontCollection> {
    let mut sys: Option<IDWriteFontCollection> = None;
    // SAFETY: out param owned by us; `checkforupdates=false` is the
    // cheap path — we don't care if the system font list changed.
    unsafe { dwrite.GetSystemFontCollection(&mut sys, false) }
        .context("IDWriteFactory::GetSystemFontCollection failed")?;
    sys.context("GetSystemFontCollection returned None")
}

fn family_exists_in(collection: &IDWriteFontCollection, family: &str) -> bool {
    let family_w: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
    let mut index: u32 = 0;
    let mut exists = windows::core::BOOL(0);
    // SAFETY: buffer is NUL-terminated; out params are stack-local. A
    // non-Ok HRESULT here is treated as "not found"; we don't want to
    // propagate it because the caller has a well-defined fallback.
    let hr = unsafe {
        collection.FindFamilyName(
            windows::core::PCWSTR(family_w.as_ptr()),
            &mut index,
            &mut exists,
        )
    };
    hr.is_ok() && exists.as_bool()
}

fn is_wide_codepoint(codepoint: u32) -> bool {
    char::from_u32(codepoint)
        .and_then(UnicodeWidthChar::width)
        .is_some_and(|width| width >= 2)
}

pub(super) fn is_cjk_codepoint(codepoint: u32) -> bool {
    matches!(
        codepoint,
        // Hangul Jamo.
        0x1100..=0x11FF
            // CJK radicals, Kangxi radicals, ideographic description chars.
            | 0x2E80..=0x2FFF
            // CJK punctuation, Hiragana, Katakana, Bopomofo, Hangul compatibility.
            | 0x3000..=0x318F
            // CJK strokes, Katakana extensions, enclosed CJK.
            | 0x31C0..=0x32FF
            // CJK Unified Ideographs Extension A + Unified Ideographs.
            | 0x3400..=0x9FFF
            // Hangul syllables.
            | 0xAC00..=0xD7AF
            // CJK compatibility ideographs.
            | 0xF900..=0xFAFF
            // Kana Supplement / Extended, Small Kana, Nushu.
            | 0x1AFF0..=0x1B16F
            // Ideographic symbols and Tangut/Nushu-era East Asian wide scripts.
            | 0x16FE0..=0x18D8F
            // CJK Extension B and later assigned extension planes.
            | 0x20000..=0x323AF
    )
}

#[cfg(test)]
mod tests {
    use super::{gamma_ratios, is_cjk_codepoint, is_wide_codepoint};

    #[test]
    fn gamma_ratios_match_directwrite_default() {
        let actual = gamma_ratios(1.8);
        let expected = [0.148054421, -0.894594550, 1.47590804, -0.324668258];
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 0.000_001);
        }
    }

    #[test]
    fn wide_codepoint_follows_unicode_width() {
        // These ranges were easy to miss in a hand-written East Asian
        // Width table. Keep the atlas decision delegated to
        // unicode-width so newly covered Unicode ranges do not regress
        // into one-cell clipped glyphs.
        for codepoint in [
            0x4E00,  // CJK Unified Ideograph
            0xA960,  // Hangul Jamo Extended-A
            0x1B000, // Kana Supplement
            0x16FE0, // Tangut / ideographic marks
            0x1B170, // Nushu
            0x1FA70, // Symbols and Pictographs Extended-A
        ] {
            assert!(is_wide_codepoint(codepoint), "U+{codepoint:04X}");
        }

        // Ambiguous-width punctuation should stay one cell unless the
        // terminal layer explicitly gains a CJK-ambiguous-width mode.
        assert!(!is_wide_codepoint(0x00B7));
        // Hangul vowel and trailing jamo combine with a preceding consonant;
        // unicode-width therefore assigns them zero columns rather than two.
        assert!(!is_wide_codepoint(0xD7B0));
    }

    #[test]
    fn cjk_codepoint_detection_is_limited_to_east_asian_text() {
        for codepoint in [
            0x4E00,  // CJK Unified Ideograph
            0x3002,  // Ideographic full stop
            0x3042,  // Hiragana
            0x30A2,  // Katakana
            0x3105,  // Bopomofo
            0xAC00,  // Hangul syllable
            0x20000, // CJK Unified Ideographs Extension B
        ] {
            assert!(is_cjk_codepoint(codepoint), "U+{codepoint:04X}");
        }

        assert!(!is_cjk_codepoint('A' as u32));
        assert!(!is_cjk_codepoint(0xE0B0)); // Powerline glyph; keep prompt icons untouched.
    }
}
