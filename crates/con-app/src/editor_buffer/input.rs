//! Native text input uses document-wide UTF-16 offsets, independently of disk line endings.
use super::*;
use std::ops::Range;

impl EditorBuffer {
    pub fn input_text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn input_offset(&self, position: CursorPosition) -> usize {
        let position = self.clamp_position(position);
        self.lines[..position.row]
            .iter()
            .map(|line| line.encode_utf16().count() + 1)
            .sum::<usize>()
            + self.lines[position.row][..position.column]
                .encode_utf16()
                .count()
    }

    pub fn input_position(&self, offset: usize) -> CursorPosition {
        let mut remaining = offset;
        for (row, line) in self.lines.iter().enumerate() {
            let length = line.encode_utf16().count();
            if remaining <= length || row == self.lines.len() - 1 {
                return CursorPosition::new(row, utf16_byte_offset(line, remaining, false));
            }
            remaining -= length + 1;
        }
        unreachable!("buffers always have a line")
    }

    pub fn input_selection(&self) -> (Range<usize>, bool) {
        let cursor = self.input_offset(self.cursor);
        let anchor = self.input_offset(self.selection_anchor.unwrap_or(self.cursor));
        (anchor.min(cursor)..anchor.max(cursor), cursor < anchor)
    }

    pub fn input_marked_range(&self) -> Option<Range<usize>> {
        self.marked_range.clone()
    }

    pub fn unmark_input(&mut self) {
        self.marked_range = None;
    }

    pub fn input_slice(&self, range: Range<usize>) -> (String, Range<usize>) {
        let text = self.input_text();
        let bytes = utf16_byte_range(&text, range);
        let adjusted =
            text[..bytes.start].encode_utf16().count()..text[..bytes.end].encode_utf16().count();
        (text[bytes].to_string(), adjusted)
    }

    pub fn select_input_range(&mut self, range: Range<usize>) {
        let (_, range) = self.input_slice(range);
        self.selection_anchor = Some(self.input_position(range.start));
        self.cursor = self.input_position(range.end);
    }

    /// Preedit updates and the final commit are one undo operation. A supplied
    /// range is absolute; a preedit selection is relative to the inserted text.
    pub fn replace_input(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        marked_selection: Option<Range<usize>>,
        composing: bool,
    ) {
        let range = range
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.input_selection().0);
        let (_, range) = self.input_slice(range);
        let text = text.replace("\r\n", "\n").replace('\r', "");
        if range.is_empty() && text.is_empty() && self.marked_range.is_none() {
            return;
        }
        if self.marked_range.is_none() {
            self.push_undo_snapshot();
        }
        self.selection_anchor = Some(self.input_position(range.start));
        self.cursor = self.input_position(range.end);
        self.delete_selection_if_any();
        for ch in text.chars() {
            if ch == '\n' {
                self.insert_newline_raw();
            } else {
                self.insert_char(ch);
            }
        }
        let end = range.start + text.encode_utf16().count();
        self.selection_anchor = None;
        self.marked_range = (composing && !text.is_empty()).then_some(range.start..end);
        if composing && let Some(selection) = marked_selection {
            let bytes = utf16_byte_range(&text, selection);
            let start = range.start + text[..bytes.start].encode_utf16().count();
            let end = range.start + text[..bytes.end].encode_utf16().count();
            self.selection_anchor = Some(self.input_position(start));
            self.cursor = self.input_position(end);
        }
        self.dirty = true;
        self.bump_revision();
    }
}

fn utf16_byte_offset(text: &str, offset: usize, round_up: bool) -> usize {
    let mut consumed = 0;
    for (byte, ch) in text.char_indices() {
        if consumed >= offset {
            return byte;
        }
        if consumed + ch.len_utf16() > offset {
            return if round_up { byte + ch.len_utf8() } else { byte };
        }
        consumed += ch.len_utf16();
    }
    text.len()
}

fn utf16_byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    let start = utf16_byte_offset(text, range.start, false);
    let end = utf16_byte_offset(text, range.end.max(range.start), range.end > range.start);
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cjk_preedit_replaces_itself_and_commits_as_one_undo() {
        let mut b = EditorBuffer::from_text("before after");
        b.set_cursor(0, 7);
        b.replace_input(None, "ni", Some(2..2), true);
        assert_eq!(b.input_marked_range(), Some(7..9));
        b.replace_input(None, "你好", Some(2..2), true);
        assert_eq!(b.text(), "before 你好after");
        b.replace_input(None, "你好", None, false);
        assert_eq!(b.input_selection().0, 9..9);
        assert_eq!(b.input_marked_range(), None);
        assert!(b.undo());
        assert_eq!(b.text(), "before after");
        assert!(!b.is_dirty());
        assert!(!b.undo());
    }
    #[test]
    fn utf16_ranges_handle_surrogates_newlines_and_preserve_crlf_on_disk() {
        let mut b = EditorBuffer::from_text("a😀\r\n日本語\r\nx");
        assert_eq!(b.input_position(4), CursorPosition::new(1, 0));
        assert_eq!(b.input_slice(2..3), ("😀".into(), 1..3));
        b.replace_input(Some(1..6), "한글\n", None, false);
        assert_eq!(b.text(), "a한글\r\n語\r\nx");
        assert_eq!(b.input_selection().0, 4..4);
        assert!(b.undo());
        assert_eq!(b.text(), "a😀\r\n日本語\r\nx");
    }
    #[test]
    fn relative_preedit_selection_and_explicit_replacement_are_not_document_offsets() {
        let mut b = EditorBuffer::from_text("prefix 日本");
        b.set_selection(CursorPosition::new(0, 7), CursorPosition::new(0, 13));
        b.replace_input(None, "にほん😀", Some(3..5), true);
        assert_eq!(b.input_marked_range(), Some(7..12));
        assert_eq!(b.input_selection().0, 10..12);
        b.replace_input(Some(7..12), "日本", None, false);
        assert_eq!(b.text(), "prefix 日本");
        b.undo();
        assert_eq!(b.selected_text().as_deref(), Some("日本"));
    }
    #[test]
    fn cancelled_preedit_does_not_leave_latin_text_and_next_edit_is_separate() {
        let mut b = EditorBuffer::from_text("韓");
        b.set_cursor(0, 3);
        b.replace_input(None, "gks", None, true);
        b.replace_input(None, "", None, false);
        assert_eq!(b.text(), "韓");
        b.replace_input(None, "文", None, false);
        assert_eq!(b.text(), "韓文");
        b.undo();
        assert_eq!(b.text(), "韓");
    }
}
