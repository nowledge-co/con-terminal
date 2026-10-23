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

/// One identity catalog for horizontal tabs, the sidebar and drag previews.
pub(crate) fn tab_identity_asset(icon: &'static str) -> &'static str {
    match icon {
        "phosphor/terminal.svg" | "phosphor/terminal-window.svg" => "tab-terminal-refined.png",
        "phosphor/code.svg" => "tab-code.png",
        "phosphor/file-code.svg" => "tab-file.png",
        "phosphor/globe.svg" => "tab-remote.png",
        "phosphor/pulse.svg" => "tab-monitor.png",
        "phosphor/book-open.svg" => "tab-reader.png",
        _ => icon,
    }
}

/// Normalize aliases before deduplicating so one application occupies one tile.
pub(crate) fn unique_tab_identities(
    icons: impl IntoIterator<Item = &'static str>,
    primary: &'static str,
) -> Vec<&'static str> {
    let mut identities = Vec::new();
    for icon in icons {
        let asset = tab_identity_asset(icon);
        if !identities.contains(&asset) {
            identities.push(asset);
        }
    }
    // Match the tab's current identity only when it names real pane content;
    // an AI-chosen decorative icon must not invent another application.
    let primary = tab_identity_asset(primary);
    if let Some(index) = identities.iter().position(|asset| *asset == primary) {
        identities[..=index].rotate_right(1);
    }
    identities
}

fn tab_identity_card_step(size: gpui::Pixels, compact: bool) -> gpui::Pixels {
    gpui::px((size.as_f32() * if compact { 0.44 } else { 0.56 }).round())
}

pub(crate) fn tab_identity_group_width(
    count: usize,
    size: gpui::Pixels,
    compact: bool,
) -> gpui::Pixels {
    if count <= 1 {
        size
    } else {
        size + tab_identity_card_step(size, compact)
    }
}

/// One primary app and one secondary identity keep multi-pane tabs legible.
pub(crate) fn tab_identity_group(
    icons: &[&'static str],
    size: gpui::Pixels,
    color: gpui::Hsla,
    compact: bool,
) -> gpui::AnyElement {
    use gpui::{InteractiveElement, IntoElement, ParentElement, Styled};
    if icons.len() <= 1 {
        return tab_identity_icon(
            tab_identity_asset(icons.first().copied().unwrap_or("phosphor/terminal.svg")),
            size,
            color,
        );
    }
    let count = icons.len().min(2);
    let step = tab_identity_card_step(size, compact);
    let mut group = gpui::div()
        .debug_selector(|| "tab-identity-group".into())
        .relative()
        .w(tab_identity_group_width(icons.len(), size, compact))
        .h(size)
        .flex_shrink_0();
    for index in (0..count).rev() {
        let icon = icons[index];
        let card_size = if index == 0 {
            size
        } else {
            gpui::px((size.as_f32() * 0.8).round())
        };
        let card = gpui::div()
            .debug_selector(move || format!("tab-identity-item-{index}"))
            .absolute()
            .left(step * (count - index - 1) as f32)
            .top_0()
            .size(card_size)
            .flex_shrink_0();
        group = group.child(
            card.child(
                gpui::div()
                    .debug_selector(move || format!("tab-identity-art-{index}"))
                    .size(card_size)
                    .flex_shrink_0()
                    .child(tab_identity_icon(
                        tab_identity_asset(icon),
                        card_size,
                        color,
                    )),
            ),
        );
    }
    group.into_any_element()
}

/// Tab identities may be small bitmap objects; action/status glyphs stay SVG.
pub(crate) fn tab_identity_icon(
    path: &'static str,
    size: gpui::Pixels,
    color: gpui::Hsla,
) -> gpui::AnyElement {
    use gpui::{IntoElement, Styled, StyledImage};
    if let Some(glyph) = crate::terminal_identity::glyph(path) {
        gpui::canvas(
            move |bounds, window, _| {
                let (font_size, ink) = crate::terminal_identity::glyph_geometry(glyph, size)?;
                let text = SharedString::from(glyph.to_string());
                let line = window.text_system().shape_line(
                    text.clone(),
                    font_size,
                    &[gpui::TextRun {
                        len: text.len(),
                        font: gpui::font(crate::file_icons::FILE_ICON_FONT_FAMILY),
                        color: crate::terminal_identity::color(path),
                        ..Default::default()
                    }],
                    None,
                );
                let baseline = (size - line.ascent - line.descent) * 0.5 + line.ascent;
                let origin = gpui::point(
                    bounds.center().x - ink.center().x,
                    bounds.center().y + ink.center().y - baseline,
                );
                Some((line, origin))
            },
            move |_, shaped, window, cx| {
                if let Some((line, origin)) = shaped {
                    let _ = line.paint(origin, size, gpui::TextAlign::Left, None, window, cx);
                }
            },
        )
        .size(size)
        .flex_shrink_0()
        .into_any_element()
    } else if path.ends_with(".png") {
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
    #[test]
    fn pane_identities_deduplicate_final_assets_in_layout_order() {
        assert_eq!(
            super::unique_tab_identities(
                [
                    "phosphor/terminal.svg",
                    "phosphor/code.svg",
                    "phosphor/terminal-window.svg",
                    "agents/opencode.svg",
                    "phosphor/code.svg",
                    "phosphor/file-code.svg",
                ],
                "phosphor/terminal-window.svg"
            ),
            vec![
                "tab-terminal-refined.png",
                "tab-code.png",
                "agents/opencode.svg",
                "tab-file.png"
            ]
        );
        assert_eq!(
            super::unique_tab_identities(
                [
                    "phosphor/terminal.svg",
                    "phosphor/code.svg",
                    "phosphor/file-code.svg"
                ],
                "phosphor/code.svg",
            ),
            vec!["tab-code.png", "tab-terminal-refined.png", "tab-file.png"]
        );
        assert_eq!(
            super::unique_tab_identities(["phosphor/terminal.svg"], "phosphor/globe.svg"),
            vec!["tab-terminal-refined.png"]
        );
    }
    use super::*;
    use gpui::{Context, IntoElement, Render, Styled, Window, img, px};

    struct IdentityGroupTestView {
        count: usize,
        compact: bool,
        size: f32,
    }

    impl Render for IdentityGroupTestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            tab_identity_group(
                &[
                    "phosphor/terminal.svg",
                    "phosphor/code.svg",
                    "phosphor/file-code.svg",
                    "phosphor/pulse.svg",
                ][..self.count],
                px(self.size),
                gpui::black(),
                self.compact,
            )
        }
    }

    #[gpui::test]
    fn multi_app_groups_show_two_whole_identities_without_growing_with_pane_count(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, _| IdentityGroupTestView {
            count: 2,
            compact: false,
            size: 18.0,
        });
        for size in [16.0, 18.0, 20.0] {
            for compact in [false, true] {
                for count in [2, 3, 4] {
                    view.update(cx, |view, cx| {
                        view.count = count;
                        view.compact = compact;
                        view.size = size;
                        cx.notify();
                    });
                    cx.run_until_parked();
                    let group = cx.debug_bounds("tab-identity-group").unwrap();
                    let first = cx.debug_bounds("tab-identity-item-0").unwrap();
                    let second = cx.debug_bounds("tab-identity-item-1").unwrap();
                    assert!(first.left() > second.left());
                    assert!(second.right() > first.left());
                    assert!(second.right() - first.left() <= second.size.width * 0.5);
                    assert!(first.center().y > second.center().y);
                    assert_eq!(first.size.width, px(size));
                    assert_eq!(first.right(), group.right());
                    assert!(second.left() >= group.left());
                    let rear_art = cx.debug_bounds("tab-identity-art-1").unwrap();
                    assert_eq!(rear_art.size.width, px((size * 0.8).round()));
                    assert_eq!(rear_art, second, "secondary art must not be cropped");
                    assert_eq!(
                        group.size.width,
                        tab_identity_group_width(2, px(size), compact)
                    );
                    assert!(cx.debug_bounds("tab-identity-more").is_none());
                    assert!(cx.debug_bounds("tab-identity-item-2").is_none());
                    assert!(cx.debug_bounds("tab-identity-item-3").is_none());
                }
            }
        }
    }

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
