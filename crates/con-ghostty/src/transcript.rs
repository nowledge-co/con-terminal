const MAX_TRANSCRIPT_BYTES: usize = 256 * 1024;

use crate::vt::ScreenSnapshot;

#[derive(Default)]
pub(crate) struct TranscriptBuffer {
    text: String,
}

impl TranscriptBuffer {
    pub(crate) fn push(&mut self, chunk: &str) {
        self.text.push_str(chunk);
        if self.text.len() <= MAX_TRANSCRIPT_BYTES {
            return;
        }

        let mut keep_from = self.text.len().saturating_sub(MAX_TRANSCRIPT_BYTES);
        while keep_from < self.text.len() && !self.text.is_char_boundary(keep_from) {
            keep_from += 1;
        }
        if keep_from > 0 {
            self.text.drain(..keep_from);
        }
    }

    pub(crate) fn recent_lines(&self, max_lines: usize) -> Vec<String> {
        if max_lines == 0 {
            return Vec::new();
        }
        let sanitized = sanitize_terminal_output(&self.text);
        let mut lines: Vec<String> = sanitized
            .lines()
            .rev()
            .take(max_lines)
            .map(ToOwned::to_owned)
            .collect();
        lines.reverse();
        lines
    }

    pub(crate) fn search(&self, pattern: &str, limit: usize) -> Vec<(usize, String)> {
        if pattern.is_empty() || limit == 0 {
            return Vec::new();
        }
        sanitize_terminal_output(&self.text)
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains(pattern))
            .take(limit)
            .map(|(idx, line)| (idx, line.to_string()))
            .collect()
    }
}

pub(crate) fn sanitize_terminal_output(raw: &str) -> String {
    #[derive(Clone, Copy)]
    enum EscapeState {
        None,
        Esc,
        Csi,
        Osc,
        OscEsc,
        Ss3,
        Charset,
        Dcs,
        DcsEsc,
    }

    let mut output = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut index = 0;
    let mut state = EscapeState::None;

    while index < bytes.len() {
        let byte = bytes[index];
        match state {
            EscapeState::None => match byte {
                b'\x1b' => {
                    state = EscapeState::Esc;
                    index += 1;
                }
                b'\r' => {
                    if bytes.get(index + 1) != Some(&b'\n') {
                        clear_current_line(&mut output);
                    }
                    index += 1;
                }
                b'\x08' => {
                    if !output.ends_with('\n') {
                        output.pop();
                    }
                    index += 1;
                }
                b'\t' => {
                    output.push_str("    ");
                    index += 1;
                }
                b'\n' => {
                    output.push('\n');
                    index += 1;
                }
                0x00..=0x1f | 0x7f => {
                    index += 1;
                }
                _ => {
                    let ch = raw[index..]
                        .chars()
                        .next()
                        .expect("valid utf-8 chunk while sanitizing pty output");
                    output.push(ch);
                    index += ch.len_utf8();
                }
            },
            EscapeState::Esc => {
                state = match byte {
                    b'[' => EscapeState::Csi,
                    b']' => EscapeState::Osc,
                    b'O' => EscapeState::Ss3,
                    b'(' | b')' | b'*' | b'+' => EscapeState::Charset,
                    b'P' | b'X' | b'^' | b'_' => EscapeState::Dcs,
                    _ => EscapeState::None,
                };
                index += 1;
            }
            EscapeState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    state = EscapeState::None;
                }
                index += 1;
            }
            EscapeState::Osc => {
                match byte {
                    b'\x07' => state = EscapeState::None,
                    b'\x1b' => state = EscapeState::OscEsc,
                    _ => {}
                }
                index += 1;
            }
            EscapeState::OscEsc => {
                state = if byte == b'\\' {
                    EscapeState::None
                } else {
                    EscapeState::Osc
                };
                index += 1;
            }
            EscapeState::Ss3 => {
                if (0x40..=0x7e).contains(&byte) {
                    state = EscapeState::None;
                }
                index += 1;
            }
            EscapeState::Charset => {
                state = EscapeState::None;
                index += 1;
            }
            EscapeState::Dcs => {
                match byte {
                    b'\x07' => state = EscapeState::None,
                    b'\x1b' => state = EscapeState::DcsEsc,
                    _ => {}
                }
                index += 1;
            }
            EscapeState::DcsEsc => {
                state = if byte == b'\\' {
                    EscapeState::None
                } else {
                    EscapeState::Dcs
                };
                index += 1;
            }
        }
    }

    output
}

pub(crate) fn snapshot_to_lines(snapshot: &ScreenSnapshot, max_lines: usize) -> Vec<String> {
    if max_lines == 0 || snapshot.cols == 0 || snapshot.rows == 0 {
        return Vec::new();
    }

    let cols = usize::from(snapshot.cols);
    let mut lines = Vec::with_capacity(usize::from(snapshot.rows));

    for row in 0..usize::from(snapshot.rows) {
        let row_start = row * cols;
        let row_end = row_start + cols;
        let Some(cells) = snapshot.cells.get(row_start..row_end) else {
            break;
        };

        let mut line = String::with_capacity(cols);
        for cell in cells {
            let ch = match cell.codepoint {
                0 => ' ',
                codepoint => char::from_u32(codepoint).unwrap_or('\u{FFFD}'),
            };
            line.push(ch);
        }

        lines.push(line.trim_end_matches(' ').to_string());
    }

    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }

    if lines.len() > max_lines {
        lines.drain(..lines.len() - max_lines);
    }

    lines
}

fn clear_current_line(output: &mut String) {
    while output.chars().last().is_some_and(|ch| ch != '\n') {
        output.pop();
    }
}

#[cfg(test)]
mod tests {
    use crate::vt::{Cell, Cursor, ScreenSnapshot};

    use super::{TranscriptBuffer, sanitize_terminal_output, snapshot_to_lines};

    #[test]
    fn transcript_buffer_returns_recent_lines_in_order() {
        let mut transcript = TranscriptBuffer::default();
        transcript.push("one\ntwo\nthree\nfour\n");

        assert_eq!(
            transcript.recent_lines(2),
            vec!["three".to_string(), "four".to_string()]
        );
    }

    #[test]
    fn transcript_buffer_search_is_bounded() {
        let mut transcript = TranscriptBuffer::default();
        transcript.push("alpha\nbeta\nalphabet\n");

        assert_eq!(
            transcript.search("alpha", 1),
            vec![(0, "alpha".to_string())]
        );
    }

    #[test]
    fn sanitize_terminal_output_strips_ansi_sequences() {
        assert_eq!(
            sanitize_terminal_output("\x1b]0;title\x07\x1b[31mhello\x1b[0m"),
            "hello"
        );
    }

    #[test]
    fn sanitize_terminal_output_honors_carriage_return_rewrites() {
        assert_eq!(sanitize_terminal_output("loading\rready"), "ready");
    }

    #[test]
    fn snapshot_to_lines_trims_trailing_blank_rows() {
        let mut cells = vec![Cell::default(); 6];
        cells[0].codepoint = 'p' as u32;
        cells[1].codepoint = 's' as u32;
        cells[2].codepoint = '1' as u32;

        let snapshot = ScreenSnapshot {
            cols: 3,
            rows: 2,
            cells,
            kitty_placements: Default::default(),
            dirty_rows: vec![0, 1],
            cursor: Cursor::default(),
            alternate_screen: false,
            scrollbar: None,
            title: None,
            generation: 1,
        };

        assert_eq!(snapshot_to_lines(&snapshot, 10), vec!["ps1".to_string()]);
    }
}
