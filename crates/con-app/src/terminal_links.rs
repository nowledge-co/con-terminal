//! Terminal link interaction for GPUI-owned terminal renderers.
//!
//! macOS delegates this to embedded libghostty. Windows and Linux own
//! their terminal paint/input path: OSC 8 targets use the shared URL policy,
//! while plain URLs are detected in the visible row. Resolve links only on
//! mouse gestures; painting uses the already-classified, owned hover target.

#[cfg(any(target_os = "windows", target_os = "linux", test))]
use crate::terminal_url::{Osc8UrlDecision, evaluate_osc8_url};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use con_ghostty::vt::ScreenSnapshot;
#[cfg(any(target_os = "windows", target_os = "linux"))]
use gpui::Modifiers;

#[cfg(any(target_os = "windows", target_os = "linux", test))]
const URL_SCHEMES: &[&str] = &[
    "http://",
    "https://",
    "mailto:",
    "ftp://",
    "file:",
    "ssh:",
    "git://",
    "tel:",
    "magnet:",
    "ipfs://",
    "ipns://",
    "gemini://",
    "gopher://",
    "news:",
];

#[cfg(any(target_os = "windows", target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TerminalLink {
    target: LinkTarget,
    pub(crate) row: u16,
    /// Inclusive start column.
    pub(crate) start_col: u16,
    /// Exclusive end column.
    pub(crate) end_col: u16,
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LinkTarget {
    Plain(String),
    Osc8(Osc8UrlDecision),
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
impl TerminalLink {
    pub(crate) fn osc8(raw: &str, col: u16, row: u16) -> Self {
        Self {
            target: LinkTarget::Osc8(evaluate_osc8_url(raw)),
            row,
            start_col: col,
            end_col: col + 1,
        }
    }

    /// A stationary pointer can outlive terminal output. If the target changed
    /// since hover, this press only updates the preview; it must not arm the
    /// newly substituted target. The caller still consumes the matching release.
    pub(crate) fn for_press(self, preview: Option<&Self>) -> Option<Self> {
        if preview.is_some_and(|preview| !self.same_link(preview)) {
            return None;
        }
        Some(self)
    }

    /// OSC 8 hit rectangles cover one cell, not the whole linked label. Match
    /// activation by target so crossing a wide cell or wrapped label is allowed;
    /// keep structural equality for hover geometry and plain URL ranges.
    pub(crate) fn same_link(&self, other: &Self) -> bool {
        match (&self.target, &other.target) {
            (LinkTarget::Osc8(target), LinkTarget::Osc8(other)) => target == other,
            _ => self == other,
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn open(&self, window: &mut gpui::Window, cx: &mut gpui::App) {
        use gpui_component::{WindowExt as _, notification::Notification};
        match &self.target {
            LinkTarget::Plain(url) | LinkTarget::Osc8(Osc8UrlDecision::Allow(url)) => {
                cx.open_url(url)
            }
            LinkTarget::Osc8(Osc8UrlDecision::Deny(denial)) => {
                window.push_notification(
                    Notification::new().title("Link blocked").message(format!(
                        "{}\n{}",
                        denial.reason.message(),
                        denial.display
                    )),
                    cx,
                );
            }
        }
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn preview(
        &self,
        theme: &gpui_component::Theme,
        pane_width: gpui::Pixels,
    ) -> Option<gpui::Div> {
        use gpui::*;
        let LinkTarget::Osc8(decision) = &self.target else {
            return None;
        };
        let (display, status, color) = match decision {
            Osc8UrlDecision::Allow(url) => (url, "Opens", theme.foreground),
            Osc8UrlDecision::Deny(denial) => (&denial.display, "Blocked", theme.warning),
        };
        Some(
            div()
                .absolute()
                .left(px(10.0))
                .bottom(px(10.0))
                .max_w((pane_width - px(20.0)).max(px(0.0)))
                .flex()
                .flex_none()
                .min_w_0()
                .items_center()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(5.0))
                .bg(theme.popover.opacity(0.96))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .text_color(color)
                        .child(status),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(11.0))
                        .text_color(theme.foreground)
                        .child(display.clone()),
                ),
        )
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn should_open_link(modifiers: &Modifiers) -> bool {
    modifiers.control
        && !modifiers.alt
        && !modifiers.shift
        && !modifiers.platform
        && !modifiers.function
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(crate) fn link_at_snapshot(
    snapshot: &ScreenSnapshot,
    col: u16,
    row: u16,
) -> Option<TerminalLink> {
    if row >= snapshot.rows || col >= snapshot.cols || snapshot.cols == 0 {
        return None;
    }

    let row_start = usize::from(row) * usize::from(snapshot.cols);
    let row_end = row_start + usize::from(snapshot.cols);
    let cells = snapshot.cells.get(row_start..row_end)?;

    let mut line = String::with_capacity(cells.len());
    let mut col_byte_ranges = Vec::with_capacity(cells.len());
    for cell in cells {
        let start = line.len();
        let ch = match cell.codepoint {
            0 => ' ',
            codepoint => char::from_u32(codepoint).unwrap_or('\u{FFFD}'),
        };
        line.push(ch);
        col_byte_ranges.push((start, line.len()));
    }

    let col = usize::from(col);
    let (hover_start, hover_end) = *col_byte_ranges.get(col)?;
    link_at_line(&line, hover_start, hover_end).map(|mut link| {
        link.row = row;
        link.start_col = byte_to_col(&col_byte_ranges, link.start_col as usize) as u16;
        link.end_col = byte_to_col_end(&col_byte_ranges, link.end_col as usize) as u16;
        link
    })
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn link_at_line(line: &str, hover_start: usize, hover_end: usize) -> Option<TerminalLink> {
    let mut search_from = 0;
    while let Some((scheme_start, scheme_len)) = find_next_scheme(line, search_from) {
        let mut end = consume_url(line, scheme_start);
        end = trim_url_end(line, scheme_start, end);
        let has_payload = end > scheme_start + scheme_len;

        if has_payload
            && byte_range_intersects(scheme_start, end, hover_start, hover_end)
            && let Ok(start_col) = u16::try_from(scheme_start)
            && let Ok(end_col) = u16::try_from(end)
        {
            return Some(TerminalLink {
                target: LinkTarget::Plain(line[scheme_start..end].to_string()),
                row: 0,
                start_col,
                end_col,
            });
        }

        search_from = end.max(scheme_start + scheme_len);
    }

    None
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn find_next_scheme(line: &str, start: usize) -> Option<(usize, usize)> {
    let mut index = start.min(line.len());
    while index < line.len() {
        if !line.is_char_boundary(index) {
            index += 1;
            continue;
        }

        for scheme in URL_SCHEMES {
            if starts_with_ascii_ci(line, index, scheme) && is_scheme_boundary(line, index) {
                return Some((index, scheme.len()));
            }
        }

        index += 1;
    }
    None
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn starts_with_ascii_ci(line: &str, index: usize, needle: &str) -> bool {
    line.as_bytes()
        .get(index..index + needle.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn is_scheme_boundary(line: &str, index: usize) -> bool {
    if index == 0 {
        return true;
    }

    line[..index]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_ascii_alphanumeric())
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn consume_url(line: &str, start: usize) -> usize {
    let mut end = start;
    for (offset, ch) in line[start..].char_indices() {
        if is_url_terminator(ch) {
            break;
        }
        end = start + offset + ch.len_utf8();
    }
    end
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn is_url_terminator(ch: char) -> bool {
    ch.is_whitespace()
        || ch.is_control()
        || matches!(ch, '"' | '\'' | '`' | '<' | '>' | '{' | '}' | '|' | '\\')
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn trim_url_end(line: &str, start: usize, mut end: usize) -> usize {
    loop {
        let Some(last) = line[start..end].chars().next_back() else {
            return end;
        };

        let should_trim = matches!(last, '.' | ',' | ';' | ':' | '!')
            || (last == ')'
                && count_char(line, start, end, ')') > count_char(line, start, end, '('))
            || (last == ']'
                && count_char(line, start, end, ']') > count_char(line, start, end, '['));

        if !should_trim {
            return end;
        }
        end -= last.len_utf8();
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn count_char(line: &str, start: usize, end: usize, needle: char) -> usize {
    line[start..end].chars().filter(|ch| *ch == needle).count()
}

#[cfg(any(target_os = "windows", target_os = "linux", test))]
fn byte_range_intersects(start: usize, end: usize, hover_start: usize, hover_end: usize) -> bool {
    let hover_end = hover_end.max(hover_start + 1);
    start < hover_end && end > hover_start
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn byte_to_col(col_byte_ranges: &[(usize, usize)], byte: usize) -> usize {
    col_byte_ranges
        .iter()
        .position(|(_, end)| *end > byte)
        .unwrap_or(col_byte_ranges.len().saturating_sub(1))
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn byte_to_col_end(col_byte_ranges: &[(usize, usize)], byte_end: usize) -> usize {
    if byte_end == 0 {
        return 0;
    }
    byte_to_col(col_byte_ranges, byte_end - 1).saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(line: &str, needle: &str) -> Option<String> {
        let index = line.find(needle).expect("needle in line");
        link_at_line(line, index, index + 1).map(|link| match link.target {
            LinkTarget::Plain(url) => url,
            LinkTarget::Osc8(_) => panic!("plain-text detection returned an OSC 8 target"),
        })
    }

    #[test]
    fn osc8_targets_are_classified_before_activation() {
        let link = TerminalLink::osc8("HTTPS://Example.COM:443/path", 3, 2);
        assert_eq!(
            link.target,
            LinkTarget::Osc8(Osc8UrlDecision::Allow("https://example.com/path".into()))
        );
        for uri in [
            "file:///tmp/script.sh",
            "javascript:alert(1)",
            "https://user:pass@example.com",
            "https://exa\u{200b}mple.com",
        ] {
            assert!(matches!(
                TerminalLink::osc8(uri, 3, 2).target,
                LinkTarget::Osc8(Osc8UrlDecision::Deny(_))
            ));
        }
        assert_ne!(
            link,
            TerminalLink::osc8("https://example.com/changed", 3, 2)
        );
        assert_ne!(link, TerminalLink::osc8("https://example.com/path", 4, 2));
        // Plain, visible URLs retain their existing scheme support.
        assert_eq!(
            detect("file:///tmp/file.txt", "file"),
            Some("file:///tmp/file.txt".into())
        );
    }

    #[test]
    fn press_does_not_arm_a_target_substituted_after_hover() {
        let preview = TerminalLink::osc8("https://example.com/shown", 3, 2);
        let changed = TerminalLink::osc8("https://example.com/substituted", 3, 2);
        assert_eq!(changed.clone().for_press(Some(&preview)), None);
        assert_eq!(changed.clone().for_press(Some(&changed)), Some(changed));
        assert_eq!(preview.clone().for_press(None), Some(preview.clone()));
        // Removing OSC 8 must not bypass the preview check via plain detection.
        let plain = link_at_line("https://example.com/plain", 3, 4).unwrap();
        assert_eq!(plain.for_press(Some(&preview)), None);
    }

    #[test]
    fn osc8_activation_matches_target_across_cells_but_hover_tracks_geometry() {
        let start = TerminalLink::osc8("https://example.com/label", 9, 2);
        let wrapped = TerminalLink::osc8("https://example.com/label", 0, 3);
        assert_ne!(start, wrapped);
        assert!(start.same_link(&wrapped));
        assert_eq!(wrapped.clone().for_press(Some(&start)), Some(wrapped));
        assert!(!start.same_link(&TerminalLink::osc8("https://example.com/other", 9, 2)));

        let plain = link_at_line("https://example.com/label", 3, 4).unwrap();
        let mut other_row = plain.clone();
        other_row.row = 1;
        assert!(!plain.same_link(&other_row));
        assert!(!start.same_link(&plain));
    }

    #[test]
    fn detects_scheme_url() {
        assert_eq!(
            detect("visit https://example.com now", "example"),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn trims_sentence_punctuation() {
        assert_eq!(
            detect("visit https://example.com, then continue.", "example"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            detect("visit https://example.com.", "example"),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn trims_unbalanced_closing_paren() {
        assert_eq!(
            detect("open (https://example.com/path)", "example"),
            Some("https://example.com/path".to_string())
        );
    }

    #[test]
    fn keeps_balanced_url_parens() {
        assert_eq!(
            detect(
                "see https://en.wikipedia.org/wiki/Rust_(video_game)",
                "wikipedia"
            ),
            Some("https://en.wikipedia.org/wiki/Rust_(video_game)".to_string())
        );
    }

    #[test]
    fn handles_query_strings() {
        assert_eq!(
            detect("open https://example.com/search?q=rust&sort=desc", "search"),
            Some("https://example.com/search?q=rust&sort=desc".to_string())
        );
    }

    #[test]
    fn requires_a_scheme_boundary() {
        assert_eq!(detect("prefixhttps://example.com", "example"), None);
    }
}
