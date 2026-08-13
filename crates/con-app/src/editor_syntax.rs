use gpui::{FontStyle, FontWeight, Hsla, Pixels, TextRun, TextStyle, WhiteSpace};
use gpui_component::Colorize;
use gpui_component::{Theme, highlighter::SyntaxHighlighter};
use ropey::Rope;
use std::path::Path;

pub(crate) fn language_for_path(path: &Path) -> Option<&'static str> {
    let file_name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
    match file_name.as_str() {
        "cargo.toml" | "pyproject.toml" => return Some("toml"),
        "package.json" | "tsconfig.json" => return Some("json"),
        "dockerfile" => return Some("dockerfile"),
        "makefile" => return Some("make"),
        _ => {}
    }

    match path
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => Some("rust"),
        "toml" => Some("toml"),
        "json" | "jsonc" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "md" | "markdown" => Some("markdown"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("jsx"),
        "py" => Some("python"),
        "go" => Some("go"),
        "sh" | "bash" | "zsh" => Some("bash"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "scss" => Some("scss"),
        "sql" => Some("sql"),
        _ => None,
    }
}

/// Image extensions routed to the read-only image viewer. Keeps in sync with
/// the formats GPUI's `img` element can decode (`Img::extensions`), trimmed to
/// the ones commonly found in code repositories.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "tiff", "tif", "tga", "ico",
];

/// Returns true when `path` has a known image extension (case-insensitive).
/// This is a purely path-based check — a directory literally named `foo.png`
/// matches too. Used to route files to the read-only image viewer instead of
/// the text editor.
pub(crate) fn is_image_path(path: &Path) -> bool {
    path.extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension.as_str()))
}

pub(crate) fn highlighted_line_runs(
    text: &str,
    lines: &[String],
    language: Option<&str>,
    theme: &Theme,
    mono_font_family: impl Into<gpui::SharedString>,
    font_size: Pixels,
    line_height: Pixels,
) -> Vec<Vec<TextRun>> {
    let base_style = base_text_style(
        theme.foreground.opacity(0.90),
        mono_font_family,
        font_size,
        line_height,
    );
    let Some(language) = language else {
        return base_line_runs(lines, &base_style);
    };

    let line_starts = line_start_offsets(text, lines.len());
    let rope = Rope::from_str(text);
    let mut highlighter = SyntaxHighlighter::new(language);
    highlighter.update(None, &rope, None);
    let highlights = highlighter.styles(&(0..text.len()), &theme.highlight_theme);

    lines
        .iter()
        .enumerate()
        .map(|(line_index, line)| {
            let line_start = line_starts[line_index];
            let line_end = line_start + line.len();
            runs_for_line(
                line_start,
                line_end,
                line.len(),
                &highlights,
                &base_style,
                theme,
            )
        })
        .collect()
}

fn base_line_runs(lines: &[String], base_style: &TextStyle) -> Vec<Vec<TextRun>> {
    lines
        .iter()
        .map(|line| vec![base_style.to_run(line.len())])
        .collect()
}

fn line_start_offsets(text: &str, line_count: usize) -> Vec<usize> {
    let mut starts = Vec::with_capacity(line_count);
    starts.push(0);
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' && starts.len() < line_count {
            starts.push(index + 1);
        }
    }
    while starts.len() < line_count {
        starts.push(text.len());
    }
    starts
}

fn runs_for_line(
    line_start: usize,
    line_end: usize,
    line_len: usize,
    highlights: &[(std::ops::Range<usize>, gpui::HighlightStyle)],
    base_style: &TextStyle,
    theme: &Theme,
) -> Vec<TextRun> {
    if line_len == 0 {
        return vec![base_style.to_run(0)];
    }

    let mut runs = Vec::new();
    let mut cursor = line_start;

    for (range, highlight) in highlights {
        let start = range.start.max(line_start).min(line_end);
        let end = range.end.max(line_start).min(line_end);
        if start >= end {
            continue;
        }

        if start > cursor {
            runs.push(base_style.to_run(start - cursor));
        }

        let mut style = base_style.clone();
        apply_highlight_style(&mut style, *highlight, theme);
        runs.push(style.to_run(end - start));
        cursor = end;
    }

    if cursor < line_end {
        runs.push(base_style.to_run(line_end - cursor));
    }

    if runs.is_empty() {
        runs.push(base_style.to_run(line_len));
    }
    runs
}

fn base_text_style(
    color: Hsla,
    mono_font_family: impl Into<gpui::SharedString>,
    font_size: Pixels,
    line_height: Pixels,
) -> TextStyle {
    TextStyle {
        color,
        font_family: mono_font_family.into(),
        font_size: font_size.into(),
        line_height: line_height.into(),
        font_weight: FontWeight::NORMAL,
        font_style: FontStyle::Normal,
        white_space: WhiteSpace::Nowrap,
        ..Default::default()
    }
}

fn apply_highlight_style(
    text_style: &mut TextStyle,
    highlight: gpui::HighlightStyle,
    theme: &Theme,
) {
    if let Some(color) = highlight.color {
        let base = if theme.is_dark() {
            theme.foreground.opacity(0.96)
        } else {
            theme.foreground.opacity(0.90)
        };
        text_style.color = color.mix_oklab(base, 0.76).opacity(0.99);
    }
    if let Some(weight) = highlight.font_weight {
        text_style.font_weight = weight;
    }
    if let Some(style) = highlight.font_style {
        text_style.font_style = style;
    }
    text_style.background_color = highlight.background_color;
    text_style.underline = highlight.underline;
    text_style.strikethrough = highlight.strikethrough;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_for_path_recognizes_common_editor_files() {
        for (path, language) in [
            ("src/main.rs", "rust"),
            ("Cargo.toml", "toml"),
            ("package.json", "json"),
            ("src/app.tsx", "tsx"),
            ("script.py", "python"),
            ("README.md", "markdown"),
            ("Dockerfile", "dockerfile"),
        ] {
            assert_eq!(language_for_path(Path::new(path)), Some(language));
        }
    }

    #[test]
    fn is_image_path_recognizes_image_extensions_case_insensitively() {
        for extension in [
            "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "bmp", "tiff", "tif", "tga", "ico",
        ] {
            assert!(is_image_path(Path::new(&format!("screenshot.{extension}"))));
        }
        assert!(is_image_path(Path::new("/tmp/screenshot.PNG")));
        assert!(is_image_path(Path::new("/tmp/Screenshot.Jpeg")));
    }

    #[test]
    fn is_image_path_rejects_non_images() {
        assert!(!is_image_path(Path::new("notes.md")));
        assert!(!is_image_path(Path::new("src/main.rs")));
        assert!(!is_image_path(Path::new("archive.png.tar")));
        assert!(!is_image_path(Path::new("no_extension")));
        assert!(!is_image_path(Path::new(".gitignore")));
    }

    #[test]
    fn is_image_path_is_path_based_not_file_type_based() {
        // A directory named `foo.png` matches too — the check never stats the
        // filesystem, it only looks at the extension.
        assert!(is_image_path(Path::new("assets/logo.png")));
        assert!(!is_image_path(Path::new("images")));
        assert!(!is_image_path(Path::new("images/")));
    }

    #[test]
    fn line_start_offsets_account_for_lf_newlines() {
        let text = "abc\nde\n\nf";

        assert_eq!(line_start_offsets(text, 4), vec![0, 4, 7, 8]);
    }

    #[test]
    fn line_start_offsets_account_for_crlf_newlines() {
        let text = "abc\r\nde\r\n\r\nf";

        assert_eq!(line_start_offsets(text, 4), vec![0, 5, 9, 11]);
    }
}
