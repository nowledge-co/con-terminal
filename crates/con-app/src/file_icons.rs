//! Path → icon mapping for the file tree.
//!
//! An icon is either one of con's bundled Phosphor SVGs (`assets/icons/`) or a
//! Nerd Font glyph. Glyphs live in the Seti-UI + Custom private-use range, so
//! they only render with a Nerd Font — con bundles IoskeleyMono, which covers
//! that range.

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
    if let Some(icon) = icon_for_unmapped_extension(path) {
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
        "dockerfile" => FileIcon::Glyph('\u{e650}'), // seti-docker
        "make" => FileIcon::Glyph('\u{e673}'),   // seti-makefile
        "java" => FileIcon::Glyph('\u{e66d}'),   // seti-java
        "kotlin" => FileIcon::Glyph('\u{e634}'), // custom-kotlin
        "ruby" => FileIcon::Glyph('\u{e605}'),   // custom-ruby
        "c" => FileIcon::Glyph('\u{e649}'),      // seti-c
        "cpp" => FileIcon::Glyph('\u{e646}'),    // seti-cpp
        "swift" => FileIcon::Glyph('\u{e699}'),  // seti-swift
        "zig" => FileIcon::Glyph('\u{e6a9}'),    // seti-zig
        "toml" | "json" | "yaml" => FileIcon::Svg("phosphor/gear.svg"),
        "bash" => FileIcon::Svg("phosphor/terminal.svg"),
        _ => return None,
    })
}

/// Icon coverage for extensions [`language_for_path`] deliberately leaves
/// unmapped, so that adding an icon here cannot change editor behaviour.
///
/// `.vue` is the only such case today: gpui-component bundles tree-sitter
/// grammars for svelte and astro but not vue, so mapping `.vue` in
/// `language_for_path` would make the editor parse every `.vue` buffer with the
/// empty plain-text query set — no highlighting, just wasted work. The icon is
/// still worth showing.
fn icon_for_unmapped_extension(path: &Path) -> Option<FileIcon> {
    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "vue" => Some(FileIcon::Glyph('\u{e6a0}')), // seti-vue
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
            ("Dockerfile", '\u{e650}'),
            ("Makefile", '\u{e673}'),
            ("src/Main.java", '\u{e66d}'),
            ("src/App.kt", '\u{e634}'),
            ("lib/thing.rb", '\u{e605}'),
            ("src/main.c", '\u{e649}'),
            ("src/header.h", '\u{e649}'),
            ("src/engine.cpp", '\u{e646}'),
            ("src/engine.hpp", '\u{e646}'),
            ("Sources/App.swift", '\u{e699}'),
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
    fn vue_gets_an_icon_without_entering_language_for_path() {
        // `.vue` has no bundled tree-sitter grammar, so the editor must keep
        // treating it as an unknown extension while the tree still shows the
        // Vue glyph.
        assert_eq!(language_for_path(Path::new("src/App.vue")), None);
        assert_eq!(
            icon_for_path(Path::new("src/App.vue"), false, false),
            FileIcon::Glyph('\u{e6a0}')
        );
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
