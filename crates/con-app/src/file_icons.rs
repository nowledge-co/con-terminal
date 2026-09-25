//! Path → icon mapping for the file tree.
//!
//! An icon is either one of con's bundled Phosphor SVGs (`assets/icons/`) or a
//! Nerd Font glyph. Glyphs live in the Seti-UI + Custom private-use range, so
//! they only render with a Nerd Font. They are always drawn with con's bundled
//! IoskeleyMono rather than the user's terminal font, so icon availability is
//! stable across font settings.

use crate::editor_syntax::{is_image_path, language_for_path};
use std::path::Path;

/// File icon, either a Phosphor SVG path or a Nerd Font glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileIcon {
    /// Phosphor icon path relative to `assets/icons`, e.g. `"phosphor/gear.svg"`.
    Svg(&'static str),
    /// Nerd Font glyph, e.g. `'\u{e68b}'` (seti-rust).
    Glyph(char),
}

/// File-type glyphs are product chrome, not terminal text. Keep them on the
/// bundled font even when the user selects a terminal font without Nerd Font
/// private-use glyphs. This must be the registered TTF family (no space):
/// GPUI does not resolve the "Ioskeley Mono" display label and falls back to
/// a font without the private-use glyphs.
pub(crate) const FILE_ICON_FONT_FAMILY: &str = "IoskeleyMono";

/// Icon for a file tree row.
///
/// Directories get a folder icon that tracks `is_expanded`. Files are matched
/// by special filename first, then language, icon-only extension, image
/// extension, and finally a generic text-file icon.
pub fn icon_for_path(path: &Path, is_dir: bool, is_expanded: bool) -> FileIcon {
    if is_dir {
        return FileIcon::Svg(if is_expanded {
            "phosphor/folder-open.svg"
        } else {
            "phosphor/folder.svg"
        });
    }

    if let Some(icon) = icon_for_special_filename(path) {
        return icon;
    }
    if let Some(language) = language_for_path(path)
        && let Some(icon) = icon_for_language(language)
    {
        return icon;
    }
    if let Some(icon) = icon_for_icon_only_path(path) {
        return icon;
    }
    if is_image_path(path) {
        return FileIcon::Svg("phosphor/image.svg");
    }
    FileIcon::Svg("phosphor/file-text.svg")
}

/// Icon for a language reported by [`language_for_path`], if we have one.
///
/// Glyph codepoints come from the Nerd Fonts Seti-UI + Custom table
/// (`bin/scripts/lib/i_seti.sh`), and each one is present in the bundled
/// IoskeleyMono. Use glyphs when they identify a file type more clearly than
/// a generic Phosphor SVG.
fn icon_for_language(language: &str) -> Option<FileIcon> {
    Some(match language {
        "rust" => FileIcon::Glyph('\u{e68b}'),   // seti-rust
        "python" => FileIcon::Glyph('\u{e606}'), // seti-python
        "go" => FileIcon::Glyph('\u{e627}'),     // seti-go
        "typescript" | "tsx" => FileIcon::Glyph('\u{e628}'), // seti-typescript
        "javascript" | "jsx" => FileIcon::Glyph('\u{e60c}'), // seti-javascript
        "markdown" => FileIcon::Glyph('\u{e609}'), // seti-markdown
        "html" => FileIcon::Glyph('\u{e60e}'),   // seti-html
        "css" => FileIcon::Glyph('\u{e614}'),    // seti-css
        "scss" => FileIcon::Glyph('\u{e603}'),   // seti-sass
        "sql" => FileIcon::Glyph('\u{e64d}'),    // seti-db
        "make" => FileIcon::Glyph('\u{e673}'),   // seti-makefile
        "java" => FileIcon::Glyph('\u{e66d}'),   // seti-java
        "kotlin" => FileIcon::Glyph('\u{e634}'), // custom-kotlin
        "ruby" => FileIcon::Glyph('\u{e605}'),   // custom-ruby
        "c" => FileIcon::Glyph('\u{e649}'),      // seti-c
        "cpp" => FileIcon::Glyph('\u{e646}'),    // seti-cpp
        "zig" => FileIcon::Glyph('\u{e6a9}'),    // seti-zig
        "toml" => FileIcon::Glyph('\u{e6b2}'),   // custom-toml
        "yaml" => FileIcon::Glyph('\u{e6a8}'),   // seti-yml
        "json" => FileIcon::Svg("phosphor/gear.svg"),
        "bash" => FileIcon::Glyph('\u{e691}'), // seti-shell
        _ => return None,
    })
}

/// These names carry more meaning than their extension (or have none).
fn icon_for_special_filename(path: &Path) -> Option<FileIcon> {
    let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    Some(match name.as_str() {
        ".gitignore" => FileIcon::Glyph('\u{e65d}'), // seti-git_ignore
        ".editorconfig" => FileIcon::Glyph('\u{e652}'), // seti-editorconfig
        "tsconfig.json" => FileIcon::Glyph('\u{e69d}'), // seti-tsconfig
        "cargo.lock" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "gemfile.lock"
        | "pipfile.lock" | "poetry.lock" => {
            FileIcon::Glyph('\u{e672}') // seti-lock
        }
        "license" | "license.md" | "license.txt" => FileIcon::Glyph('\u{e60a}'), // seti-license
        _ if name == "dockerfile" || name.starts_with("dockerfile.") => {
            FileIcon::Glyph('\u{e650}') // seti-docker
        }
        _ => return None,
    })
}

/// Icon coverage for paths [`language_for_path`] deliberately leaves unmapped,
/// so adding an icon cannot accidentally enable an unavailable highlighter.
///
/// This only affects presentation. For example, gpui-component has no Vue or
/// Dockerfile grammar and its Swift grammar has no highlight query today.
fn icon_for_icon_only_path(path: &Path) -> Option<FileIcon> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "xml" | "xsl" | "xslt" => Some(FileIcon::Glyph('\u{e619}')), // seti-xml
        "csv" | "tsv" => Some(FileIcon::Glyph('\u{e64a}')),          // seti-csv
        "pdf" => Some(FileIcon::Glyph('\u{e67d}')),                  // seti-pdf
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" => {
            Some(FileIcon::Glyph('\u{e6aa}')) // seti-zip
        }
        "svg" => Some(FileIcon::Glyph('\u{e698}')), // seti-svg
        "ttf" | "otf" | "woff" | "woff2" => Some(FileIcon::Glyph('\u{e659}')), // seti-font
        "graphql" | "gql" => Some(FileIcon::Glyph('\u{e662}')), // seti-graphql
        "tf" | "tfvars" => Some(FileIcon::Glyph('\u{e69a}')), // seti-terraform
        "vue" => Some(FileIcon::Glyph('\u{e6a0}')), // seti-vue
        "svelte" => Some(FileIcon::Glyph('\u{e697}')), // seti-svelte
        "astro" => Some(FileIcon::Glyph('\u{e6b3}')), // custom-astro
        "swift" => Some(FileIcon::Glyph('\u{e699}')), // seti-swift
        "php" => Some(FileIcon::Glyph('\u{e608}')), // seti-php
        "lua" => Some(FileIcon::Glyph('\u{e620}')), // seti-lua
        "dart" => Some(FileIcon::Glyph('\u{e64c}')), // seti-dart
        "scala" => Some(FileIcon::Glyph('\u{e68e}')), // seti-scala
        "ps1" | "psm1" => Some(FileIcon::Glyph('\u{e683}')), // seti-powershell
        "ipynb" => Some(FileIcon::Glyph('\u{e678}')), // seti-notebook
        "wasm" => Some(FileIcon::Glyph('\u{e6a1}')), // seti-wasm
        "wat" => Some(FileIcon::Glyph('\u{e6a2}')), // seti-wat
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_font_family_matches_bundled_ttf_name() {
        let face = ttf_parser::Face::parse(
            include_bytes!("../../../assets/fonts/IoskeleyMono-Regular.ttf"),
            0,
        )
        .unwrap();
        let family = face
            .names()
            .into_iter()
            .filter(|name| name.name_id == ttf_parser::name_id::FAMILY)
            .find_map(|name| name.to_string())
            .expect("bundled font has a family name");
        assert_eq!(family, FILE_ICON_FONT_FAMILY);
    }

    #[test]
    fn directories_use_folder_icons_that_track_expansion() {
        let path = Path::new("src");
        assert_eq!(
            icon_for_path(path, true, false),
            FileIcon::Svg("phosphor/folder.svg")
        );
        assert_eq!(
            icon_for_path(path, true, true),
            FileIcon::Svg("phosphor/folder-open.svg")
        );
    }

    #[test]
    fn languages_map_to_their_glyphs() {
        for (path, glyph) in [
            ("src/main.rs", '\u{e68b}'),
            ("script.py", '\u{e606}'),
            ("cmd/server.go", '\u{e627}'),
            ("src/app.ts", '\u{e628}'),
            ("src/app.tsx", '\u{e628}'),
            ("src/app.js", '\u{e60c}'),
            ("src/app.jsx", '\u{e60c}'),
            ("README.md", '\u{e609}'),
            ("index.html", '\u{e60e}'),
            ("styles.css", '\u{e614}'),
            ("styles.scss", '\u{e603}'),
            ("schema.sql", '\u{e64d}'),
            ("Makefile", '\u{e673}'),
            ("src/Main.java", '\u{e66d}'),
            ("src/App.kt", '\u{e634}'),
            ("lib/thing.rb", '\u{e605}'),
            ("src/main.c", '\u{e649}'),
            ("src/header.h", '\u{e649}'),
            ("src/engine.cpp", '\u{e646}'),
            ("src/engine.hpp", '\u{e646}'),
            ("src/main.zig", '\u{e6a9}'),
            ("Cargo.toml", '\u{e6b2}'),
            ("config.yaml", '\u{e6a8}'),
            ("config.yml", '\u{e6a8}'),
            ("build.sh", '\u{e691}'),
        ] {
            assert_eq!(
                icon_for_path(Path::new(path), false, false),
                FileIcon::Glyph(glyph),
                "unexpected icon for {path}"
            );
        }
    }

    #[test]
    fn icon_only_file_types_do_not_enable_unavailable_highlighters() {
        for (path, glyph) in [
            ("Dockerfile", '\u{e650}'),
            ("Dockerfile.dev", '\u{e650}'),
            ("src/App.vue", '\u{e6a0}'),
            ("Sources/App.swift", '\u{e699}'),
            ("config.xml", '\u{e619}'),
            ("CONFIG.XML", '\u{e619}'),
            ("layout.xslt", '\u{e619}'),
            ("data.csv", '\u{e64a}'),
            ("data.tsv", '\u{e64a}'),
            ("guide.pdf", '\u{e67d}'),
            ("archive.tar.gz", '\u{e6aa}'),
            ("archive.tar.xz", '\u{e6aa}'),
            ("assets/icon.svg", '\u{e698}'),
            ("assets/font.woff2", '\u{e659}'),
            ("schema.graphql", '\u{e662}'),
            ("main.tf", '\u{e69a}'),
            ("src/App.svelte", '\u{e697}'),
            ("src/Page.astro", '\u{e6b3}'),
            ("index.php", '\u{e608}'),
            ("script.lua", '\u{e620}'),
            ("main.dart", '\u{e64c}'),
            ("Main.scala", '\u{e68e}'),
            ("build.ps1", '\u{e683}'),
            ("analysis.ipynb", '\u{e678}'),
            ("module.wasm", '\u{e6a1}'),
            ("module.wat", '\u{e6a2}'),
        ] {
            assert_eq!(language_for_path(Path::new(path)), None);
            assert_eq!(
                icon_for_path(Path::new(path), false, false),
                FileIcon::Glyph(glyph)
            );
        }
    }

    #[test]
    fn special_filenames_override_extension_icons() {
        for (path, glyph) in [
            (".gitignore", '\u{e65d}'),
            (".editorconfig", '\u{e652}'),
            ("tsconfig.json", '\u{e69d}'),
            ("Cargo.lock", '\u{e672}'),
            ("pnpm-lock.yaml", '\u{e672}'),
            ("LICENSE", '\u{e60a}'),
        ] {
            assert_eq!(
                icon_for_path(Path::new(path), false, false),
                FileIcon::Glyph(glyph)
            );
        }
    }

    #[test]
    fn json_without_a_special_filename_uses_the_configuration_icon() {
        assert_eq!(
            icon_for_path(Path::new("package.json"), false, false),
            FileIcon::Svg("phosphor/gear.svg")
        );
    }

    #[test]
    fn bundled_font_has_visible_file_icon_glyphs() {
        let face = ttf_parser::Face::parse(
            include_bytes!("../../../assets/fonts/IoskeleyMono-Regular.ttf"),
            0,
        )
        .unwrap();
        for glyph in [
            '\u{e68b}', '\u{e606}', '\u{e627}', '\u{e628}', '\u{e60c}', '\u{e609}', '\u{e60e}',
            '\u{e614}', '\u{e603}', '\u{e64d}', '\u{e650}', '\u{e673}', '\u{e66d}', '\u{e634}',
            '\u{e605}', '\u{e649}', '\u{e646}', '\u{e699}', '\u{e6a9}', '\u{e6a0}', '\u{e6b2}',
            '\u{e6a8}', '\u{e691}', '\u{e65d}', '\u{e652}', '\u{e69d}', '\u{e672}', '\u{e60a}',
            '\u{e619}', '\u{e64a}', '\u{e67d}', '\u{e6aa}', '\u{e698}', '\u{e659}', '\u{e662}',
            '\u{e69a}', '\u{e697}', '\u{e6b3}', '\u{e608}', '\u{e620}', '\u{e64c}', '\u{e68e}',
            '\u{e683}', '\u{e678}', '\u{e6a1}', '\u{e6a2}',
        ] {
            let glyph_id = face.glyph_index(glyph).expect("file icon glyph must exist");
            let bounds = face
                .glyph_bounding_box(glyph_id)
                .expect("file icon glyph must have an outline");
            assert!(
                bounds.width() > 0 && bounds.height() > 0,
                "empty file icon glyph: {glyph}"
            );
        }
    }

    #[test]
    fn images_fall_back_to_the_image_icon() {
        assert_eq!(
            icon_for_path(Path::new("assets/logo.png"), false, false),
            FileIcon::Svg("phosphor/image.svg")
        );
        assert_eq!(
            icon_for_path(Path::new("assets/icon.svg"), false, false),
            FileIcon::Glyph('\u{e698}')
        );
    }

    #[test]
    fn unknown_files_fall_back_to_the_text_icon() {
        for path in ["no_extension", "notes.unknown", "README.bak"] {
            assert_eq!(
                icon_for_path(Path::new(path), false, false),
                FileIcon::Svg("phosphor/file-text.svg"),
                "unexpected icon for {path}"
            );
        }
    }

    #[test]
    fn a_directory_named_like_an_image_still_gets_a_folder_icon() {
        assert_eq!(
            icon_for_path(Path::new("assets/logo.png"), true, false),
            FileIcon::Svg("phosphor/folder.svg")
        );
    }
}
