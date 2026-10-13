use gpui::{
    App, Asset, AssetSource, ImageCacheError, ImageSource, RenderImage, Result, SharedString,
};
use std::borrow::Cow;
use std::sync::Arc;

/// Embeds con's own icons (Phosphor) from `assets/icons/`.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets/icons"]
#[include = "**/*.svg"]
struct ConIcons;

/// Embeds top-level app images such as the macOS app icon PNG.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets"]
#[include = "*.png"]
struct ConImages;

/// Alternate selectable raccoon app icons.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../assets/app-icons"]
#[include = "*.png"]
struct AppIcons;

/// Asset source that serves con's icons first, then falls back to
/// gpui-component's bundled icons (Lucide).
pub struct ConAssets;

impl AssetSource for ConAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        // Try con's own icons first
        if let Some(data) = ConIcons::get(path) {
            return Ok(Some(data.data));
        }

        if let Some(name) = path.strip_prefix("app-icons/")
            && let Some(data) = AppIcons::get(name)
        {
            return Ok(Some(data.data));
        }

        if let Some(data) = ConImages::get(path) {
            return Ok(Some(data.data));
        }

        // Fall back to gpui-component's bundled assets
        gpui_kit_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut results: Vec<SharedString> = ConIcons::iter()
            .filter(|p| p.starts_with(path))
            .map(|p| p.into())
            .collect();

        results.extend(
            AppIcons::iter()
                .map(|p| SharedString::from(format!("app-icons/{p}")))
                .filter(|p| p.starts_with(path)),
        );

        results.extend(
            ConImages::iter()
                .filter(|p| p.starts_with(path))
                .map(|p| p.into()),
        );

        if let Ok(mut component_results) = gpui_kit_assets::Assets.list(path) {
            results.append(&mut component_results);
        }

        Ok(results)
    }
}

pub fn png_bytes(asset_path: &str) -> Option<Cow<'static, [u8]>> {
    if let Some(name) = asset_path.strip_prefix("app-icons/")
        && let Some(file) = AppIcons::get(name)
    {
        return Some(file.data);
    }
    ConImages::get(asset_path).map(|file| file.data)
}

/// Downsample small picker images before uploading them to GPUI's single-level atlas.
/// Cache by physical size so moving between displays does not reuse a blurry 1x image.
pub fn png_preview(path: &'static str, logical_size: f32) -> ImageSource {
    ImageSource::Custom(Arc::new(move |window, cx| {
        let size = (logical_size * window.scale_factor()).round().max(1.0) as u32;
        window.use_asset::<PngPreview>(&(path, size), cx)
    }))
}

/// Tab identities may be small bitmap objects; action/status glyphs stay SVG.
pub(crate) fn tab_identity_icon(
    path: &'static str,
    size: gpui::Pixels,
    color: gpui::Hsla,
) -> gpui::AnyElement {
    use gpui::{IntoElement, Styled, StyledImage};
    if path.ends_with(".png") {
        gpui::img(png_preview(path, size.as_f32()))
            .size(size)
            .object_fit(gpui::ObjectFit::Contain)
            .flex_shrink_0()
            .into_any_element()
    } else {
        gpui::svg()
            .path(path)
            .size(size)
            .flex_shrink_0()
            .text_color(color)
            .into_any_element()
    }
}

struct PngPreview;

impl Asset for PngPreview {
    type Source = (&'static str, u32);
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    #[expect(
        clippy::manual_async_fn,
        reason = "async fn captures the non-Send App context"
    )]
    fn load(
        (path, size): Self::Source,
        _cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        async move {
            let bytes = png_bytes(path).ok_or_else(|| anyhow::anyhow!("Missing PNG: {path}"))?;
            let pixels = preview_bgra(&bytes, size)?;
            Ok(Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])))
        }
    }
}

fn preview_bgra(bytes: &[u8], size: u32) -> Result<image::RgbaImage> {
    let mut pixels = image::load_from_memory(bytes)?.into_rgba32f();
    // Filter premultiplied alpha; invisible RGB must not bleed into curved edges.
    for pixel in pixels.pixels_mut() {
        for channel in 0..3 {
            pixel[channel] *= pixel[3];
        }
    }
    let mut pixels =
        image::imageops::resize(&pixels, size, size, image::imageops::FilterType::Lanczos3);
    for pixel in pixels.pixels_mut() {
        let alpha = pixel[3];
        for channel in 0..3 {
            pixel[channel] = if alpha > 0.0 {
                pixel[channel] / alpha
            } else {
                0.0
            };
        }
    }
    let mut pixels = image::DynamicImage::ImageRgba32F(pixels).into_rgba8();
    // RenderImage expects straight-alpha BGRA, despite the RgbaImage buffer type.
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, IntoElement, Render, Styled, Window, img, px};

    #[test]
    fn tab_identities_are_embedded_small_and_transparent() {
        for path in [
            "tab-terminal-refined.png",
            "tab-code.png",
            "tab-file.png",
            "tab-remote.png",
            "tab-monitor.png",
            "tab-reader.png",
            "input-mode-smart.png",
            "input-mode-agent.png",
            "input-mode-shell.png",
        ] {
            let bytes = png_bytes(path).expect("tab identity must ship in the binary");
            let image = image::load_from_memory(&bytes).unwrap().into_rgba8();
            assert_eq!(image.dimensions(), (256, 256));
            assert_eq!(image.get_pixel(0, 0)[3], 0, "{path} has an opaque backdrop");
            let solid = image.pixels().filter(|p| p[3] >= 240).count();
            let fringe = image.pixels().filter(|p| p[3] > 0 && p[3] < 200).count();
            assert!(solid > 4096, "{path} has insufficient solid coverage");
            assert!(fringe < solid / 4, "{path} has a diffuse alpha halo");
            for size in [16, 32] {
                let icon = preview_bgra(&bytes, size).unwrap();
                let solid = icon.pixels().filter(|p| p[3] >= 240).count();
                assert!(
                    solid > (size * size / 20) as usize,
                    "{path} is too faint at {size}px"
                );
                assert_eq!(icon.get_pixel(0, 0)[3], 0);
                assert!(
                    icon.pixels().filter(|p| p[3] == 0).count() > (size * size / 10) as usize,
                    "{path} loses its transparent silhouette at {size}px"
                );
            }
        }
    }

    struct PreviewView {
        loaded: std::rc::Rc<std::cell::Cell<bool>>,
    }

    impl Render for PreviewView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let ImageSource::Custom(load) = png_preview("app-icons/con-raccoon-A1.png", 48.0)
            else {
                unreachable!()
            };
            let loaded = self.loaded.clone();
            img(move |window: &mut Window, cx: &mut App| {
                let result = load(window, cx);
                loaded.set(matches!(result, Some(Ok(_))));
                result
            })
            .size(px(48.0))
        }
    }

    #[gpui::test]
    fn preview_notifies_a_view_joining_an_inflight_load(cx: &mut gpui::TestAppContext) {
        // Start loading without a live requesting view, as when its window closed.
        let _pending =
            cx.update(|cx| cx.fetch_asset::<PngPreview>(&("app-icons/con-raccoon-A1.png", 48)));
        let loaded = std::rc::Rc::new(std::cell::Cell::new(false));
        let window = cx.add_window(|_, _| PreviewView {
            loaded: loaded.clone(),
        });
        let window: gpui::AnyWindowHandle = window.into();
        window
            .update(cx, |_, window, cx| {
                let _ = window.draw(cx);
            })
            .unwrap();
        assert!(
            !loaded.get(),
            "must exercise an in-flight load, not a warm cache"
        );
        cx.run_until_parked();
        assert!(
            loaded.get(),
            "joining view was not redrawn after the shared load completed"
        );
    }

    #[test]
    fn previews_filter_detail_without_transparent_color_bleeding() {
        // Half opaque red, half invisible cyan. Point sampling aliases to an
        // endpoint; filtering straight alpha would incorrectly mix in cyan.
        let source = image::RgbaImage::from_fn(32, 32, |x, _| {
            if x % 2 == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 255, 255, 0])
            }
        });
        let mut png = std::io::Cursor::new(Vec::new());
        source.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let preview = preview_bgra(png.get_ref(), 1).unwrap();
        let pixel = preview.get_pixel(0, 0).0;
        assert_eq!(&pixel[..3], &[0, 0, 255], "BGRA red without a cyan halo");
        assert!(
            (126..=129).contains(&pixel[3]),
            "coverage must average: {pixel:?}"
        );
    }

    #[test]
    fn app_icon_previews_match_physical_sizes_and_preserve_alpha() {
        let bytes = png_bytes("app-icons/con-raccoon-A1.png").unwrap();
        for size in [48, 60, 72, 96, 144] {
            let preview = preview_bgra(&bytes, size).unwrap();
            assert_eq!(preview.dimensions(), (size, size));
            assert!(preview.pixels().any(|pixel| pixel[3] == 0));
            assert!(preview.pixels().any(|pixel| pixel[3] == 255));
            assert!(preview.pixels().any(|pixel| (1..255).contains(&pixel[3])));
        }
    }
}
