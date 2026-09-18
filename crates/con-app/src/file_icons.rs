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
/// private-use glyphs.
pub(crate) const FILE_ICON_FONT_FAMILY: &str = "Ioskeley Mono";

/// Icon for a file tree row.
///
/// Directories get a folder icon that tracks `is_expanded`. Files are matched
/// by language first, then by the extensions that only need an icon, then by
/// image extension, and finally fall back to a generic text-file icon.
pub fn icon_for_path(path: &Path, is_dir: bool, is_expanded: bool) -> FileIcon {
    if is_dir {
        return FileIcon::Svg(if is_expanded {
            "phosphor/folder-open.svg"
        } else {
            "phosphor/folder.svg"
        });
    }

    if let Some(language) = language_for_path(path) {
        if let Some(icon) = icon_for_language(language) {
            return icon;
        }
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
/// IoskeleyMono. Languages that Phosphor can express keep the SVG; glyphs are
/// for the languages it cannot.
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
        "toml" | "json" | "yaml" => FileIcon::Svg("phosphor/gear.svg"),
        "bash" => FileIcon::Svg("phosphor/terminal.svg"),
        _ => return None,
    })
}

/// Icon coverage for paths [`language_for_path`] deliberately leaves unmapped,
/// so adding an icon cannot accidentally enable an unavailable highlighter.
///
/// gpui-component has no Vue or Dockerfile grammar and its Swift grammar has
/// no highlight query today. These files still deserve recognizable icons.
fn icon_for_icon_only_path(path: &Path) -> Option<FileIcon> {
    let file_name = path.file_name()?.to_string_lossy();
    if file_name.eq_ignore_ascii_case("dockerfile") {
        return Some(FileIcon::Glyph('\u{e650}')); // seti-docker
    }

    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "vue" => Some(FileIcon::Glyph('\u{e6a0}')),   // seti-vue
        "swift" => Some(FileIcon::Glyph('\u{e699}')), // seti-swift
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("src/App.vue", '\u{e6a0}'),
            ("Sources/App.swift", '\u{e699}'),
        ] {
            assert_eq!(language_for_path(Path::new(path)), None);
            assert_eq!(
                icon_for_path(Path::new(path), false, false),
                FileIcon::Glyph(glyph)
            );
        }
    }

    #[test]
    fn configuration_and_script_files_use_phosphor_icons() {
        for path in ["Cargo.toml", "package.json", "ci.yaml", "ci.yml"] {
            assert_eq!(
                icon_for_path(Path::new(path), false, false),
                FileIcon::Svg("phosphor/gear.svg"),
                "unexpected icon for {path}"
            );
        }
        assert_eq!(
            icon_for_path(Path::new("build.sh"), false, false),
            FileIcon::Svg("phosphor/terminal.svg")
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
            '\u{e605}', '\u{e649}', '\u{e646}', '\u{e699}', '\u{e6a9}', '\u{e6a0}',
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
            FileIcon::Svg("phosphor/image.svg")
        );
    }

    #[test]
    fn unknown_files_fall_back_to_the_text_icon() {
        for path in ["LICENSE", ".gitignore", "no_extension", "archive.tar.gz"] {
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
