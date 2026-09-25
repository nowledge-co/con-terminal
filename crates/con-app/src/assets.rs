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

        if let Some(name) = path.strip_prefix("app-icons/") {
            if let Some(data) = AppIcons::get(name) {
                return Ok(Some(data.data));
            }
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
    if let Some(name) = asset_path.strip_prefix("app-icons/") {
        if let Some(file) = AppIcons::get(name) {
            return Some(file.data);
        }
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
