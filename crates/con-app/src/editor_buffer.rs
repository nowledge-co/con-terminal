//! Editable text buffer for the lightweight editor pane.
//!
//! This module deliberately has no GPUI dependencies so editing behavior stays
//! unit-testable and can evolve independently from rendering.

use std::{collections::VecDeque, ops::Range};

mod input;

const MAX_UNDO_ENTRIES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CursorPosition {
    pub row: usize,
    pub column: usize,
}

impl CursorPosition {
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

#[derive(Debug, Clone)]
pub struct EditorBuffer {
    lines: Vec<String>,
    line_ending: LineEnding,
    cursor: CursorPosition,
    selection_anchor: Option<CursorPosition>,
    dirty: bool,
    undo_stack: VecDeque<BufferSnapshot>,
    revision: u64,
    marked_range: Option<Range<usize>>,
}

#[derive(Debug, Clone)]
struct BufferSnapshot {
    lines: Vec<String>,
    line_ending: LineEnding,
    cursor: CursorPosition,
    selection_anchor: Option<CursorPosition>,
    dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Self::Crlf
        } else {
            Self::Lf
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

impl EditorBuffer {
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let line_ending = LineEnding::detect(&text);
        let mut lines = text.lines().map(ToString::to_string).collect::<Vec<_>>();
        if text.ends_with('\n') {
            lines.push(String::new());
        }
        if lines.is_empty() {
            lines.push(String::new());
        }

        Self {
            lines,
            line_ending,
            cursor: CursorPosition::new(0, 0),
            selection_anchor: None,
            dirty: false,
            undo_stack: VecDeque::new(),
            revision: 0,
            marked_range: None,
        }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn cursor(&self) -> CursorPosition {
        self.cursor
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn has_selection(&self) -> bool {
        self.normalized_selection().is_some()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_cursor(&mut self, row: usize, column: usize) {
        let row = row.min(self.lines.len().saturating_sub(1));
        let column = clamp_to_char_boundary(&self.lines[row], column.min(self.lines[row].len()));
        self.cursor = CursorPosition::new(row, column);
        self.clear_selection();
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        self.push_undo_snapshot();
        let _ = self.delete_selection_if_any();
        for ch in text.chars() {
            match ch {
                '\r' => {}
                '\n' => self.insert_newline_raw(),
                ch => self.insert_char(ch),
            }
        }
        self.dirty = true;
        self.bump_revision();
    }

    pub fn insert_newline(&mut self) {
        self.push_undo_snapshot();
        let _ = self.delete_selection_if_any();
        self.insert_newline_raw();
        self.dirty = true;
        self.bump_revision();
    }

    pub fn delete_backward(&mut self) {
        if self.has_selection() {
            self.push_undo_snapshot();
            if self.delete_selection_if_any() {
                self.dirty = true;
                self.bump_revision();
            }
            return;
        }

        if self.cursor.column == 0 && self.cursor.row == 0 {
            return;
        }

        self.push_undo_snapshot();
        if self.delete_selection_if_any() {
            self.dirty = true;
            return;
        }

        if self.cursor.column > 0 {
            let row = self.cursor.row;
            let column = self.cursor.column;
            let previous = previous_char_start(&self.lines[row], column).unwrap_or(0);
            self.lines[row].replace_range(previous..column, "");
            self.cursor.column = previous;
            self.dirty = true;
            self.bump_revision();
            return;
        }

        if self.cursor.row == 0 {
            return;
        }

        let row = self.cursor.row;
        let current = self.lines.remove(row);
        let previous_len = self.lines[row - 1].len();
        self.lines[row - 1].push_str(&current);
        self.cursor = CursorPosition::new(row - 1, previous_len);
        self.dirty = true;
        self.bump_revision();
    }

    pub fn delete_forward(&mut self) {
        if self.has_selection() {
            self.push_undo_snapshot();
            if self.delete_selection_if_any() {
                self.dirty = true;
                self.bump_revision();
            }
            return;
        }

        let row = self.cursor.row;
        let column = self.cursor.column;
        if column >= self.lines[row].len() && row + 1 >= self.lines.len() {
            return;
        }

        self.push_undo_snapshot();
        if self.delete_selection_if_any() {
            self.dirty = true;
            return;
        }

        if column < self.lines[row].len() {
            self.lines[row].remove(column);
            self.dirty = true;
            self.bump_revision();
            return;
        }

        if row + 1 >= self.lines.len() {
            return;
        }

        let next = self.lines.remove(row + 1);
        self.lines[row].push_str(&next);
        self.dirty = true;
        self.bump_revision();
    }

    pub fn move_left(&mut self) {
        self.clear_selection();
        self.move_left_raw();
    }

    pub fn move_right(&mut self) {
        self.clear_selection();
        self.move_right_raw();
    }

    pub fn move_up(&mut self) {
        self.clear_selection();
        self.move_up_raw();
    }

    pub fn move_down(&mut self) {
        self.clear_selection();
        self.move_down_raw();
    }

    pub fn move_home(&mut self) {
        self.clear_selection();
        self.move_to_line_start();
    }

    pub fn move_end(&mut self) {
        self.clear_selection();
        self.move_to_line_end();
    }

    pub fn move_to_line_start(&mut self) {
        self.cursor.column = 0;
    }

    pub fn move_to_line_end(&mut self) {
        self.cursor.column = self.lines[self.cursor.row].len();
    }

    pub fn move_left_selecting(&mut self) {
        self.move_selecting(Self::move_left_raw);
    }

    pub fn move_right_selecting(&mut self) {
        self.move_selecting(Self::move_right_raw);
    }

    pub fn move_up_selecting(&mut self) {
        self.move_selecting(Self::move_up_raw);
    }

    pub fn move_down_selecting(&mut self) {
        self.move_selecting(Self::move_down_raw);
    }

    pub fn move_home_selecting(&mut self) {
        self.move_selecting(Self::move_to_line_start);
    }

    pub fn move_end_selecting(&mut self) {
        self.move_selecting(Self::move_to_line_end);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_selection(&mut self, anchor: CursorPosition, cursor: CursorPosition) {
        self.unmark_input();
        self.selection_anchor = Some(self.clamp_position(anchor));
        self.cursor = self.clamp_position(cursor);
    }

    pub fn select_word_at(&mut self, row: usize, column: usize) -> bool {
        self.unmark_input();
        let row = row.min(self.lines.len().saturating_sub(1));
        let Some((start, end)) = word_range_at_column(&self.lines[row], column) else {
            self.set_cursor(row, column);
            return false;
        };

        self.selection_anchor = Some(CursorPosition::new(row, start));
        self.cursor = CursorPosition::new(row, end);
        true
    }

    pub fn select_all(&mut self) {
        self.unmark_input();
        self.selection_anchor = Some(CursorPosition::new(0, 0));
        let last_row = self.lines.len().saturating_sub(1);
        self.cursor = CursorPosition::new(last_row, self.lines[last_row].len());
    }

    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.normalized_selection()?;
        Some(self.text_in_range(start, end))
    }

    pub fn cut_selection(&mut self) -> Option<String> {
        let text = self.selected_text()?;
        self.push_undo_snapshot();
        self.delete_selection_if_any();
        self.dirty = true;
        self.bump_revision();
        Some(text)
    }

    pub fn text(&self) -> String {
        self.lines.join(self.line_ending.as_str())
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn save_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, self.text())?;
        self.mark_clean();
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        self.unmark_input();
        let Some(snapshot) = self.undo_stack.pop_back() else {
            return false;
        };
        self.lines = snapshot.lines;
        self.line_ending = snapshot.line_ending;
        self.cursor = snapshot.cursor;
        self.selection_anchor = snapshot.selection_anchor;
        self.dirty = snapshot.dirty;
        self.bump_revision();
        true
    }

    fn push_undo_snapshot(&mut self) {
        self.unmark_input();
        while self.undo_stack.len() >= MAX_UNDO_ENTRIES {
            let _ = self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(BufferSnapshot {
            lines: self.lines.clone(),
            line_ending: self.line_ending,
            cursor: self.cursor,
            selection_anchor: self.selection_anchor,
            dirty: self.dirty,
        });
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn move_right_raw(&mut self) {
        if self.cursor.column < self.lines[self.cursor.row].len() {
            self.cursor.column = next_char_end(&self.lines[self.cursor.row], self.cursor.column);
        } else if self.cursor.row + 1 < self.lines.len() {
            self.cursor.row += 1;
            self.cursor.column = 0;
        }
    }

    fn move_left_raw(&mut self) {
        if self.cursor.column > 0 {
            self.cursor.column =
                previous_char_start(&self.lines[self.cursor.row], self.cursor.column).unwrap_or(0);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.cursor.column = self.lines[self.cursor.row].len();
        }
    }

    fn move_up_raw(&mut self) {
        if self.cursor.row == 0 {
            return;
        }
        self.cursor.row -= 1;
        self.cursor.column = clamp_to_char_boundary(
            &self.lines[self.cursor.row],
            self.cursor.column.min(self.lines[self.cursor.row].len()),
        );
    }

    fn move_down_raw(&mut self) {
        if self.cursor.row + 1 >= self.lines.len() {
            return;
        }
        self.cursor.row += 1;
        self.cursor.column = clamp_to_char_boundary(
            &self.lines[self.cursor.row],
            self.cursor.column.min(self.lines[self.cursor.row].len()),
        );
    }

    fn move_selecting(&mut self, move_cursor: fn(&mut Self)) {
        self.unmark_input();
        let anchor = self.selection_anchor.unwrap_or(self.cursor);
        move_cursor(self);
        self.selection_anchor = Some(anchor);
    }

    fn clear_selection(&mut self) {
        self.unmark_input();
        self.selection_anchor = None;
    }

    pub fn normalized_selection(&self) -> Option<(CursorPosition, CursorPosition)> {
        let anchor = self.selection_anchor?;
        if anchor == self.cursor {
            return None;
        }
        let (start, end) = if anchor <= self.cursor {
            (anchor, self.cursor)
        } else {
            (self.cursor, anchor)
        };
        Some((self.clamp_position(start), self.clamp_position(end)))
    }

    fn clamp_position(&self, position: CursorPosition) -> CursorPosition {
        let row = position.row.min(self.lines.len().saturating_sub(1));
        CursorPosition::new(
            row,
            clamp_to_char_boundary(&self.lines[row], position.column.min(self.lines[row].len())),
        )
    }

    fn text_in_range(&self, start: CursorPosition, end: CursorPosition) -> String {
        if start.row == end.row {
            return self.lines[start.row][start.column..end.column].to_string();
        }

        let mut text = String::new();
        text.push_str(&self.lines[start.row][start.column..]);
        for row in (start.row + 1)..end.row {
            text.push_str(self.line_ending.as_str());
            text.push_str(&self.lines[row]);
        }
        text.push_str(self.line_ending.as_str());
        text.push_str(&self.lines[end.row][..end.column]);
        text
    }

    fn delete_selection_if_any(&mut self) -> bool {
        let Some((start, end)) = self.normalized_selection() else {
            return false;
        };

        if start.row == end.row {
            self.lines[start.row].replace_range(start.column..end.column, "");
        } else {
            let tail = self.lines[end.row][end.column..].to_string();
            self.lines[start.row].replace_range(start.column.., "");
            self.lines[start.row].push_str(&tail);
            self.lines.drain((start.row + 1)..=end.row);
        }
        self.cursor = start;
        self.clear_selection();
        true
    }

    fn insert_newline_raw(&mut self) {
        let row = self.cursor.row;
        let column = self.cursor.column;
        let tail = self.lines[row].split_off(column);
        self.lines.insert(row + 1, tail);
        self.cursor = CursorPosition::new(row + 1, 0);
    }

    fn insert_char(&mut self, ch: char) {
        let row = self.cursor.row;
        let column = self.cursor.column;
        self.lines[row].insert(column, ch);
        self.cursor.column += ch.len_utf8();
    }
}

fn word_range_at_column(line: &str, column: usize) -> Option<(usize, usize)> {
    if line.is_empty() {
        return None;
    }

    let column = clamp_to_char_boundary(line, column.min(line.len()));
    let target = if column < line.len() && is_word_char_at(line, column) {
        column
    } else if column == line.len() {
        previous_char_start(line, column).filter(|&start| is_word_char_at(line, start))?
    } else {
        return None;
    };

    let mut start = target;
    while let Some(previous) = previous_char_start(line, start) {
        if !is_word_char_at(line, previous) {
            break;
        }
        start = previous;
    }

    let mut end = next_char_end(line, target);
    while end < line.len() && is_word_char_at(line, end) {
        end = next_char_end(line, end);
    }

    Some((start, end))
}

fn is_word_char_at(line: &str, index: usize) -> bool {
    line[index..]
        .chars()
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_alphanumeric())
}

fn previous_char_start(line: &str, index: usize) -> Option<usize> {
    line[..index].char_indices().last().map(|(index, _)| index)
}

fn next_char_end(line: &str, index: usize) -> usize {
    let ch = line[index..].chars().next().expect("char at byte index");
    index + ch.len_utf8()
}

fn clamp_to_char_boundary(line: &str, mut column: usize) -> usize {
    while column > 0 && !line.is_char_boundary(column) {
        column -= 1;
    }
    column
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_text_updates_line_cursor_and_dirty_state() {
        let mut buffer = EditorBuffer::from_text("hello");

        buffer.set_cursor(0, 5);
        buffer.insert_text(" world");

        assert_eq!(buffer.text(), "hello world");
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 11));
        assert!(buffer.is_dirty());
    }

    #[test]
    fn undo_restores_text_cursor_selection_and_dirty_state() {
        let mut buffer = EditorBuffer::from_text("hello");

        buffer.set_cursor(0, 5);
        buffer.insert_text(" world");
        assert_eq!(buffer.text(), "hello world");

        assert!(buffer.undo());

        assert_eq!(buffer.text(), "hello");
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 5));
        assert!(!buffer.has_selection());
        assert!(!buffer.is_dirty());
    }

    #[test]
    fn undo_history_is_bounded() {
        let mut buffer = EditorBuffer::from_text("");

        for _ in 0..(MAX_UNDO_ENTRIES + 8) {
            buffer.insert_text("x");
        }

        assert_eq!(buffer.undo_stack.len(), MAX_UNDO_ENTRIES);
    }

    #[test]
    fn revision_changes_only_when_text_changes() {
        let mut buffer = EditorBuffer::from_text("hello");
        let initial_revision = buffer.revision();

        buffer.set_cursor(0, 2);
        assert_eq!(buffer.revision(), initial_revision);

        buffer.insert_text("!");
        assert!(buffer.revision() > initial_revision);
        let edited_revision = buffer.revision();

        assert!(buffer.undo());
        assert!(buffer.revision() > edited_revision);
    }

    #[test]
    fn enter_splits_line_and_backspace_joins_lines() {
        let mut buffer = EditorBuffer::from_text("hello world");

        buffer.set_cursor(0, 5);
        buffer.insert_newline();

        assert_eq!(buffer.lines(), &["hello".to_string(), " world".to_string()]);
        assert_eq!(buffer.cursor(), CursorPosition::new(1, 0));

        buffer.delete_backward();

        assert_eq!(buffer.text(), "hello world");
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 5));
    }

    #[test]
    fn delete_forward_merges_next_line_at_line_end() {
        let mut buffer = EditorBuffer::from_text("hello\nworld");

        buffer.set_cursor(0, 5);
        buffer.delete_forward();

        assert_eq!(buffer.text(), "helloworld");
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 5));
    }

    #[test]
    fn vertical_movement_clamps_column_to_target_line_length() {
        let mut buffer = EditorBuffer::from_text("abcdef\nxy");

        buffer.set_cursor(0, 5);
        buffer.move_down();

        assert_eq!(buffer.cursor(), CursorPosition::new(1, 2));
    }

    #[test]
    fn horizontal_movement_keeps_cursor_on_utf8_boundaries() {
        let mut buffer = EditorBuffer::from_text("éx");

        buffer.move_right();
        assert_eq!(buffer.cursor(), CursorPosition::new(0, "é".len()));

        buffer.insert_text("!");
        assert_eq!(buffer.text(), "é!x");

        buffer.move_left();
        assert_eq!(buffer.cursor(), CursorPosition::new(0, "é".len()));
        buffer.delete_backward();

        assert_eq!(buffer.text(), "!x");
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 0));
    }

    #[test]
    fn vertical_movement_clamps_to_utf8_boundary() {
        let mut buffer = EditorBuffer::from_text("ab\né");

        buffer.set_cursor(0, 1);
        buffer.move_down();
        assert!(buffer.lines()[1].is_char_boundary(buffer.cursor().column));

        buffer.insert_text("!");
        assert_eq!(buffer.text(), "ab\n!é");
    }

    #[test]
    fn select_all_and_replace_with_text() {
        let mut buffer = EditorBuffer::from_text("hello\nworld");

        buffer.select_all();
        assert_eq!(buffer.selected_text().as_deref(), Some("hello\nworld"));
        buffer.insert_text("replacement");

        assert_eq!(buffer.text(), "replacement");
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 11));
        assert!(!buffer.has_selection());
    }

    #[test]
    fn copy_and_cut_selected_text() {
        let mut buffer = EditorBuffer::from_text("hello world");
        buffer.set_selection(CursorPosition::new(0, 6), CursorPosition::new(0, 11));

        assert_eq!(buffer.selected_text().as_deref(), Some("world"));
        assert_eq!(buffer.cut_selection().as_deref(), Some("world"));
        assert_eq!(buffer.text(), "hello ");
        assert!(buffer.is_dirty());
    }

    #[test]
    fn ctrl_a_and_ctrl_e_move_to_line_boundaries() {
        let mut buffer = EditorBuffer::from_text("hello world");
        buffer.set_cursor(0, 5);

        buffer.move_to_line_start();
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 0));

        buffer.move_to_line_end();
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 11));
    }

    #[test]
    fn non_selecting_line_boundary_moves_clear_selection() {
        let mut buffer = EditorBuffer::from_text("hello world");
        buffer.set_selection(CursorPosition::new(0, 0), CursorPosition::new(0, 5));

        buffer.move_end();

        assert_eq!(buffer.cursor(), CursorPosition::new(0, 11));
        assert!(!buffer.has_selection());

        buffer.set_selection(CursorPosition::new(0, 0), CursorPosition::new(0, 5));

        buffer.move_home();

        assert_eq!(buffer.cursor(), CursorPosition::new(0, 0));
        assert!(!buffer.has_selection());
    }

    #[test]
    fn shift_movement_extends_selection() {
        let mut buffer = EditorBuffer::from_text("hello");
        buffer.set_cursor(0, 1);

        buffer.move_right_selecting();
        buffer.move_right_selecting();

        assert_eq!(buffer.cursor(), CursorPosition::new(0, 3));
        assert_eq!(buffer.selected_text().as_deref(), Some("el"));

        buffer.move_left();
        assert!(!buffer.has_selection());
        assert_eq!(buffer.cursor(), CursorPosition::new(0, 2));
    }

    #[test]
    fn shift_vertical_movement_extends_multiline_selection() {
        let mut buffer = EditorBuffer::from_text("abc\ndef");
        buffer.set_cursor(0, 1);

        buffer.move_down_selecting();

        assert_eq!(buffer.cursor(), CursorPosition::new(1, 1));
        assert_eq!(buffer.selected_text().as_deref(), Some("bc\nd"));
    }

    #[test]
    fn selecting_empty_range_does_not_copy_or_cut() {
        let mut buffer = EditorBuffer::from_text("hello");
        buffer.set_selection(CursorPosition::new(0, 2), CursorPosition::new(0, 2));

        assert_eq!(buffer.selected_text(), None);
        assert_eq!(buffer.cut_selection(), None);
        assert_eq!(buffer.text(), "hello");
    }

    #[test]
    fn select_word_at_selects_identifier_under_cursor() {
        let mut buffer = EditorBuffer::from_text("pub use iceberg_inspect::IcebergInspectTable;");

        assert!(buffer.select_word_at(0, 10));

        assert_eq!(buffer.cursor(), CursorPosition::new(0, 23));
        assert_eq!(buffer.selected_text().as_deref(), Some("iceberg_inspect"));
    }

    #[test]
    fn select_word_at_line_end_selects_previous_word() {
        let mut buffer = EditorBuffer::from_text("hello");

        assert!(buffer.select_word_at(0, 5));

        assert_eq!(buffer.selected_text().as_deref(), Some("hello"));
    }

    #[test]
    fn select_word_at_separator_places_cursor_without_selection() {
        let mut buffer = EditorBuffer::from_text("hello world");

        assert!(!buffer.select_word_at(0, 5));

        assert_eq!(buffer.cursor(), CursorPosition::new(0, 5));
        assert!(!buffer.has_selection());
    }

    #[test]
    fn save_writes_text_to_disk_and_marks_buffer_clean() {
        let path = std::env::temp_dir().join(format!(
            "con-editor-buffer-save-{}-{}.txt",
            std::process::id(),
            unique_suffix()
        ));
        let mut buffer = EditorBuffer::from_text("hello");
        buffer.set_cursor(0, 5);
        buffer.insert_text(" world");

        buffer.save_to(&path).expect("save buffer");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
        assert!(!buffer.is_dirty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn text_preserves_detected_crlf_line_endings() {
        let mut buffer = EditorBuffer::from_text("hello\r\nworld\r\n");

        buffer.set_cursor(1, 5);
        buffer.insert_text("!");

        assert_eq!(buffer.text(), "hello\r\nworld!\r\n");
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
