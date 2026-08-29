use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

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
        gpui_component_assets::Assets.load(path)
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

        if let Ok(mut component_results) = gpui_component_assets::Assets.list(path) {
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
