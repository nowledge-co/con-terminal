use std::path::PathBuf;

#[cfg(target_os = "windows")]
use con_ghostty::GhosttyTerminal;
use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths};

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TerminalPastePayload {
    Text(String),
    ForwardCtrlV,
}

/// Produce a bounded, inert preview for the unsafe-paste confirmation UI.
/// Terminal controls, Unicode directional controls, and invisible formatting
/// characters are replaced so text cannot alter the UI or disguise a command.
pub fn unsafe_paste_preview(text: &str) -> String {
    const MAX_CHARS: usize = 240;
    const MAX_LINES: usize = 4;

    let mut preview = String::with_capacity(text.len().min(MAX_CHARS));
    let mut chars = text.chars();
    let mut consumed = 0;
    let mut lines = 1;
    let mut truncated = false;

    for ch in chars.by_ref() {
        if consumed == MAX_CHARS {
            truncated = true;
            break;
        }
        consumed += 1;

        match ch {
            '\n' if lines < MAX_LINES => {
                preview.push('↵');
                preview.push('\n');
                lines += 1;
            }
            '\n' => {
                truncated = true;
                break;
            }
            '\r' => preview.push('␍'),
            '\t' => preview.push('⇥'),
            ch if is_unsafe_preview_control(ch) => preview.push('�'),
            ch => preview.push(ch),
        }
    }

    if !truncated && chars.next().is_some() {
        truncated = true;
    }
    if truncated {
        preview.push('…');
    }
    preview
}

fn is_unsafe_preview_control(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            // Soft hyphen, Arabic letter mark, and Mongolian vowel separator.
            '\u{00ad}' | '\u{061c}' | '\u{180e}'
            // Zero-width controls plus left-to-right/right-to-left marks.
            | '\u{200b}'..='\u{200f}'
            // Unicode line/paragraph separators and bidi embeddings/overrides.
            | '\u{2028}'..='\u{202e}'
            // Word joiner, bidi isolates, and deprecated directional controls.
            | '\u{2060}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
        )
}

pub fn payload_from_clipboard(item: &ClipboardItem) -> Option<TerminalPastePayload> {
    let paths = external_paths_from_entries(item.entries());
    if !paths.is_empty() {
        return quoted_paths_text(&paths).map(TerminalPastePayload::Text);
    }

    if item
        .entries()
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::Image(image) if !image.bytes.is_empty()))
    {
        return Some(TerminalPastePayload::ForwardCtrlV);
    }

    let text = text_from_entries(item.entries());
    if cfg!(target_os = "linux")
        && let Some(paths) = paths_from_uri_list(&text)
        && !paths.is_empty()
    {
        return quoted_paths_text(&paths).map(TerminalPastePayload::Text);
    }

    if !text.is_empty() {
        return Some(TerminalPastePayload::Text(text));
    }

    None
}

pub fn payload_from_external_paths(paths: &ExternalPaths) -> Option<TerminalPastePayload> {
    quoted_paths_text(paths.paths()).map(TerminalPastePayload::Text)
}

#[cfg(target_os = "windows")]
pub fn copy_selection_to_clipboard(terminal: &GhosttyTerminal, cx: &mut gpui::App) -> bool {
    let has_selection = terminal.has_selection();
    let selection = has_selection.then(|| terminal.selection_text()).flatten();
    match copy_selection_decision(has_selection, selection.as_deref()) {
        CopySelectionDecision::NoSelection => false,
        CopySelectionDecision::ClearOnly => {
            terminal.clear_selection();
            true
        }
        CopySelectionDecision::CopyAndClear(selection) => {
            cx.write_to_clipboard(ClipboardItem::new_string(selection.to_string()));
            terminal.clear_selection();
            true
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
#[cfg(any(target_os = "windows", test))]
enum CopySelectionDecision<'a> {
    NoSelection,
    ClearOnly,
    CopyAndClear(&'a str),
}

#[cfg(any(target_os = "windows", test))]
fn copy_selection_decision(
    has_selection: bool,
    selection: Option<&str>,
) -> CopySelectionDecision<'_> {
    if !has_selection {
        return CopySelectionDecision::NoSelection;
    }
    match selection {
        Some(selection) if !selection.is_empty() => CopySelectionDecision::CopyAndClear(selection),
        _ => CopySelectionDecision::ClearOnly,
    }
}

fn external_paths_from_entries(entries: &[ClipboardEntry]) -> Vec<PathBuf> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.paths()),
            _ => None,
        })
        .flatten()
        .cloned()
        .collect()
}

fn text_from_entries(entries: &[ClipboardEntry]) -> String {
    entries
        .iter()
        .filter_map(|entry| match entry {
            ClipboardEntry::String(string) => Some(string.text.as_str()),
            _ => None,
        })
        .collect()
}

fn paths_from_uri_list(text: &str) -> Option<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(path) = path_from_file_uri(line) else {
            return None;
        };
        paths.push(path);
    }

    if paths.is_empty() { None } else { Some(paths) }
}

fn path_from_file_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = rest
        .strip_prefix("localhost/")
        .map(|path| format!("/{path}"))
        .unwrap_or_else(|| rest.to_string());

    if !path.starts_with('/') {
        return None;
    }

    Some(PathBuf::from(percent_decode(&path)?))
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hi = *bytes.get(index + 1)?;
            let lo = *bytes.get(index + 2)?;
            decoded.push((hex_value(hi)? << 4) | hex_value(lo)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn quoted_paths_text(paths: &[PathBuf]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }

    let mut text = String::new();
    for path in paths {
        text.push(' ');
        text.push_str(&quote_path(path));
    }
    text.push(' ');
    Some(text)
}

#[cfg(windows)]
fn quote_path(path: &std::path::Path) -> String {
    let mut quoted = String::new();
    quoted.push('"');
    for character in path.display().to_string().chars() {
        if character == '"' {
            quoted.push('"');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

#[cfg(not(windows))]
fn quote_path(path: &std::path::Path) -> String {
    let mut quoted = String::new();
    quoted.push('"');
    for character in path.display().to_string().chars() {
        if matches!(character, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{ClipboardEntry, ClipboardItem, ClipboardString, ExternalPaths, Image, ImageFormat};

    use super::{
        CopySelectionDecision, TerminalPastePayload, copy_selection_decision,
        payload_from_clipboard, quoted_paths_text, unsafe_paste_preview,
    };

    #[test]
    fn copy_selection_decision_distinguishes_empty_and_absent_selection() {
        assert_eq!(
            copy_selection_decision(false, Some("ignored")),
            CopySelectionDecision::NoSelection
        );
        assert_eq!(
            copy_selection_decision(true, None),
            CopySelectionDecision::ClearOnly
        );
        assert_eq!(
            copy_selection_decision(true, Some("")),
            CopySelectionDecision::ClearOnly
        );
        assert_eq!(
            copy_selection_decision(true, Some("selected")),
            CopySelectionDecision::CopyAndClear("selected")
        );
    }

    #[test]
    fn quoted_paths_are_space_padded() {
        let paths = vec![
            PathBuf::from("plain.txt"),
            PathBuf::from("name with spaces.png"),
        ];

        assert_eq!(
            quoted_paths_text(&paths),
            Some(" \"plain.txt\" \"name with spaces.png\" ".to_string())
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_paths_escape_double_quote_expansions() {
        let paths = vec![PathBuf::from(r#"cost "$HOME".txt"#)];

        assert_eq!(
            quoted_paths_text(&paths),
            Some(r#" "cost \"\$HOME\".txt" "#.to_string())
        );
    }

    #[test]
    fn clipboard_text_becomes_text_payload() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::String(ClipboardString::new(
                "echo hello".to_string(),
            ))],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::Text("echo hello".to_string()))
        );
    }

    #[test]
    fn clipboard_paths_win_over_lossy_text_fallback() {
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::String(ClipboardString::new(
                    "lossy platform path text".to_string(),
                )),
                ClipboardEntry::ExternalPaths(ExternalPaths(
                    vec![PathBuf::from("one.txt"), PathBuf::from("two words.txt")].into(),
                )),
            ],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::Text(
                " \"one.txt\" \"two words.txt\" ".to_string()
            ))
        );
    }

    #[test]
    fn clipboard_image_wins_over_text_representation() {
        let image = Image::from_bytes(ImageFormat::Png, vec![137, 80, 78, 71]);
        let item = ClipboardItem {
            entries: vec![
                ClipboardEntry::String(ClipboardString::new(
                    "https://example.com/image.png".to_string(),
                )),
                ClipboardEntry::Image(image),
            ],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::ForwardCtrlV)
        );
    }

    #[test]
    fn copied_image_and_code_files_paste_as_paths() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::ExternalPaths(ExternalPaths(
                vec![PathBuf::from("diagram.png"), PathBuf::from("main.rs")].into(),
            ))],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::Text(
                " \"diagram.png\" \"main.rs\" ".to_string()
            ))
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_file_uri_clipboard_pastes_as_paths() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::String(ClipboardString::new(
                "# copied files\nfile:///home/me/diagram%20one.png\nfile:///home/me/main.rs\n"
                    .to_string(),
            ))],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::Text(
                " \"/home/me/diagram one.png\" \"/home/me/main.rs\" ".to_string()
            ))
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn file_uri_clipboard_stays_text_off_linux() {
        let text = "file:///home/me/diagram%20one.png\nfile:///home/me/main.rs\n";
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::String(ClipboardString::new(
                text.to_string(),
            ))],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::Text(text.to_string()))
        );
    }

    #[test]
    fn ordinary_file_url_text_stays_text_when_mixed_with_words() {
        let text = "see file:///home/me/main.rs";
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::String(ClipboardString::new(
                text.to_string(),
            ))],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::Text(text.to_string()))
        );
    }

    #[test]
    fn image_only_clipboard_forwards_native_paste_to_tui() {
        let image = Image::from_bytes(ImageFormat::Png, vec![137, 80, 78, 71]);
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::Image(image)],
        };

        assert_eq!(
            payload_from_clipboard(&item),
            Some(TerminalPastePayload::ForwardCtrlV)
        );
    }

    #[test]
    fn empty_image_clipboard_is_ignored() {
        let item = ClipboardItem {
            entries: vec![ClipboardEntry::Image(Image::empty())],
        };

        assert_eq!(payload_from_clipboard(&item), None);
    }

    #[test]
    fn unsafe_paste_preview_is_bounded_and_neutralizes_control_text() {
        let text = format!(
            "echo ok\n\x1b[31mhidden\u{202e}\u{200b}\u{2028}{}",
            "x".repeat(300)
        );
        let preview = unsafe_paste_preview(&text);

        assert!(preview.starts_with("echo ok↵\n�[31mhidden���"));
        assert!(preview.ends_with('…'));
        assert!(!preview.contains('\x1b'));
        assert!(!preview.contains('\u{202e}'));
        assert!(!preview.contains('\u{200b}'));
        assert!(!preview.contains('\u{2028}'));
        assert!(preview.chars().count() <= 245);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_keep_single_backslashes() {
        let paths = vec![PathBuf::from(r"C:\Users\me\image file.png")];

        assert_eq!(
            quoted_paths_text(&paths),
            Some(r#" "C:\Users\me\image file.png" "#.to_string())
        );
    }
}
